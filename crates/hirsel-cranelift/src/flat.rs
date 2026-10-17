//! Sequence blocks, FlatFormatV0 encodings and sequential Packed sequences (MEM-13), as
//! `hirsel-llvm`'s `flat` reads and writes them.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::Switch;
use hirsel_lir::emission::slice_storage;
use hirsel_lir::ir::*;
use hirsel_lir::layout::{
    SEQUENCE_CAPACITY_OFFSET, sequence_elements_offset, sequential_elements_offset,
};
use hirsel_lir::layout_types::{FieldLayout, LayoutId, LayoutKind, ScalarKind, SizeSpec, SizeTerm};
use hirsel_lir::runtime_abi::RuntimeEntry;

use crate::function::{FnEmitter, Place, Shape, TRAP, V};
use crate::module::{Emitted, PTR};

/// One field's FlatFormatV0 bytes in a flat construction.
#[derive(Clone, Copy)]
pub(crate) enum FlatPart {
    /// A fixed field's value.
    Fixed { value: ValueId, size: u64 },
    /// An encoded source's `length` bytes at `bytes`, behind a u64 length prefix.
    Encoded { bytes: Value, length: Value },
    /// An ordinary sequence encoded element by element.
    Sequence { source: Place, encoding: LayoutId },
    /// A flat list builder's payload: its count and used bytes are the list's.
    Built { data: Value, first: u64 },
    /// A byte field an open element built in place: its length word and bytes are
    /// already written.
    InPlace { length: Value },
}

impl FnEmitter<'_, '_, '_> {
    /// The payload a block reference layout points at.
    pub(crate) fn layout_payload(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        Ok(match self.layout(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Some(payload),
            _ => None,
        })
    }

    /// The element layout of a counted sequence block layout.
    pub(crate) fn sequence_element(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        let Some(payload) = self.layout_payload(layout)? else {
            return Ok(None);
        };
        Ok(match self.layout(payload)?.kind {
            LayoutKind::Sequence { element } => Some(element),
            _ => None,
        })
    }

    /// The payload address of a sequence block value and its element layout.
    pub(crate) fn sequence_payload(&mut self, value: ValueId) -> Emitted<(Value, LayoutId)> {
        let layout = self.value_type(value)?.layout;
        let (Some(payload), Some(element)) =
            (self.layout_payload(layout)?, self.sequence_element(layout)?)
        else {
            return self.invalid("a sequence operand that is no sequence block");
        };
        let Some(pointer) = self.counted_view(value)?.one() else {
            return self.invalid("a sequence operand that is not a pointer");
        };
        Ok((self.payload_of(pointer, payload)?.ptr, element))
    }

    /// Elements the block's storage holds: its capacity word (runtime ABI 5).
    /// Zero-sized elements report `u64::MAX`.
    pub(crate) fn sequence_capacity(&mut self, value: ValueId) -> Emitted<Value> {
        let (data, element) = self.sequence_payload(value)?;
        if self.fixed_size(element)? == 0 {
            return Ok(self.u64c(u64::MAX));
        }
        Ok(self.load_ty(types::I64, data, SEQUENCE_CAPACITY_OFFSET))
    }

    /// The element of a flat sequence encoding and whether its elements are variable.
    pub(crate) fn flat_sequence(&self, layout: LayoutId) -> Emitted<(LayoutId, bool)> {
        let record = self.layout(layout)?;
        match (&record.kind, &record.size, record.fields.as_slice()) {
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if only.offset == 16 => {
                Ok((only.layout, true))
            }
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only])
                if only.offset == 8
                    && matches!(self.layout(only.layout)?.size, SizeSpec::Fixed(_)) =>
            {
                Ok((only.layout, false))
            }
            _ => self.invalid("a flat part that is no sequence"),
        }
    }

    /// The element count of the flat sequence encoding at `place`.
    pub(crate) fn flat_count(&mut self, place: Place) -> Emitted<Value> {
        let (element, variable) = self.flat_sequence(place.layout)?;
        if variable {
            return Ok(self.load_ty(types::I64, place.ptr, 8));
        }
        match self.fixed_size(element)? {
            0 => self.unsupported("a flat sequence of empty elements"),
            size => {
                let bytes = self.load_u64(place.ptr);
                let size = self.u64c(size);
                Ok(self.b.ins().udiv(bytes, size))
            }
        }
    }

    /// The address of the `index`th prefixed encoding from `first`, each `8 + prefix` bytes.
    pub(crate) fn flat_walk(&mut self, first: Value, index: Value) -> Value {
        let (head, body, exit) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let k = self.b.append_block_param(head, types::I64);
        let cursor = self.b.append_block_param(head, PTR);
        let zero = self.u64c(0);
        self.b.ins().jump(head, &[zero.into(), first.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, k, index);
        self.b.ins().brif(more, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let length = self.load_u64(cursor);
        let past = self.offset(cursor, 8);
        let next = self.offset_by(past, length);
        let k1 = self.b.ins().iadd_imm_s(k, 1);
        self.b.ins().jump(head, &[k1.into(), next.into()]);
        self.b.switch_to_block(exit);
        cursor
    }

    /// The payload offset of a sequential sequence's first element (MEM-13).
    pub(crate) fn sequential_offset(&self, element: LayoutId) -> Emitted<u64> {
        match sequential_elements_offset(self.layout(element)?) {
            Ok(offset) => Ok(offset),
            Err(_) => self.invalid("a sequential element offset beyond u64"),
        }
    }

    pub(crate) fn sequential_first(&mut self, payload: Value, element: LayoutId) -> Emitted<Value> {
        let offset = self.sequential_offset(element)?;
        Ok(self.offset(payload, offset))
    }

    /// The element `index` elements past `first` in a sequential sequence.
    pub(crate) fn sequential_walk(
        &mut self,
        first: Value,
        element: LayoutId,
        index: Value,
    ) -> Emitted<Value> {
        let (head, body, exit) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let k = self.b.append_block_param(head, types::I64);
        let cursor = self.b.append_block_param(head, PTR);
        let zero = self.u64c(0);
        self.b.ins().jump(head, &[zero.into(), first.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, k, index);
        self.b.ins().brif(more, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let bytes = self.extent(element, cursor)?;
        let next = self.offset_by(cursor, bytes);
        let k1 = self.b.ins().iadd_imm_s(k, 1);
        self.b.ins().jump(head, &[k1.into(), next.into()]);
        self.b.switch_to_block(exit);
        Ok(cursor)
    }

    /// Stores the dynamic Packed `value` at `at` within its active extent, which it returns.
    pub(crate) fn store_packed(
        &mut self,
        value: ValueId,
        element: LayoutId,
        at: Value,
    ) -> Emitted<Value> {
        let temp = self.stack(element)?;
        let shape = self.value_shape(value)?;
        let v = self.get(value)?;
        self.store(shape, v, temp)?;
        let bytes = self.extent(element, temp.ptr)?;
        self.copy_dynamic(at, temp.ptr, bytes);
        Ok(bytes)
    }

    /// `load`, except that a dynamic Packed value reads only its active extent.
    pub(crate) fn load_bounded(&mut self, shape: Shape, place: Place) -> Emitted<V> {
        if hirsel_lir::layout::packed_extents(self.layout(place.layout)?).is_none() {
            return Ok(self.load(shape, place));
        }
        let temp = self.stack(place.layout)?;
        let bytes = self.extent(place.layout, place.ptr)?;
        self.copy_dynamic(temp.ptr, place.ptr, bytes);
        Ok(self.load(shape, temp))
    }

    /// The byte extent of the value of `layout` at `at`.
    pub(crate) fn extent(&mut self, layout: LayoutId, at: Value) -> Emitted<Value> {
        match &self.layout(layout)?.size {
            SizeSpec::Fixed(size) => Ok(self.u64c(*size)),
            SizeSpec::Dynamic(program) => self.size_program(program, at),
        }
    }

    pub(crate) fn size_program(&mut self, program: &[SizeTerm], at: Value) -> Emitted<Value> {
        let mut stack: Vec<Value> = Vec::new();
        for term in program {
            let mut pop = |this: &Self| match stack.pop() {
                Some(v) => Ok(v),
                None => this.invalid("a size program that underflows"),
            };
            let value = match term {
                SizeTerm::Constant(n) => self.u64c(*n),
                SizeTerm::ReadLength { offset, scalar } => {
                    let Some((width, false)) = crate::scalar::integer(*scalar) else {
                        return self.invalid("a length that is not an unsigned integer");
                    };
                    if width > 64 {
                        return self.unsupported("a length wider than u64");
                    }
                    let length = self.load_ty(crate::module::scalar_type(*scalar), at, *offset);
                    if width < 64 {
                        self.b.ins().uextend(types::I64, length)
                    } else {
                        length
                    }
                }
                SizeTerm::Add | SizeTerm::Multiply => {
                    let (b, a) = (pop(self)?, pop(self)?);
                    match term {
                        SizeTerm::Add => self.b.ins().iadd(a, b),
                        _ => self.b.ins().imul(a, b),
                    }
                }
                SizeTerm::AlignUp(n) => {
                    if !n.is_power_of_two() {
                        return self.invalid("a size alignment that is not a power of two");
                    }
                    let a = pop(self)?;
                    let bumped = self.b.ins().iadd_imm_s(a, (n - 1) as i64);
                    self.b.ins().band_imm_s(bumped, !(n - 1) as i64)
                }
                SizeTerm::Dup => match stack.last() {
                    Some(&top) => top,
                    None => return self.invalid("a size program that underflows"),
                },
                SizeTerm::At(inner) => {
                    let offset = pop(self)?;
                    let at = self.offset_by(at, offset);
                    self.size_program(inner, at)?
                }
                SizeTerm::ByTag {
                    tag_offset,
                    variants,
                } => {
                    let tag = self.load_ty(types::I32, at, *tag_offset);
                    let tag = self.b.ins().uextend(types::I64, tag);
                    let done = self.b.create_block();
                    let size = self.b.append_block_param(done, types::I64);
                    let bad = self.b.create_block();
                    let mut switch = Switch::new();
                    let mut arms = Vec::new();
                    for (variant, inner) in variants {
                        let arm = self.b.create_block();
                        switch.set_entry(u128::from(*variant), arm);
                        arms.push((arm, inner));
                    }
                    switch.emit(&mut self.b, tag, bad);
                    // A tag without a size is an invariant failure, never a guess.
                    self.b.switch_to_block(bad);
                    self.b.ins().trap(TRAP);
                    for (arm, inner) in arms {
                        self.b.switch_to_block(arm);
                        let size = self.size_program(inner, at)?;
                        self.b.ins().jump(done, &[size.into()]);
                    }
                    self.b.switch_to_block(done);
                    size
                }
            };
            stack.push(value);
        }
        match stack.as_slice() {
            [size] => Ok(*size),
            _ => self.invalid("a size program that leaves no single size"),
        }
    }

    /// The bytes a `Slice { u8 }` value shows and its length.
    pub(crate) fn slice_bytes(&mut self, slice: ValueId) -> Emitted<(Value, Value)> {
        let layout = self.value_type(slice)?.layout;
        let record = self.layout(layout)?;
        let LayoutKind::Slice { element } = record.kind else {
            return self.invalid("an encoded byte source that is not a slice");
        };
        let V::Mem { addr, .. } = self.get(slice)? else {
            return self.invalid("a slice that is not bytes");
        };
        let word = |ordinal: u32| match record.fields.iter().find(|f| f.ordinal == ordinal) {
            Some(field) => Ok(field.offset),
            None => self.invalid("a slice layout without its words"),
        };
        let (owner_at, start_at, length_at) = (word(0)?, word(1)?, word(2)?);
        let owner = self.load_ty(PTR, addr, owner_at);
        let start = self.load_ty(types::I64, addr, start_at);
        let length = self.load_ty(types::I64, addr, length_at);
        let Some(storage) = slice_storage(self.em.layouts(), element) else {
            return self.invalid("a slice without its element's sequence layout");
        };
        let payload = self.payload_of(owner, storage)?;
        let data = self.offset(payload.ptr, 8);
        Ok((self.offset_by(data, start), length))
    }

    /// The place of a sequential sequence block an append writes, its payload and element
    /// layouts.
    pub(crate) fn sequential_place(
        &self,
        address: AddressId,
    ) -> Emitted<(Place, LayoutId, LayoutId)> {
        let place = self.place(address)?;
        let Some(payload) = self.layout_payload(place.layout)? else {
            return self.invalid("an append to a place that holds no block");
        };
        let LayoutKind::Sequential { element } = self.layout(payload)?.kind else {
            return self.invalid("an append to a block that is not sequential");
        };
        Ok((place, payload, element))
    }

    /// Room for `bytes` more past the used bytes of the sequential block at `place`: the
    /// payload address and the used bytes before the append.
    pub(crate) fn sequential_room(
        &mut self,
        place: Place,
        payload: LayoutId,
        first: u64,
        bytes: Value,
    ) -> Emitted<(Value, Value)> {
        let (old, _, used) = self.sequential_used(place, payload)?;
        let keep = self.b.ins().iadd_imm_s(used, first as i64);
        let need = self.b.ins().iadd(keep, bytes);
        let data = self.sequential_grow(place, payload, old, keep, need)?;
        Ok((data, used))
    }

    /// The block the sequential place holds, its payload address and its used bytes.
    pub(crate) fn sequential_used(
        &mut self,
        place: Place,
        payload: LayoutId,
    ) -> Emitted<(Value, Value, Value)> {
        let old = self.load_ptr(place.ptr);
        let data = self.payload_of(old, payload)?.ptr;
        let used = self.load_ty(types::I64, data, 8);
        Ok((old, data, used))
    }

    /// Room for `need` bytes from the payload's start of the sequential block `old` at
    /// `place`, growing it geometrically when it is full and keeping its first `keep`
    /// bytes: the payload address.
    pub(crate) fn sequential_grow(
        &mut self,
        place: Place,
        payload: LayoutId,
        old: Value,
        keep: Value,
        need: Value,
    ) -> Emitted<Value> {
        let align = u64::from(self.layout(payload)?.align);
        let usable = self.rt1(RuntimeEntry::Capacity, &[old])?;
        let full = self.b.ins().icmp(IntCC::UnsignedGreaterThan, need, usable);
        let (grow, done) = (self.b.create_block(), self.b.create_block());
        let block = self.b.append_block_param(done, PTR);
        self.b.ins().brif(full, grow, &[], done, &[old.into()]);
        self.b.switch_to_block(grow);
        let twice = self.b.ins().iadd(usable, usable);
        let more = self.b.ins().icmp(IntCC::UnsignedGreaterThan, need, twice);
        let wanted = self.b.ins().select(more, need, twice);
        let align = self.u64c(align);
        let context = self.context;
        let grown = self.rt1(
            RuntimeEntry::BlockGrow,
            &[context, old, keep, wanted, align],
        )?;
        self.b.ins().jump(done, &[grown.into()]);
        self.b.switch_to_block(done);
        self.store_at(block, place.ptr, 0);
        Ok(self.payload_of(block, payload)?.ptr)
    }

    /// Where the length word of the open element's field `field` lies in a builder's
    /// element of layout `element`.
    pub(crate) fn open_word(&self, element: LayoutId, field: u32) -> Emitted<u64> {
        let record = self.layout(element)?;
        let content = match record.fields.as_slice() {
            [only] if record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
            _ => return self.invalid("an open element of an unprefixed builder"),
        };
        let content = self.layout(content)?;
        let Some(at) = content.fields.iter().find(|f| f.ordinal == field) else {
            return self.invalid("an open element of a missing field");
        };
        for earlier in content.fields.iter().filter(|f| f.ordinal < field) {
            if !matches!(self.layout(earlier.layout)?.size, SizeSpec::Fixed(_)) {
                return self.invalid("an open field after a variable one");
            }
        }
        Ok(8 + at.offset)
    }

    /// One more element of `bytes` in the sequential payload at `data`.
    pub(crate) fn sequential_grew(&mut self, data: Value, used: Value, bytes: Value) {
        let count = self.load_u64(data);
        let count = self.b.ins().iadd_imm_s(count, 1);
        self.store_at(count, data, 0);
        let used = self.b.ins().iadd(used, bytes);
        self.store_at(used, data, 8);
    }

    /// The FlatFormatV0 parts of a flat construction in field order.
    pub(crate) fn flat_parts(&mut self, site: &ConstructSite) -> Emitted<Vec<FlatPart>> {
        self.flat_parts_around(site, None)
    }

    /// `flat_parts` of a construction completing an open element: `open` is the field
    /// built in place and its length.
    pub(crate) fn flat_parts_around(
        &mut self,
        site: &ConstructSite,
        open: Option<(u32, Value)>,
    ) -> Emitted<Vec<FlatPart>> {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let mut declared: Vec<&FieldLayout> = record.fields.iter().collect();
        declared.sort_by_key(|f| f.ordinal);
        if declared.len() != values.len() + usize::from(open.is_some()) {
            return self.invalid("a flat construction without a value for every field");
        }
        let mut parts = Vec::new();
        for field in declared {
            if let Some((ordinal, length)) = open
                && field.ordinal == ordinal
            {
                parts.push(FlatPart::InPlace { length });
                continue;
            }
            let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
                return self.invalid("a flat construction without a value for its field");
            };
            let source = self.value_type(value.value)?.layout;
            let field_record = self.layout(field.layout)?;
            if let SizeSpec::Fixed(size) = field_record.size {
                parts.push(FlatPart::Fixed {
                    value: value.value,
                    size,
                });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
            {
                let pointer = self.get1(value.value)?;
                let data = self.payload_of(pointer, payload)?.ptr;
                let first = self.sequential_offset(element)?;
                parts.push(FlatPart::Built { data, first });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. })
            {
                let pointer = self.get1(value.value)?;
                let source = if self.value_type(value.value)?.repr == ValueRepr::Uncounted {
                    self.uncounted_payload(pointer, payload)?
                } else {
                    self.payload_of(pointer, payload)?
                };
                parts.push(FlatPart::Sequence {
                    source,
                    encoding: field.layout,
                });
                continue;
            }
            let content = match field_record.fields.as_slice() {
                [only] if field_record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
                _ => return self.unsupported("encoding a list into a flat field"),
            };
            let (bytes, length) = match self.layout(source)?.kind {
                LayoutKind::Slice { .. } => self.slice_bytes(value.value)?,
                LayoutKind::Block { payload } | LayoutKind::Box { payload }
                    if payload == content =>
                {
                    let pointer = self.get1(value.value)?;
                    let at = self.payload_of(pointer, payload)?;
                    (at.ptr, self.extent(payload, at.ptr)?)
                }
                _ => return self.invalid("a flat field source that has no encoding"),
            };
            parts.push(FlatPart::Encoded { bytes, length });
        }
        Ok(parts)
    }

    /// The byte extent of `parts` once written.
    pub(crate) fn flat_extent(&mut self, parts: &[FlatPart]) -> Emitted<Value> {
        let mut total = self.u64c(0);
        for part in parts {
            let size = match *part {
                FlatPart::Fixed { size, .. } => self.u64c(size),
                FlatPart::Encoded { length, .. } => {
                    let eight = self.u64c(8);
                    self.flat_add(length, eight)
                }
                FlatPart::Sequence { source, encoding } => {
                    self.flat_encode_sequence(source, encoding, None)?
                }
                FlatPart::Built { data, .. } => {
                    let used = self.load_ty(types::I64, data, 8);
                    let sixteen = self.u64c(16);
                    self.flat_add(used, sixteen)
                }
                FlatPart::InPlace { length } => {
                    let eight = self.u64c(8);
                    self.flat_add(length, eight)
                }
            };
            total = self.flat_add(total, size);
        }
        Ok(total)
    }

    /// Writes `parts` one after another from `at`.
    pub(crate) fn write_flat(&mut self, at: Value, parts: &[FlatPart]) -> Emitted {
        let mut cursor = at;
        for part in parts {
            match *part {
                FlatPart::Fixed { value, size } => {
                    let layout = self.value_type(value)?.layout;
                    let place = Place {
                        ptr: cursor,
                        layout,
                        align: 1,
                        block: None,
                    };
                    let shape = self.value_shape(value)?;
                    let v = self.get(value)?;
                    self.store(shape, v, place)?;
                    cursor = self.offset(cursor, size);
                }
                FlatPart::Encoded { bytes, length } => {
                    self.store_at(length, cursor, 0);
                    let data = self.offset(cursor, 8);
                    self.copy_dynamic(data, bytes, length);
                    cursor = self.offset_by(data, length);
                }
                FlatPart::Sequence { source, encoding } => {
                    let size = self.flat_encode_sequence(source, encoding, Some(cursor))?;
                    cursor = self.offset_by(cursor, size);
                }
                FlatPart::Built { data, first } => {
                    let count = self.load_u64(data);
                    let used = self.load_ty(types::I64, data, 8);
                    let length = self.b.ins().iadd_imm_s(used, 8);
                    self.store_at(length, cursor, 0);
                    self.store_at(count, cursor, 8);
                    let to = self.offset(cursor, 16);
                    let from = self.offset(data, first);
                    self.copy_dynamic(to, from, used);
                    cursor = self.offset_by(to, used);
                }
                FlatPart::InPlace { length } => {
                    let data = self.offset(cursor, 8);
                    cursor = self.offset_by(data, length);
                }
            }
        }
        Ok(())
    }

    /// Size overflow is an implementation invariant fault, before allocating or writing an
    /// extent: it never wraps into an undersized allocation.
    pub(crate) fn flat_add(&mut self, left: Value, right: Value) -> Value {
        let (sum, overflow) = self.b.ins().uadd_overflow(left, right);
        self.trap_if(overflow);
        sum
    }

    /// Size and optionally write an ordinary sequence at its flat encoding.
    pub(crate) fn flat_encode_sequence(
        &mut self,
        source: Place,
        encoding: LayoutId,
        output: Option<Value>,
    ) -> Emitted<Value> {
        let LayoutKind::Sequence { element } = self.layout(source.layout)?.kind else {
            return self.invalid("a flat sequence source without sequence storage");
        };
        let (encoded_element, variable) = self.flat_sequence(encoding)?;
        let count = self.load_u64(source.ptr);
        let stride = self.fixed_size(element)?;
        let start = match sequence_elements_offset(self.layout(element)?) {
            Ok(start) => start,
            Err(e) => return self.invalid(format!("{e:?}")),
        };
        let (head, body, exit) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let index = self.b.append_block_param(head, types::I64);
        let total = self.b.append_block_param(head, types::I64);
        let zero = self.u64c(0);
        let initial = self.u64c(if variable { 16 } else { 8 });
        self.b.ins().jump(head, &[zero.into(), initial.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b.ins().brif(more, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let offset = self.b.ins().imul_imm_s(index, stride as i64);
        let base = self.offset(source.ptr, start);
        let child = Place {
            ptr: self.offset_by(base, offset),
            layout: element,
            align: self.layout(element)?.align,
            block: source.block,
        };
        let at = output.map(|p| self.offset_by(p, total));
        let size = self.flat_encode_element(child, encoded_element, at)?;
        let next_total = self.flat_add(total, size);
        let next_index = self.b.ins().iadd_imm_s(index, 1);
        self.b
            .ins()
            .jump(head, &[next_index.into(), next_total.into()]);
        self.b.switch_to_block(exit);
        if let Some(output) = output {
            let length = self.b.ins().iadd_imm_s(total, -8);
            self.store_at(length, output, 0);
            if variable {
                self.store_at(count, output, 8);
            }
        }
        Ok(total)
    }

    /// Encode one source element at its flat layout.
    pub(crate) fn flat_encode_element(
        &mut self,
        source: Place,
        encoding: LayoutId,
        output: Option<Value>,
    ) -> Emitted<Value> {
        if let SizeSpec::Fixed(size) = self.layout(encoding)?.size {
            if let Some(output) = output {
                self.copy(output, source.ptr, size, 1);
            }
            return Ok(self.u64c(size));
        }
        let [content] = self.layout(encoding)?.fields.as_slice() else {
            return self.invalid("a flat element encoding without its content");
        };
        let (bytes, length) = match self.layout(source.layout)?.kind {
            LayoutKind::Block { payload } => {
                let pointer = self.load_ptr(source.ptr);
                let payload_place = self.payload(pointer, source.layout)?;
                if matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. }) {
                    return self.flat_encode_sequence(payload_place, encoding, output);
                }
                if content.offset != 8 || payload != content.layout {
                    return self.invalid("a flat element source of another payload layout");
                }
                (payload_place.ptr, self.extent(payload, payload_place.ptr)?)
            }
            LayoutKind::Slice { element } => {
                let record = self.layout(source.layout)?;
                let word = |ordinal| {
                    record
                        .fields
                        .iter()
                        .find(|f| f.ordinal == ordinal)
                        .map(|f| f.offset)
                };
                let (Some(owner_at), Some(start_at), Some(length_at)) = (word(0), word(1), word(2))
                else {
                    return self.invalid("a flat slice source without its words");
                };
                let owner = self.load_ty(PTR, source.ptr, owner_at);
                let start = self.load_ty(types::I64, source.ptr, start_at);
                let length = self.load_ty(types::I64, source.ptr, length_at);
                let Some(storage) = slice_storage(self.em.layouts(), element) else {
                    return self.invalid("a flat slice source without byte storage");
                };
                let payload = self.payload_of(owner, storage)?;
                let data = self.offset(payload.ptr, 8);
                (self.offset_by(data, start), length)
            }
            _ => return self.invalid("a flat element source without an encoding"),
        };
        let eight = self.u64c(8);
        let size = self.flat_add(length, eight);
        if let Some(output) = output {
            self.store_at(length, output, 0);
            let data = self.offset(output, 8);
            self.copy_dynamic(data, bytes, length);
        }
        Ok(size)
    }

    /// A pointer-sized scalar shape, for loads of block references.
    #[allow(dead_code)]
    pub(crate) fn pointer_shape() -> Shape {
        Shape::Scalar(ScalarKind::Pointer)
    }
}
