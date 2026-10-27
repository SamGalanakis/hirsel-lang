//! Constants, statics, callables, closures and constructions: inline values in zeroed stack
//! storage, heap blocks through `hirsel_rt_v2_alloc` and `hirsel_rt_v2_reuse`, arena blocks
//! through `hirsel_rt_v2_arena_alloc`, and compiler-local region storage, as `hirsel-llvm`'s
//! `construction`, `constants` and `callables` build them without inline fast paths.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::Switch;

use hirsel_lir::emission::{
    IMMORTAL_PREFIX_BYTES, function_thunk_name, slice_storage_with_index, symbol_name,
};
use hirsel_lir::ir::*;
use hirsel_lir::layout::{
    SEQUENCE_CAPACITY_OFFSET, align_up, sequence_elements_offset, sequence_slice_start,
};
use hirsel_lir::layout_types::{
    DropStep, FieldLayout, LayoutId, LayoutKind, ScalarKind, SizeSpec, TagForm,
};
use hirsel_lir::runtime_abi::{COUNT_IMMORTAL, COUNT_OFFSET, DomainTag, RuntimeEntry};

use crate::data::StaticBlock;
use crate::function::{FnEmitter, Place, Shape, V, low_bit};
use crate::module::{Emitted, Hidden, PTR};

impl<'a> FnEmitter<'_, 'a, '_> {
    /// The address of a static block's header.
    pub(crate) fn static_header(&mut self, block: StaticBlock) -> Value {
        self.data_addr(block.data, block.header)
    }

    /// `layout`'s descriptor index as the `u32` descriptor argument, loaded from its glue
    /// cell (`hirsel_lir::glue`): units carry no number the program assigns.
    pub(crate) fn descriptor_value(&mut self, layout: LayoutId) -> Emitted<Value> {
        self.layout(layout)?;
        let name = hirsel_lir::glue::descriptor_index_symbol(&self.em.inputs.keys.key(layout));
        let cell = self.em.glue_data(&name)?;
        let address = self.data_addr(cell, 0);
        Ok(self.load_ty(types::I32, address, 0))
    }

    /// The descriptor index a block's header names.
    pub(crate) fn block_descriptor(&mut self, block: Value) -> Value {
        let word = self.load_ty(types::I32, block, 4);
        self.b.ins().band_imm_s(word, 0x1fff_ffff)
    }

    /// The `const uint64_t *live_mask` of a release: null when every field is live.
    pub(crate) fn live_mask(&mut self, live: MaskRef) -> Emitted<Value> {
        match live {
            MaskRef::All => Ok(self.b.ins().iconst(PTR, 0)),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
                if words.is_empty() {
                    return self.invalid("a static mask without words");
                }
                let id = self.em.mask_words(words)?;
                Ok(self.data_addr(id, 0))
            }
            MaskRef::Dynamic(mask) => {
                let kind = self.scalar_kind(mask)?;
                let Some((width, false)) = crate::scalar::integer(kind) else {
                    return self.invalid("a mask that is not an unsigned integer");
                };
                let mut word = self.get1(mask)?;
                if width < 64 {
                    word = self.b.ins().uextend(types::I64, word);
                } else if width > 64 {
                    return self.unsupported("a live mask wider than one word");
                }
                let spill = self.out_word();
                self.store_at(word, spill, 0);
                Ok(spill)
            }
        }
    }

    pub(crate) fn constant(&mut self, constant: ConstantId) -> Emitted<V> {
        let Some(data) = self.unit.tables.constants.get(constant.index()) else {
            return self.invalid("a constant outside the constant table");
        };
        let shape = self.shape(self.lir_type(data.ty)?)?;
        Ok(match (&data.value, shape) {
            (ConstantValue::Heap, Shape::Scalar(ScalarKind::Pointer)) => {
                V::One(self.b.ins().iconst(PTR, 0))
            }
            (ConstantValue::Unit, Shape::Unit) => V::Unit,
            (ConstantValue::Bool(b), Shape::Scalar(ScalarKind::Bool)) => {
                V::One(self.b.ins().iconst(types::I8, i64::from(*b)))
            }
            (ConstantValue::Integer(bits), Shape::Scalar(kind))
                if crate::scalar::integer(kind).is_some() =>
            {
                let width = crate::scalar::integer(kind).unwrap().0;
                if width < 128 && *bits >> width != 0 {
                    return self.invalid("an integer constant wider than its type");
                }
                let ty = crate::module::scalar_type(kind);
                V::One(self.int_const(ty, *bits))
            }
            (ConstantValue::F32(bits), Shape::Scalar(ScalarKind::F32)) => {
                V::One(self.float_const(ScalarKind::F32, u64::from(*bits)))
            }
            (ConstantValue::F64(bits), Shape::Scalar(ScalarKind::F64)) => {
                V::One(self.float_const(ScalarKind::F64, *bits))
            }
            (ConstantValue::String(text), _) => return self.text_literal(data.ty, text.as_bytes()),
            (ConstantValue::Bytes(bytes), _) => return self.text_literal(data.ty, bytes),
            (ConstantValue::Descriptor(layout), Shape::Scalar(ScalarKind::U32)) => {
                V::One(self.descriptor_value(*layout)?)
            }
            _ => return self.invalid("a constant of another type"),
        })
    }

    /// A String or Bytes literal over a static immortal block: a Plain slice (owner, start,
    /// length) for a `Slice` layout, else the block reference itself.
    fn text_literal(&mut self, ty: LirTypeId, bytes: &[u8]) -> Emitted<V> {
        let ty = self.lir_type(ty)?;
        let record = self.layout(ty.layout)?;
        let definition = self.definition();
        match (ty.repr, &record.kind) {
            (ValueRepr::Aggregate, &LayoutKind::Slice { element }) => {
                let field = |ordinal: u32| record.fields.iter().find(|f| f.ordinal == ordinal);
                let (Some(owner), Some(start), Some(length)) = (field(0), field(1), field(2))
                else {
                    return self.invalid("a slice layout without owner, start and length");
                };
                let Some(payload) = slice_storage_with_index(
                    self.em.layouts(),
                    self.em.inputs.unit.and_then(|i| i.index),
                    element,
                ) else {
                    return self.invalid("a slice without its element's sequence layout");
                };
                let block = self.em.literal_block(payload, bytes, definition)?;
                let whole = match sequence_slice_start(self.layout(element)?) {
                    Ok(whole) => whole,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let place = self.stack(ty.layout)?;
                let size = self.fixed_size(ty.layout)?;
                self.zero(place.ptr, size, place.align);
                let header = self.static_header(block);
                self.store_at(header, place.ptr, owner.offset);
                let whole = self.u64c(whole);
                self.store_at(whole, place.ptr, start.offset);
                let count = self.u64c(bytes.len() as u64);
                self.store_at(count, place.ptr, length.offset);
                let shape = self.layout_shape(ty.layout)?;
                Ok(self.load(shape, place))
            }
            (
                ValueRepr::Block | ValueRepr::Uncounted,
                LayoutKind::Block { payload } | LayoutKind::Box { payload },
            ) => {
                let payload = *payload;
                let block = self.em.literal_block(payload, bytes, definition)?;
                let header = self.static_header(block);
                if ty.repr == ValueRepr::Block {
                    return Ok(V::One(header));
                }
                Ok(V::One(self.payload_of(header, payload)?.ptr))
            }
            _ => self.invalid("a text constant of another type"),
        }
    }

    /// `ReadStatic`: the static immortal blocks of the symbol's data.
    pub(crate) fn read_static(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<V> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a static outside the symbol table");
        };
        let Some(data) = self
            .em
            .inputs
            .unit()
            .statics
            .iter()
            .find(|d| d.symbol == *symbol)
        else {
            return self.invalid("a static without its data");
        };
        let Some(root) = data.blocks.first() else {
            return self.invalid("static data without a block");
        };
        let held = root.layout;
        // Glue defines the blocks: their prefixes hold the program's descriptor indices.
        let definition = self.definition();
        let align = self.layout(held)?.align;
        let block =
            self.em
                .glue_block(&hirsel_lir::glue::static_symbol(symbol), align, definition)?;
        let header = self.static_header(block);
        let ty = self.value_type(result)?;
        match (ty.repr, &self.layout(ty.layout)?.kind) {
            (
                ValueRepr::Block,
                LayoutKind::Block { payload: p } | LayoutKind::Box { payload: p },
            ) if *p == held => Ok(V::One(header)),
            (ValueRepr::Uncounted, LayoutKind::Block { .. } | LayoutKind::Box { .. }) => {
                Ok(V::One(self.payload_of(header, held)?.ptr))
            }
            _ if ty.layout == held => {
                let place = self.payload_of(header, held)?;
                Ok(match self.layout_shape(held)? {
                    // Read-only data is never written while the value is live: no copy.
                    Shape::Bytes { .. } => V::Mem {
                        addr: place.ptr,
                        slot: None,
                    },
                    shape => self.load(shape, place),
                })
            }
            _ => self.invalid("a static read at another layout than its data"),
        }
    }

    /// A callable value: the code pointer, then the environment block (an iterator's
    /// environment first).
    fn callable(&mut self, layout: LayoutId, code: Value, environment: Value) -> Emitted<V> {
        if self.layout_shape(layout)? == Shape::Callable {
            return Ok(V::Pair(code, environment));
        }
        let place = self.stack(layout)?;
        let size = self.fixed_size(layout)?;
        self.zero(place.ptr, size, place.align);
        let iterator = matches!(self.layout(layout)?.kind, LayoutKind::Iterator { .. });
        let (code_at, env_at) = if iterator { (8, 0) } else { (0, 8) };
        self.store_at(code, place.ptr, code_at);
        self.store_at(environment, place.ptr, env_at);
        let shape = self.layout_shape(layout)?;
        Ok(self.load(shape, place))
    }

    /// The empty environment's header address.
    fn empty_environment(&mut self) -> Emitted<Value> {
        let definition = self.definition();
        let block = self.em.empty_environment(definition)?;
        Ok(self.static_header(block))
    }

    /// The address of function `name`, declared at `sig` (an address-only import of a
    /// function-reference thunk takes no signature: no code calls it by name).
    fn function_address(
        &mut self,
        name: &str,
        sig: Option<&cranelift_codegen::ir::Signature>,
    ) -> Emitted<Value> {
        let id = match sig {
            Some(sig) => self.em.import_function(name, sig)?,
            None => {
                let sig = cranelift_codegen::ir::Signature::new(self.em.call_conv);
                self.em.import_function(name, &sig)?
            }
        };
        Ok(self.func_addr(id))
    }

    /// `FunctionRef`: the definition's function-reference thunk with the static empty
    /// environment.
    pub(crate) fn function_ref(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<V> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a function reference outside the symbol table");
        };
        if self.layout(self.value_type(result)?.layout)?.kind
            == LayoutKind::Scalar(ScalarKind::Pointer)
        {
            // A pointer scalar result is a C callback thunk's address (unsafe ffi).
            let name = hirsel_lir::emission::callback_thunk_name(
                &symbol.definition,
                symbol.instance.as_ref(),
            );
            self.import_definition(&name, &symbol.definition, None);
            return Ok(V::One(self.function_address(&name, None)?));
        }
        let name = function_thunk_name(&symbol.definition, symbol.instance.as_ref());
        self.import_definition(&name, &symbol.definition, None);
        let code = self.function_address(&name, None)?;
        let environment = self.empty_environment()?;
        let layout = self.value_type(result)?.layout;
        self.callable(layout, code, environment)
    }

    /// The unit's codec `codec`: glue's `hirsel_rt_v2_codec` record of its `CodecKey`,
    /// bound to the program's registry there (`hirsel_lir::glue`).
    pub(crate) fn codec(&mut self, codec: CodecId) -> Emitted<Value> {
        let unit = self.unit;
        let Some(plan) = unit.tables.codecs.get(codec.index()) else {
            return self.invalid("a codec outside the codec table");
        };
        let key = match hirsel_lir::glue::GlueCodec::new(
            plan,
            unit,
            self.em.inputs.layouts,
            self.em.inputs.keys,
        ) {
            Ok(codec) => codec.key,
            Err(error) => return self.invalid(format!("a codec plan off its layouts: {error}")),
        };
        let id = self.em.glue_data(&hirsel_lir::glue::codec_symbol(&key))?;
        Ok(self.data_addr(id, 0))
    }

    /// `Closure`: a counted heap environment holding the captures, or the static empty
    /// environment when it has no bytes.
    pub(crate) fn closure(&mut self, result: ValueId, closure: ClosureId) -> Emitted<V> {
        let unit = self.unit;
        let Some(site) = unit.tables.closures.get(closure.index()) else {
            return self.invalid("a closure outside the closure table");
        };
        if site.receipts.is_some() {
            return self
                .unsupported("receipt closure construction requires physical capture transport");
        }
        // A generic nested function's value has no code of its own: its code word is null.
        let code = match site.code {
            Some(c) => {
                let Some(symbol) = unit.tables.symbols.get(c.symbol.index()) else {
                    return self.invalid("closure code outside the symbol table");
                };
                let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
                let abi = self.em.fn_abi(c.abi, self.definition())?;
                self.import_definition(&name, &symbol.definition, Some(abi));
                let sig = self.em.entry_signature(abi, self.definition())?.ir;
                self.function_address(&name, Some(&sig))?
            }
            None => self.b.ins().iconst(PTR, 0),
        };
        let layout = self.value_type(result)?.layout;
        let size = self.fixed_size(site.environment)?;
        if size == 0 {
            let environment = self.empty_environment()?;
            return self.callable(layout, code, environment);
        }
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        ) {
            return self.unsupported("a closure environment outside the heap");
        }
        let record = self.layout(site.environment)?;
        let align = record.align;
        let extent = self.u64c(size);
        let block = self.alloc_block(site.environment, extent, align, site.anchor)?;
        let payload = self.payload_of(block, site.environment)?;
        self.zero(payload.ptr, size, align);
        let captures = self.pool(site.captures, &unit.pools.captures)?;
        for (i, capture) in captures.iter().enumerate() {
            let Some(field) = record.fields.iter().find(|f| f.ordinal == i as u32) else {
                return self.invalid("a capture without its environment field");
            };
            let at = self.field_place(payload, field.offset, field.layout, field.unaligned)?;
            let shape = self.value_shape(capture.value)?;
            let v = self.get(capture.value)?;
            self.store(shape, v, at)?;
            // A copied uncounted reference holds no credit to duplicate.
            if capture.usage == CaptureUse::Copy
                && self.value_type(capture.value)?.repr != ValueRepr::Uncounted
            {
                self.dup_holders(at)?;
            }
        }
        if let LayoutKind::Environment { movable } = &record.kind {
            let field = record.fields.last().expect("live word");
            let bits = u64::MAX.checked_shr(64 - movable.len() as u32).unwrap_or(0);
            let bits = self.u64c(bits);
            self.store_at(bits, payload.ptr, field.offset);
        }
        self.callable(layout, code, block)
    }

    pub(crate) fn construct_site(&self, construct: ConstructId) -> Emitted<&'a ConstructSite> {
        match self.unit.tables.constructs.get(construct.index()) {
            Some(site) => Ok(site),
            None => self.invalid("a construction outside the table"),
        }
    }

    /// Makes `variant` the active variant of the tagged storage at `place`.
    fn store_tag(&mut self, place: Place, variant: u32) -> Emitted {
        if place.columns.is_some() {
            return self.store_column_tag(place, variant);
        }
        let record = self.layout(place.layout)?;
        let Some(tag) = &record.tag else {
            return self.invalid("a tag write of an untagged layout");
        };
        match tag.form {
            TagForm::Ordinal => {
                let v = self.b.ins().iconst(types::I32, i64::from(variant));
                self.store_at(v, place.ptr, tag.offset);
            }
            // A null-pointer niche: the fieldless variant's word is null; the dataful
            // variant's block reference field fills it.
            TagForm::Niche { nullary, .. } if variant == nullary => {
                let null = self.b.ins().iconst(PTR, 0);
                self.store_at(null, place.ptr, tag.offset);
            }
            TagForm::LiveBit { .. } => return self.invalid("a live word is not a variant tag"),
            TagForm::Niche { .. } => {}
        }
        Ok(())
    }

    pub(crate) fn construct(
        &mut self,
        result: ValueId,
        construct: ConstructId,
        failure: FailureId,
    ) -> Emitted {
        let site = self.construct_site(construct)?;
        let ty = self.value_type(result)?;
        if matches!(
            self.layout(ty.layout)?.kind,
            LayoutKind::Block { .. } | LayoutKind::Box { .. }
        ) {
            return self.construct_block(result, site, None, failure);
        }
        if ty.layout != site.layout || site.constructor == ConstructorKind::Box {
            return self.invalid("an inline construction of another layout");
        }
        let shape = self.shape(ty)?;
        let place = self.stack(site.layout)?;
        if self.layout(site.layout)?.kind == LayoutKind::Flat {
            // A fixed flat record: its fields' bytes, which fill it exactly.
            let parts = self.flat_parts(site)?;
            self.write_flat(place.ptr, &parts)?;
            let value = self.load(shape, place);
            return self.define(result, value);
        }
        let size = self.fixed_size(site.layout)?;
        self.zero(place.ptr, size, place.align);
        self.fill(place, site, &[])?;
        let value = self.load(shape, place);
        self.define(result, value)
    }

    /// The fields a record, tuple or variant construction stores.
    fn constructed_fields(&self, site: &ConstructSite) -> Emitted<&'a [FieldLayout]> {
        let record = self.layout(site.layout)?;
        Ok(match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => &record.fields,
            ConstructorKind::Variant { variant, .. } => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant construction of an untagged layout");
                };
                match tag.variants.iter().find(|v| v.ordinal == variant) {
                    Some(active) => &active.fields,
                    None => return self.invalid("a variant the layout does not have"),
                }
            }
            _ => {
                let what = "fields of a construction that is no record, tuple or variant";
                return self.invalid(what);
            }
        })
    }

    /// The fields a `Reuse` keeps: the set ordinals of `mask` among the construction's.
    fn kept_fields(&self, site: &ConstructSite, mask: MaskId) -> Emitted<Vec<&'a FieldLayout>> {
        let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
            return self.invalid("a mask outside the mask table");
        };
        let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
        let fields = self.constructed_fields(site)?;
        let set = |i: u32| {
            words
                .get(i as usize / 64)
                .is_some_and(|w| w >> (i % 64) & 1 == 1)
        };
        Ok(fields.iter().filter(|f| set(f.ordinal)).collect())
    }

    /// Stores the construction's value of `field` at its offset in `place`.
    fn store_field(&mut self, place: Place, site: &ConstructSite, field: &FieldLayout) -> Emitted {
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
            return self.invalid("a construction without a value for its field");
        };
        let at = self.field_place(place, field.offset, field.layout, field.unaligned)?;
        let shape = self.value_shape(value.value)?;
        let v = self.counted_view(value.value)?;
        self.store(shape, v, at)
    }

    /// Stores a construction's fields, except the ordinals in `skip`, into zeroed storage
    /// of its layout at `place`.
    fn fill(&mut self, place: Place, site: &ConstructSite, skip: &[u32]) -> Emitted {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        if !skip.is_empty()
            && !matches!(
                site.constructor,
                ConstructorKind::Tuple
                    | ConstructorKind::Record(_)
                    | ConstructorKind::Variant { .. }
            )
        {
            return self
                .invalid("kept fields of a construction that is no record, tuple or variant");
        }
        match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => {}
            ConstructorKind::Variant { variant, .. } => self.store_tag(place, variant)?,
            // The boxed value is the whole payload.
            ConstructorKind::Box => {
                let [value] = values else {
                    return self.invalid("a box of other than one value");
                };
                let shape = self.value_shape(value.value)?;
                let v = self.counted_view(value.value)?;
                return self.store(shape, v, place);
            }
            ConstructorKind::Sequence => {
                let (element, start) = match record.kind {
                    LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                        (element, 0)
                    }
                    LayoutKind::Sequential { element } => {
                        // MEM-13: the count, the used bytes, then each element back to back
                        // at its active variant's extent.
                        let count = self.u64c(values.len() as u64);
                        self.store_at(count, place.ptr, 0);
                        let mut ordered: Vec<_> = values.iter().collect();
                        ordered.sort_by_key(|v| v.field);
                        let first = self.sequential_first(place.ptr, element)?;
                        let mut used = self.u64c(0);
                        for value in ordered {
                            let at = self.offset_by(first, used);
                            let bytes = self.store_packed(value.value, element, at)?;
                            used = self.b.ins().iadd(used, bytes);
                        }
                        self.store_at(used, place.ptr, 8);
                        return Ok(());
                    }
                    LayoutKind::SoaColumns { element } => {
                        let count = self.u64c(values.len() as u64);
                        let capacity = self.column_capacity(values.len() as u64, element)?;
                        let capacity = self.u64c(capacity);
                        self.store_at(count, place.ptr, 0);
                        self.store_at(capacity, place.ptr, 8);
                        for value in values {
                            let index = self.u64c(u64::from(value.field));
                            let row = self.column_row(place, element, index)?;
                            let shape = self.value_shape(value.value)?;
                            let v = self.counted_view(value.value)?;
                            self.scatter(row, shape, v)?;
                        }
                        return Ok(());
                    }
                    LayoutKind::Sequence { element } => {
                        // The element count, the capacity (the count), then the elements
                        // from their alignment (runtime ABI 5).
                        let count = self.u64c(values.len() as u64);
                        self.store_at(count, place.ptr, 0);
                        self.store_at(count, place.ptr, SEQUENCE_CAPACITY_OFFSET);
                        match sequence_elements_offset(self.layout(element)?) {
                            Ok(start) => (element, start),
                            Err(_) => return self.invalid("a sequence element offset beyond u64"),
                        }
                    }
                    _ => return self.unsupported("a sequence of this layout"),
                };
                let stride = self.fixed_size(element)?;
                for value in values {
                    let offset = start + u64::from(value.field) * stride;
                    let at = self.field_place(place, offset, element, false)?;
                    let shape = self.value_shape(value.value)?;
                    let v = self.counted_view(value.value)?;
                    self.store(shape, v, at)?;
                }
                return Ok(());
            }
        }
        let fields = self.constructed_fields(site)?;
        for value in values {
            let Some(field) = fields.iter().find(|f| f.ordinal == value.field) else {
                return self.invalid("a constructed field the layout does not have");
            };
            if !skip.contains(&field.ordinal) {
                self.store_field(place, site, field)?;
            }
        }
        Ok(())
    }

    /// `hirsel_rt_v2_alloc` of a heap block of `layout` with `extent` payload bytes. It
    /// cannot fail (runtime ABI 3) and returns the block counted unique.
    pub(crate) fn alloc_block(
        &mut self,
        layout: LayoutId,
        extent: Value,
        align: u32,
        anchor: SourceAnchorId,
    ) -> Emitted<Value> {
        let descriptor = self.descriptor_value(layout)?;
        let instruction = self.instruction_index;
        if self.em.inputs.unit().alternate
            && self
                .em
                .inputs
                .unit()
                .ahead
                .is_some_and(|plan| plan.cpu_allocation(self.unit.header.instance, instruction))
        {
            let Some(state) = self.ahead_state else {
                return self.invalid("CPU allocation without its physical state");
            };
            self.ahead_flush();
            let align = self.u64c(u64::from(align));
            let block = self.rt1(
                RuntimeEntry::CpuAheadAlloc,
                &[state, descriptor, extent, align],
            )?;
            self.ahead_reload();
            return Ok(block);
        }
        let anchor = self.anchor_code(anchor)?;
        let align = self.u64c(u64::from(align));
        let heap = self.b.ins().iconst(types::I8, DomainTag::Heap as i64);
        let anchor = self.u64c(anchor);
        let context = self.context;
        self.rt1(
            RuntimeEntry::Alloc,
            &[context, descriptor, extent, align, heap, anchor],
        )
    }

    /// T03 Dup of the counted block `block`.
    pub(crate) fn dup_block(&mut self, block: Value) -> Emitted {
        let context = self.context;
        self.rt(RuntimeEntry::Dup, &[context, block])?;
        Ok(())
    }

    /// T03 Dup of the `Shared` root `block`: `hirsel_rt_v2_root_dup`, whose CAS keeps the
    /// count exact (runtime ABI 25).
    pub(crate) fn root_dup_block(&mut self, block: Value) -> Emitted {
        let context = self.context;
        self.rt(RuntimeEntry::RootDup, &[context, block])?;
        Ok(())
    }

    /// `hirsel_rt_v2_reuse` of `token` as a heap block of `layout`.
    fn reuse_block(
        &mut self,
        token: Value,
        layout: LayoutId,
        extent: Value,
        align: u32,
        failure: FailureId,
    ) -> Emitted<Value> {
        let descriptor = self.descriptor_value(layout)?;
        let out = self.out_word();
        let align = self.u64c(u64::from(align));
        let context = self.context;
        let status = self.rt1(
            RuntimeEntry::Reuse,
            &[context, token, descriptor, extent, align, out],
        )?;
        self.status_guard(status, failure)?;
        Ok(self.load_ptr(out))
    }

    /// A block at an arena token: a counted heap block when the token is null (`heap`),
    /// else an immortal block the arena allocates.
    #[allow(clippy::too_many_arguments)]
    fn arena_block(
        &mut self,
        token: Value,
        site: &ConstructSite,
        extent: Value,
        align: u32,
        flat: bool,
        failure: FailureId,
    ) -> Emitted<Value> {
        let (on_heap, in_arena, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let block = self.b.append_block_param(done, PTR);
        let heap = self.b.ins().icmp_imm_s(IntCC::Equal, token, 0);
        self.b.ins().brif(heap, on_heap, &[], in_arena, &[]);
        self.b.switch_to_block(on_heap);
        let allocated = self.alloc_block(site.layout, extent, align, site.anchor)?;
        self.b.ins().jump(done, &[allocated.into()]);
        self.b.switch_to_block(in_arena);
        // A bounded arena is charged before the allocation (MEM-6).
        if self.tracking() {
            let bytes = self.construction_charge(site)?;
            let values = self
                .pool(site.fields, &self.unit.pools.fields)?
                .iter()
                .map(|field| field.value)
                .collect::<Vec<_>>();
            self.charge_with_values(token, bytes, site.anchor, failure, values)?;
        }
        // A flat immortal owner's copy may read its capacity rounded to 16, as a region's.
        let capacity = if flat {
            let rounded = self.b.ins().iadd_imm_s(extent, 15);
            self.b.ins().band_imm_s(rounded, !15)
        } else {
            extent
        };
        let descriptor = self.descriptor_value(site.layout)?;
        let align = self.u64c(u64::from(align));
        let context = self.context;
        let flags = if self.tracks(site) {
            hirsel_lir::runtime_abi::ALLOC_TRACKED
        } else {
            0
        };
        let flags = self.b.ins().iconst(types::I32, i64::from(flags));
        let arena = self.rt1(
            RuntimeEntry::ArenaAlloc,
            &[context, token, descriptor, capacity, align, flags],
        )?;
        self.b.ins().jump(done, &[arena.into()]);
        self.b.switch_to_block(done);
        Ok(block)
    }

    /// Charge `bytes` to the arena record `region`: fail when they exceed its budget, else
    /// lower it (MEM-6).
    fn charge_with_values(
        &mut self,
        region: Value,
        bytes: u64,
        anchor: SourceAnchorId,
        failure: FailureId,
        values: Vec<ValueId>,
    ) -> Emitted {
        if !self.tracking() || bytes == 0 {
            return Ok(());
        }
        let offset = hirsel_lir::runtime_abi::REGION_REMAINING_OFFSET;
        let remaining = self.load_ty(types::I64, region, offset);
        let bytes = self.u64c(bytes);
        let over = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, bytes, remaining);
        let exhausted = self.exhausted_block_with_values(region, anchor, failure, values)?;
        self.branch_to(over, exhausted)?;
        let left = self.b.ins().isub(remaining, bytes);
        self.store_at(left, region, offset);
        Ok(())
    }

    /// The charge of a block construction into an arena.
    fn construction_charge(&self, site: &ConstructSite) -> Emitted<u64> {
        let fields = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
        match hirsel_lir::layout::block_charge(self.em.layouts(), site.layout) {
            Some(charge) => Ok(charge.bytes(fields)),
            None => self.unsupported("a bounded-arena charge of a flat payload"),
        }
    }

    /// After an arena block's payload is filled: when the token names an arena and the
    /// payload holds a heap credit, the arena schedules the payload's drop program for its
    /// close (MEM-4). A tracked block (MEM-6) with a drop program is always scheduled:
    /// pushes may move heap credits into it.
    fn arena_hold(
        &mut self,
        token: Value,
        payload: Place,
        layout: LayoutId,
        tracked: bool,
        failure: FailureId,
    ) -> Emitted {
        let record = self.layout(layout)?;
        if record.drop.is_empty() && record.finalizer.is_none() {
            return Ok(());
        }
        let steps = &record.drop;
        let (flag, _) = self.stack_bytes(1, 1);
        let initial = self
            .b
            .ins()
            .iconst(types::I8, i64::from(record.finalizer.is_some()));
        self.store_at(initial, flag, 0);
        let (walk, schedule, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let heap = self.b.ins().icmp_imm_s(IntCC::Equal, token, 0);
        self.b.ins().brif(heap, done, &[], walk, &[]);
        self.b.switch_to_block(walk);
        if tracked {
            let one = self.b.ins().iconst(types::I8, 1);
            self.store_at(one, flag, 0);
        } else {
            self.arena_credits(payload.ptr, payload.align, steps, flag)?;
        }
        let holds = self.load_ty(types::I8, flag, 0);
        self.b.ins().brif(holds, schedule, &[], done, &[]);
        self.b.switch_to_block(schedule);
        let descriptor = self.descriptor_value(layout)?;
        let context = self.context;
        let status = self.rt1(
            RuntimeEntry::RegionSchedule,
            &[context, token, descriptor, payload.ptr],
        )?;
        self.status_guard(status, failure)?;
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }

    /// Set `flag` where the drop program `steps` over the bytes at `base` would release a
    /// counted block that is not immortal, or would do anything but release counted
    /// children.
    fn arena_credits(
        &mut self,
        base: Value,
        align: u32,
        steps: &[DropStep],
        flag: Value,
    ) -> Emitted {
        let one = |this: &mut Self| {
            let v = this.b.ins().iconst(types::I8, 1);
            this.store_at(v, flag, 0);
        };
        for step in steps {
            match step {
                DropStep::Release { offset } => {
                    let child = self.load_ty(PTR, base, *offset);
                    let count = self.load_ty(types::I32, child, COUNT_OFFSET);
                    // Immortal and tracked arena blocks (MEM-6) hold no credit.
                    let counted = if self.tracking() {
                        self.b.ins().icmp_imm_s(
                            IntCC::UnsignedLessThan,
                            count,
                            hirsel_lir::runtime_abi::COUNT_ARENA_SHARED as i64,
                        )
                    } else {
                        self.b
                            .ins()
                            .icmp_imm_s(IntCC::NotEqual, count, COUNT_IMMORTAL as i64)
                    };
                    let old = self.load_ty(types::I8, flag, 0);
                    let new = self.b.ins().bor(old, counted);
                    self.store_at(new, flag, 0);
                }
                DropStep::Child { offset, layout } => {
                    let record = self.layout(*layout)?;
                    if record.finalizer.is_some() {
                        one(self);
                    }
                    let at = self.offset(base, *offset);
                    self.arena_credits(at, align.min(low_bit(*offset)), &record.drop, flag)?;
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let tag = self.load_tag(base, *tag_offset, *form);
                    let tag = self.b.ins().uextend(types::I64, tag);
                    let join = self.b.create_block();
                    let mut switch = Switch::new();
                    let mut arms = Vec::new();
                    for (variant, steps) in variants {
                        if steps.is_empty() {
                            continue;
                        }
                        let arm = self.b.create_block();
                        switch.set_entry(u128::from(*variant), arm);
                        arms.push((arm, steps));
                    }
                    switch.emit(&mut self.b, tag, join);
                    for (arm, steps) in arms {
                        self.b.switch_to_block(arm);
                        self.arena_credits(base, align, steps, flag)?;
                        self.b.ins().jump(join, &[]);
                    }
                    self.b.switch_to_block(join);
                }
                // Finalizers, environments, handles and element loops: schedule.
                _ => one(self),
            }
        }
        Ok(())
    }

    /// Flat content needs an owner a callee can copy: its real immortal header is reserved
    /// once with the regional payload; other regional storage stays headerless.
    pub(crate) fn reserve_region(
        &mut self,
        mark: Value,
        layout: LayoutId,
        extent: Value,
        failure: FailureId,
    ) -> Emitted<Place> {
        let record = self.layout(layout)?;
        let (flat, align) = (record.kind == LayoutKind::Flat, record.align);
        let offset = if flat {
            match align_up(IMMORTAL_PREFIX_BYTES, u64::from(align.max(8))) {
                Ok(n) => n,
                Err(error) => return self.invalid(format!("{error:?}")),
            }
        } else {
            0
        };
        // An immortal owner's copy may read its rounded header capacity.
        let capacity = if flat {
            let rounded = self.b.ins().iadd_imm_s(extent, 15);
            self.b.ins().band_imm_s(rounded, !15)
        } else {
            extent
        };
        let size = self.b.ins().iadd_imm_s(capacity, offset as i64);
        let out = self.out_word();
        let physical = self.u64c(u64::from(if flat { align.max(8) } else { align }));
        let context = self.context;
        let status = self.rt1(
            RuntimeEntry::RegionReserve,
            &[context, mark, size, physical, out],
        )?;
        self.status_guard(status, failure)?;
        let storage = self.load_ptr(out);
        let owner = if flat {
            let header = self.offset(storage, offset - 8);
            let descriptor = self.descriptor_value(layout)?;
            let align = self.u64c(u64::from(align));
            let context = self.context;
            self.rt(
                RuntimeEntry::RegionOwner,
                &[context, mark, header, descriptor, capacity, align],
            )?;
            Some(header)
        } else {
            None
        };
        Ok(Place {
            columns: None,
            ptr: self.offset(storage, offset),
            layout,
            align,
            block: owner,
        })
    }

    /// A block construction: a heap block (allocated, or installed in `token`'s storage),
    /// an arena block, or uncounted storage reserved in a compiler-local region, whose
    /// zeroed payload then receives the fields.
    pub(crate) fn construct_block(
        &mut self,
        result: ValueId,
        site: &ConstructSite,
        reuse: Option<(Value, Option<MaskId>)>,
        failure: FailureId,
    ) -> Emitted {
        let token = reuse.map(|(token, _)| token);
        let repr = self.value_type(result)?.repr;
        let record = self.layout(site.layout)?;
        let align = record.align;
        let flat = record.kind == LayoutKind::Flat && site.constructor != ConstructorKind::Box;
        if flat && reuse.is_some() {
            return self.invalid("a reuse into a flat construction");
        }
        let parts = if flat {
            self.flat_parts(site)?
        } else {
            Vec::new()
        };
        let extent = match (&record.size, record.kind.clone()) {
            (SizeSpec::Fixed(size), _) => *size,
            // A flat record's extent is its parts' bytes.
            (SizeSpec::Dynamic(_), LayoutKind::Flat) if flat => 0,
            (SizeSpec::Dynamic(_), LayoutKind::SoaColumns { element }) => {
                let count = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
                let capacity = self.column_capacity(count, element)?;
                let shape = self.soa(element)?;
                let (data, width) = (shape.data_offset, shape.width);
                match capacity
                    .checked_mul(width)
                    .and_then(|b| b.checked_add(data))
                {
                    Some(extent) => extent,
                    None => return self.invalid("column extent overflow"),
                }
            }
            (SizeSpec::Dynamic(_), LayoutKind::Sequence { element }) => {
                let count = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
                let start = sequence_elements_offset(self.layout(element)?).ok();
                let stride = self.fixed_size(element)?;
                match count
                    .checked_mul(stride)
                    .and_then(|bytes| bytes.checked_add(start?))
                {
                    Some(extent) => extent,
                    None => return self.invalid("a sequence extent beyond u64"),
                }
            }
            // The header; the elements' bytes are known once their tags are read.
            (SizeSpec::Dynamic(_), LayoutKind::Sequential { element }) => {
                self.sequential_offset(element)?
            }
            (SizeSpec::Dynamic(_), _) => {
                return self.unsupported("a dynamic block extent other than a sequence");
            }
        };
        let extent_value = if flat {
            self.flat_extent(&parts)?
        } else if let LayoutKind::Sequential { element } = record.kind {
            let mut bytes = self.u64c(extent);
            for value in self.pool(site.fields, &self.unit.pools.fields)? {
                let temp = self.stack(element)?;
                let shape = self.value_shape(value.value)?;
                let v = self.counted_view(value.value)?;
                self.store(shape, v, temp)?;
                let extent = self.extent(element, temp.ptr)?;
                bytes = self.b.ins().iadd(bytes, extent);
            }
            bytes
        } else {
            self.u64c(extent)
        };
        // The token of an arena or caller destination, whose credits it may hold.
        let mut arena = None;
        let (value, payload) = match (site.placement, repr) {
            (Some(Placement::Fixed(StorageOwner::Heap(_))), ValueRepr::Block) => {
                let block = match token {
                    None => self.alloc_block(site.layout, extent_value, align, site.anchor)?,
                    Some(token) => {
                        self.reuse_block(token, site.layout, extent_value, align, failure)?
                    }
                };
                let payload = self.payload_of(block, site.layout)?;
                (block, payload)
            }
            (Some(Placement::Fixed(StorageOwner::LocalRegion(region))), ValueRepr::Uncounted)
                if token.is_none() =>
            {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                let payload = self.reserve_region(record, site.layout, extent_value, failure)?;
                let storage = payload.ptr;
                if !site.arena_no_drop && !self.layout(site.layout)?.drop.is_empty() {
                    let descriptor = self.descriptor_value(site.layout)?;
                    let context = self.context;
                    let status = self.rt1(
                        RuntimeEntry::RegionSchedule,
                        &[context, record, descriptor, storage],
                    )?;
                    self.status_guard(status, failure)?;
                }
                (storage, payload)
            }
            (Some(Placement::Fixed(StorageOwner::Arena(id))), ValueRepr::Block)
                if token.is_none() =>
            {
                let Some(instance) = self.unit.tables.arenas.get(id.index()) else {
                    return self.invalid("an arena outside the arena table");
                };
                let Some(Some(slot)) = self.slots.get(instance.slot.index()).copied() else {
                    return self.invalid("an arena token slot without storage");
                };
                let token = self.load_ptr(slot.ptr);
                let block = self.arena_block(token, site, extent_value, align, flat, failure)?;
                arena = Some(token);
                (block, self.payload_of(block, site.layout)?)
            }
            (Some(Placement::CallerDestination), ValueRepr::Block) if token.is_none() => {
                let Some(index) = self.sig.hidden_index(Hidden::Destination) else {
                    return self.invalid("a caller destination in a function without one");
                };
                let token = self.entry_params[index];
                let block = self.arena_block(token, site, extent_value, align, flat, failure)?;
                arena = Some(token);
                (block, self.payload_of(block, site.layout)?)
            }
            _ => {
                return self
                    .invalid("a block construction whose placement and representation disagree");
            }
        };
        if flat {
            self.write_flat(payload.ptr, &parts)?;
            return self.define1(result, value);
        }
        // Reused storage keeps the bytes of each kept field: neither zeroed nor stored,
        // unless the token was null and the storage is fresh.
        let kept = match reuse {
            Some((_, Some(mask))) => self.kept_fields(site, mask)?,
            _ => Vec::new(),
        };
        let mut ranges = Vec::new();
        for field in &kept {
            ranges.push((field.offset, field.offset + self.fixed_size(field.layout)?));
        }
        ranges.sort_unstable();
        let mut at = 0;
        for (start, end) in ranges.iter().copied().chain([(extent, extent)]) {
            if start > at {
                let gap = self.offset(payload.ptr, at);
                self.zero(gap, start - at, align.min(low_bit(at)));
            }
            at = at.max(end);
        }
        let skip: Vec<u32> = kept.iter().map(|f| f.ordinal).collect();
        self.fill(payload, site, &skip)?;
        if let Some(token) = arena.filter(|_| !site.arena_no_drop || self.tracks(site)) {
            let tracked = self.tracks(site);
            self.arena_hold(token, payload, site.layout, tracked, failure)?;
        }
        if let (Some(token), false) = (token, kept.is_empty()) {
            let (store, done) = (self.b.create_block(), self.b.create_block());
            let fresh = self.b.ins().icmp_imm_s(IntCC::Equal, token, 0);
            self.b.ins().brif(fresh, store, &[], done, &[]);
            self.b.switch_to_block(store);
            for field in kept {
                self.store_field(payload, site, field)?;
            }
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(done);
        }
        self.define1(result, value)
    }
}
