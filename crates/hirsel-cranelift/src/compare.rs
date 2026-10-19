//! Structural comparison and hashing (`Inst::Compare`, `Inst::Hash`): one private
//! comparator or hash function per layout, defined once per object, as `hirsel-llvm`'s
//! `compare` generates them.
//!
//! A comparator `hirsel.compare.<layout>[.eq](left, right, depth, context) -> i8` reads two
//! values of its layout at the given addresses and returns one of the `ORDER_*` codes: the
//! interpreter's order, the first unequal pair deciding, variant ordinals before payloads,
//! sequences element by element then by length, String and Bytes bytewise then by length,
//! floats by IEEE with an unordered pair deciding at once. A pair deeper than
//! `COMPARE_MAX_DEPTH` is the MEM-3 Depth failure.
//!
//! A hash function `hirsel.hash.<layout>(at, state, depth, deep) -> i64` feeds the scalar
//! leaves of its value to `hirsel_rt_v2_hash_update` in layout order and returns the
//! caller-owned stream. Only the root finalizes it. Floats feed `-0.0` as `+0.0`,
//! a variant its ordinal, a sequence its count first.

use cranelift_codegen::Context;
use cranelift_codegen::ir::{
    self, AbiParam, Block, InstBuilder, StackSlotData, StackSlotKind, Value,
    condcodes::{FloatCC, IntCC},
    types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_module::{FuncId, Module};
use hirsel_lir::emission::slice_storage;
use hirsel_lir::ir::{COMPARE_MAX_DEPTH, CompareOp, FailureId, ValueId};
use hirsel_lir::layout::{payload_offset, sequence_elements_offset, slice_data_offset};
use hirsel_lir::layout_types::{
    FieldLayout, LayoutId, LayoutKind, LayoutRecord, ScalarKind, ScanStep, SizeSpec, TagForm,
};
use hirsel_lir::runtime_abi::RuntimeEntry;
use hirsel_lir::stage::ObjectSymbolKind;
use hirsel_syntax::DefPath;

use crate::CodegenError;
use crate::function::{FnEmitter, V, flags};
use crate::module::{Emitted, Emitter, Helper, PTR};

/// The pair is equal.
pub(crate) const ORDER_EQUAL: u8 = 0;
/// The left value is less.
pub(crate) const ORDER_LESS: u8 = 1;
/// The left value is greater.
pub(crate) const ORDER_GREATER: u8 = 2;
/// A float pair is unordered: every operator but `Ne` is false.
pub(crate) const ORDER_UNORDERED: u8 = 3;
/// A pair deeper than `COMPARE_MAX_DEPTH`.
pub(crate) const ORDER_DEPTH: u8 = 4;

type Generated<T = ()> = Result<T, CodegenError>;

impl Emitter<'_> {
    fn comparator_signature(&self) -> ir::Signature {
        let mut sig = ir::Signature::new(self.call_conv);
        sig.params.extend([
            AbiParam::new(PTR),
            AbiParam::new(PTR),
            AbiParam::new(types::I64),
            AbiParam::new(PTR),
        ]);
        sig.returns.push(AbiParam::new(types::I8).uext());
        sig
    }

    fn hasher_signature(&self) -> ir::Signature {
        let mut sig = ir::Signature::new(self.call_conv);
        sig.params.extend([
            AbiParam::new(PTR),
            AbiParam::new(types::I64),
            AbiParam::new(types::I64),
            AbiParam::new(PTR),
            AbiParam::new(PTR),
        ]);
        sig.returns.push(AbiParam::new(types::I64));
        sig
    }

    /// The comparator of `layout`, declared if new.
    fn declare_comparator(&mut self, layout: LayoutId, equality: bool) -> Emitted<FuncId> {
        let name = format!(
            "hirsel.compare.{}{}",
            layout.0,
            if equality { ".eq" } else { "" }
        );
        let sig = self.comparator_signature();
        self.helper(&name, &sig, |_| Helper::Compare { layout, equality })
    }

    fn declare_hasher(&mut self, layout: LayoutId) -> Emitted<FuncId> {
        let name = format!("hirsel.hash.{}", layout.0);
        let sig = self.hasher_signature();
        self.helper(&name, &sig, |_| Helper::Hash { layout })
    }

    /// The comparator of `layout` and every function it calls, defined now so a layout it
    /// cannot compare is `definition`'s error.
    pub(crate) fn comparator(
        &mut self,
        layout: LayoutId,
        equality: bool,
        definition: &DefPath,
    ) -> Emitted<FuncId> {
        let id = self.declare_comparator(layout, equality)?;
        self.drain_helpers(definition)?;
        Ok(id)
    }

    pub(crate) fn hasher(&mut self, layout: LayoutId, definition: &DefPath) -> Emitted<FuncId> {
        let id = self.declare_hasher(layout)?;
        self.drain_helpers(definition)?;
        Ok(id)
    }

    /// Defines every queued helper, attributing a generation error to `definition`.
    pub(crate) fn drain_helpers(&mut self, definition: &DefPath) -> Emitted {
        let mut context = Context::new();
        let mut builder_context = FunctionBuilderContext::new();
        while let Some((id, helper)) = self.pending.pop() {
            self.define_helper(id, helper, &mut context, &mut builder_context, definition)?;
        }
        Ok(())
    }
}

/// Generates one comparator or hash function body.
struct Gen<'x, 'a> {
    em: &'x mut Emitter<'a>,
    b: FunctionBuilder<'x>,
    definition: &'x DefPath,
    equality: bool,
    params: Vec<Value>,
    /// A 16-byte slot that feeds a computed scalar to the hash.
    scratch: Option<Value>,
}

impl Gen<'_, '_> {
    fn unsupported<T>(&self, what: impl Into<String>) -> Generated<T> {
        Err(CodegenError::Unsupported {
            definition: self.definition.clone(),
            what: what.into(),
        })
    }

    fn invalid<T>(&self, what: impl Into<String>) -> Generated<T> {
        Err(CodegenError::Invalid {
            definition: self.definition.clone(),
            what: what.into(),
        })
    }

    fn record(&self, layout: LayoutId) -> Generated<&LayoutRecord> {
        match self.em.inputs.layouts.layout(layout) {
            Some(record) => Ok(record),
            None => self.invalid(format!("no layout {}", layout.0)),
        }
    }

    fn map_next(
        &mut self,
        tag: &hirsel_lir::layout_types::TagEncoding,
        stride: u64,
        first: Value,
    ) -> Value {
        let head = self.b.create_block();
        let skip = self.b.create_block();
        let done = self.b.create_block();
        let at = self.b.append_block_param(head, PTR);
        self.b.ins().jump(head, &[first.into()]);
        self.b.switch_to_block(head);
        let ordinal = self.tag(at, tag.offset, tag.form);
        let live = self.b.ins().icmp_imm_s(IntCC::Equal, ordinal, 1);
        self.b.ins().brif(live, done, &[], skip, &[]);
        self.b.switch_to_block(skip);
        let next = self.at(at, stride);
        self.b.ins().jump(head, &[next.into()]);
        self.b.switch_to_block(done);
        at
    }

    fn size(&self, layout: LayoutId) -> Generated<u64> {
        match self.record(layout)?.size {
            SizeSpec::Fixed(size) => Ok(size),
            SizeSpec::Dynamic(_) => {
                self.unsupported("a comparison or hash of a value of dynamic extent")
            }
        }
    }

    fn u64(&mut self, n: u64) -> Value {
        self.b.ins().iconst(types::I64, n as i64)
    }

    fn code(&mut self, code: u8) -> Value {
        self.b.ins().iconst(types::I8, i64::from(code))
    }

    fn at(&mut self, base: Value, offset: u64) -> Value {
        if offset == 0 {
            base
        } else {
            self.b.ins().iadd_imm_s(base, offset as i64)
        }
    }

    fn load(&mut self, ty: ir::Type, at: Value) -> Value {
        self.b.ins().load(ty, flags(), at, 0)
    }

    fn tag(&mut self, base: Value, offset: u64, form: TagForm) -> Value {
        match form {
            TagForm::Ordinal => self.b.ins().load(types::I32, flags(), base, offset as i32),
            TagForm::LiveBit { bit } => {
                let word = self.b.ins().load(types::I64, flags(), base, offset as i32);
                let set = self.b.ins().band_imm_s(word, (1u64 << bit) as i64);
                let live = self.b.ins().icmp_imm_s(IntCC::NotEqual, set, 0);
                self.b.ins().uextend(types::I32, live)
            }
            TagForm::Niche { nullary, dataful } => {
                let word = self.b.ins().load(PTR, flags(), base, offset as i32);
                let null = self.b.ins().icmp_imm_s(IntCC::Equal, word, 0);
                let nullary = self.b.ins().iconst(types::I32, i64::from(nullary));
                let dataful = self.b.ins().iconst(types::I32, i64::from(dataful));
                self.b.ins().select(null, nullary, dataful)
            }
        }
    }

    /// Returns `value` from the function when `taken` holds, then continues.
    fn return_if(&mut self, taken: Value, value: Value) {
        let (done, next) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(taken, done, &[], next, &[]);
        self.b.switch_to_block(done);
        self.b.ins().return_(&[value]);
        self.b.switch_to_block(next);
    }

    /// Returns `code` from the comparator unless it is `ORDER_EQUAL`, then continues.
    fn decide(&mut self, code: Value) {
        let unequal = self
            .b
            .ins()
            .icmp_imm_s(IntCC::NotEqual, code, i64::from(ORDER_EQUAL));
        self.return_if(unequal, code);
    }

    /// `ORDER_LESS` when `less`, `ORDER_GREATER` when `greater`, else `ORDER_EQUAL`.
    fn order(&mut self, less: Value, greater: Value) -> Value {
        let (g, e, l) = (
            self.code(ORDER_GREATER),
            self.code(ORDER_EQUAL),
            self.code(ORDER_LESS),
        );
        let other = self.b.ins().select(greater, g, e);
        self.b.ins().select(less, l, other)
    }

    /// The comparator's pair at `depth` is too deep: return the Depth code.
    fn check_depth(&mut self, depth: Value) {
        let deep =
            self.b
                .ins()
                .icmp_imm_s(IntCC::UnsignedGreaterThan, depth, COMPARE_MAX_DEPTH as i64);
        let code = self.code(ORDER_DEPTH);
        self.return_if(deep, code);
    }

    fn call(&mut self, id: FuncId, args: &[Value]) -> Value {
        let r = self.em.module.declare_func_in_func(id, self.b.func);
        let call = self.b.ins().call(r, args);
        self.b.inst_results(call)[0]
    }

    /// The payload a block reference of `record` points at.
    fn payload(
        &mut self,
        pointer: Value,
        record: &LayoutRecord,
        payload: LayoutId,
    ) -> Generated<Value> {
        let uncounted = record
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if uncounted {
            return Ok(pointer);
        }
        let offset = match payload_offset(self.record(payload)?.align) {
            Ok(offset) => offset,
            Err(e) => return self.invalid(format!("{e:?}")),
        };
        Ok(self.at(pointer, offset))
    }

    fn element_offset(&self, element: LayoutId) -> Generated<u64> {
        match sequence_elements_offset(self.record(element)?) {
            Ok(offset) => Ok(offset),
            Err(e) => self.invalid(format!("{e:?}")),
        }
    }

    /// The data address and length of the slice at `at`.
    fn slice(
        &mut self,
        record: &LayoutRecord,
        element: LayoutId,
        at: Value,
    ) -> Generated<(Value, Value)> {
        let field = |ordinal: u32| -> Option<&FieldLayout> {
            record.fields.iter().find(|f| f.ordinal == ordinal)
        };
        let (Some(owner), Some(start), Some(length)) = (field(0), field(1), field(2)) else {
            return self.invalid("a slice without its fields");
        };
        let Some(payload) = slice_storage(self.em.inputs.layouts, element) else {
            return self.invalid("a slice without its element's sequence layout");
        };
        let owner_at = self.at(at, owner.offset);
        let block = self.load(PTR, owner_at);
        let offset = match payload_offset(self.record(payload)?.align) {
            Ok(offset) => offset,
            Err(e) => return self.invalid(format!("{e:?}")),
        };
        let base = match slice_data_offset(self.record(element)?) {
            Ok(base) => base,
            Err(e) => return self.invalid(format!("{e:?}")),
        };
        let data = self.at(block, offset + base);
        let start_at = self.at(at, start.offset);
        let first = self.load(types::I64, start_at);
        let data = self.b.ins().iadd(data, first);
        let length_at = self.at(at, length.offset);
        let length = self.load(types::I64, length_at);
        Ok((data, length))
    }

    // Comparators.

    fn define_comparator(mut self, layout: LayoutId) -> Generated {
        let (left, right, depth) = (self.params[0], self.params[1], self.params[2]);
        self.check_depth(depth);
        let record = self.record(layout)?.clone();
        let deeper = self.b.ins().iadd_imm_s(depth, 1);
        match &record.kind {
            LayoutKind::Path => {
                let code = if self.equality {
                    self.scalar(ScalarKind::U64, left, right)?
                } else {
                    let mut sig = ir::Signature::new(self.em.call_conv);
                    sig.params.extend([
                        AbiParam::new(PTR),
                        AbiParam::new(types::I64),
                        AbiParam::new(types::I64),
                    ]);
                    sig.returns.push(AbiParam::new(types::I32));
                    let name = "hirsel_rt_v2_path_compare";
                    self.em
                        .manifested
                        .import(name, ObjectSymbolKind::Runtime, None);
                    let id = self.em.import_function(name, &sig)?;
                    let l = self.load(types::I64, left);
                    let r = self.load(types::I64, right);
                    let context = *self.params.last().unwrap();
                    let result = self.call(id, &[context, l, r]);
                    let less = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, result, 0);
                    let greater = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, result, 0);
                    self.order(less, greater)
                };
                self.decide(code);
            }
            LayoutKind::Unit => {}
            LayoutKind::Scalar(kind) => {
                let code = self.scalar(*kind, left, right)?;
                self.decide(code);
            }
            LayoutKind::Aggregate => self.fields(&record.fields, left, right, deeper)?,
            LayoutKind::Variant => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant layout without a tag");
                };
                let l = self.tag(left, tag.offset, tag.form);
                let r = self.tag(right, tag.offset, tag.form);
                let less = self.b.ins().icmp(IntCC::UnsignedLessThan, l, r);
                let greater = self.b.ins().icmp(IntCC::UnsignedGreaterThan, l, r);
                let code = self.order(less, greater);
                self.decide(code);
                let done = self.b.create_block();
                let mut switch = Switch::new();
                let mut arms: Vec<(Block, &[FieldLayout])> = Vec::new();
                for variant in &tag.variants {
                    let arm = self.b.create_block();
                    switch.set_entry(u128::from(variant.ordinal), arm);
                    arms.push((arm, &variant.fields));
                }
                let on = self.b.ins().uextend(types::I64, l);
                switch.emit(&mut self.b, on, done);
                for (arm, fields) in arms {
                    self.b.switch_to_block(arm);
                    self.fields(fields, left, right, deeper)?;
                    self.b.ins().jump(done, &[]);
                }
                self.b.switch_to_block(done);
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(*element)?;
                let count = self.u64(*count);
                self.elements(*element, stride, left, right, count, deeper)?;
            }
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let l = self.load(PTR, left);
                let r = self.load(PTR, right);
                let (l, r) = (
                    self.payload(l, &record, *payload)?,
                    self.payload(r, &record, *payload)?,
                );
                let element = match self.record(*payload)?.kind {
                    LayoutKind::Sequence { element } => Some(element),
                    _ => None,
                };
                match element {
                    // A List or Array: its elements one level deeper, then the lengths.
                    Some(element) => {
                        let start = self.element_offset(element)?;
                        let stride = self.size(element)?;
                        let (lc, rc) = (self.load(types::I64, l), self.load(types::I64, r));
                        let fewer = self.b.ins().icmp(IntCC::UnsignedLessThan, lc, rc);
                        let count = self.b.ins().select(fewer, lc, rc);
                        let (le, re) = (self.at(l, start), self.at(r, start));
                        self.elements(element, stride, le, re, count, deeper)?;
                        let less = self.b.ins().icmp(IntCC::UnsignedLessThan, lc, rc);
                        let greater = self.b.ins().icmp(IntCC::UnsignedGreaterThan, lc, rc);
                        let code = self.order(less, greater);
                        self.decide(code);
                    }
                    None if matches!(self.record(*payload)?.kind, LayoutKind::Opaque { .. }) => {
                        self.container(*payload, l, r, deeper)?;
                    }
                    // A Box: its payload one level deeper.
                    None => {
                        let code = self.child(*payload, l, r, deeper)?;
                        self.decide(code);
                    }
                }
            }
            LayoutKind::Slice { element } => {
                // String and Bytes: bytewise, then by length, with no depth below the value.
                if self.size(*element)? != 1 {
                    return self.unsupported("a slice of elements wider than a byte");
                }
                let (lp, ll) = self.slice(&record, *element, left)?;
                let (rp, rl) = self.slice(&record, *element, right)?;
                let less = self.b.ins().icmp(IntCC::UnsignedLessThan, ll, rl);
                let greater = self.b.ins().icmp(IntCC::UnsignedGreaterThan, ll, rl);
                let lengths = self.order(less, greater);
                if self.equality {
                    // Unequal lengths decide equality before any bytes.
                    self.decide(lengths);
                }
                let count = if self.equality {
                    ll
                } else {
                    self.b.ins().select(less, ll, rl)
                };
                self.bytes(lp, rp, count)?;
                if !self.equality {
                    self.decide(lengths);
                }
            }
            LayoutKind::Opaque { .. } => self.container(layout, left, right, deeper)?,
            LayoutKind::Packed
            | LayoutKind::Simd { .. }
            | LayoutKind::Flat
            | LayoutKind::SoaColumns { .. }
            | LayoutKind::Sequence { .. }
            | LayoutKind::Sequential { .. } => {
                return self.unsupported(format!("a structural comparison of {:?}", record.kind));
            }
            LayoutKind::Environment { .. }
            | LayoutKind::Shared { .. }
            | LayoutKind::Frozen { .. }
            | LayoutKind::Handle
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header => {
                return self.invalid(format!("a structural comparison of {:?}", record.kind));
            }
        }
        let equal = self.code(ORDER_EQUAL);
        self.b.ins().return_(&[equal]);
        self.finish();
        Ok(())
    }

    fn finish(mut self) {
        self.b.seal_all_blocks();
        let config = self.em.module.target_config();
        self.b.finalize(config);
    }

    fn container(
        &mut self,
        layout: LayoutId,
        left: Value,
        right: Value,
        depth: Value,
    ) -> Generated {
        let record = self.record(layout)?.clone();
        let LayoutKind::Opaque { owner } = &record.kind else {
            return self.invalid("container payload");
        };
        if !owner.0.package.name_is("core") || !matches!(owner.0.name.as_str(), "Map" | "Set") {
            return self.unsupported(format!("comparison of opaque {owner:?}"));
        }
        let field = &record.fields[0];
        let entries = self.record(field.layout)?.clone();
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = entries.kind else {
            return self.invalid("container entries");
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return self.invalid("container sequence");
        };
        let (left_at, right_at) = (self.at(left, field.offset), self.at(right, field.offset));
        let l = self.load(PTR, left_at);
        let r = self.load(PTR, right_at);
        let (l, r) = (
            self.payload(l, &entries, payload)?,
            self.payload(r, &entries, payload)?,
        );
        let (lc, rc) = if owner.0.name == "Map" {
            let la = self.at(left, record.fields[3].offset);
            let ra = self.at(right, record.fields[3].offset);
            (self.load(types::I64, la), self.load(types::I64, ra))
        } else {
            (self.load(types::I64, l), self.load(types::I64, r))
        };
        let fewer = self.b.ins().icmp(IntCC::UnsignedLessThan, lc, rc);
        let count = self.b.ins().select(fewer, lc, rc);
        let start = self.element_offset(element)?;
        let stride = self.size(element)?;
        let (l, r) = (self.at(l, start), self.at(r, start));
        if owner.0.name == "Map" {
            let tag = self
                .record(element)?
                .tag
                .clone()
                .ok_or_else(|| CodegenError::Invalid {
                    definition: self.definition.clone(),
                    what: "map entry tag".into(),
                })?;
            let pair = tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0].clone();
            let head = self.b.create_block();
            let compare = self.b.create_block();
            let done = self.b.create_block();
            let i = self.b.append_block_param(head, types::I64);
            let lp = self.b.append_block_param(head, PTR);
            let rp = self.b.append_block_param(head, PTR);
            let zero = self.u64(0);
            self.b.ins().jump(head, &[zero.into(), l.into(), r.into()]);
            self.b.switch_to_block(head);
            let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
            self.b.ins().brif(more, compare, &[], done, &[]);
            self.b.switch_to_block(compare);
            let ln = self.map_next(&tag, stride, lp);
            let rn = self.map_next(&tag, stride, rp);
            let la = self.at(ln, pair.offset);
            let ra = self.at(rn, pair.offset);
            let code = self.child(pair.layout, la, ra, depth)?;
            self.decide(code);
            let next = self.b.ins().iadd_imm_s(i, 1);
            let ln = self.at(ln, stride);
            let rn = self.at(rn, stride);
            self.b
                .ins()
                .jump(head, &[next.into(), ln.into(), rn.into()]);
            self.b.switch_to_block(done);
        } else {
            // SEM-5: selection yields canonical structural order without allocating or
            // mutating either borrowed set.
            let (head, compare, done) = (
                self.b.create_block(),
                self.b.create_block(),
                self.b.create_block(),
            );
            let i = self.b.append_block_param(head, types::I64);
            let lp = self.b.append_block_param(head, PTR);
            let rp = self.b.append_block_param(head, PTR);
            let zero = self.u64(0);
            self.b.ins().jump(head, &[zero.into(), l.into(), r.into()]);
            self.b.switch_to_block(head);
            let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
            self.b.ins().brif(more, compare, &[], done, &[]);
            self.b.switch_to_block(compare);
            let first = self.b.ins().icmp_imm_s(IntCC::Equal, i, 0);
            let ln = self.sorted_next(element, stride, l, lc, lp, first, depth, None)?;
            let rn = self.sorted_next(element, stride, r, rc, rp, first, depth, None)?;
            let code = self.child(element, ln, rn, depth)?;
            self.decide(code);
            let next = self.b.ins().iadd_imm_s(i, 1);
            self.b
                .ins()
                .jump(head, &[next.into(), ln.into(), rn.into()]);
            self.b.switch_to_block(done);
        }
        let less = self.b.ins().icmp(IntCC::UnsignedLessThan, lc, rc);
        let greater = self.b.ins().icmp(IntCC::UnsignedGreaterThan, lc, rc);
        let code = self.order(less, greater);
        self.decide(code);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn sorted_next(
        &mut self,
        element: LayoutId,
        stride: u64,
        base: Value,
        count: Value,
        previous: Value,
        first: Value,
        depth: Value,
        hash: Option<(Value, Value)>,
    ) -> Generated<Value> {
        let (head, check, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let i = self.b.append_block_param(head, types::I64);
        let best = self.b.append_block_param(head, PTR);
        let found = self.b.append_block_param(head, types::I8);
        let zero = self.u64(0);
        let none = self.b.ins().iconst(types::I8, 0);
        self.b
            .ins()
            .jump(head, &[zero.into(), base.into(), none.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
        self.b.ins().brif(more, check, &[], done, &[]);
        self.b.switch_to_block(check);
        let offset = self.b.ins().imul_imm_s(i, stride as i64);
        let candidate = self.b.ins().iadd(base, offset);
        let equality = self.equality;
        self.equality = false;
        let after = if hash.is_some() {
            let callee = self.em.declare_comparator(element, false)?;
            let context = *self.params.last().unwrap();
            self.call(callee, &[candidate, previous, depth, context])
        } else {
            self.child(element, candidate, previous, depth)?
        };
        self.selection_depth(after, hash);
        let greater = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, after, i64::from(ORDER_GREATER));
        let eligible = self.b.ins().bor(first, greater);
        let smaller = if hash.is_some() {
            let callee = self.em.declare_comparator(element, false)?;
            let context = *self.params.last().unwrap();
            self.call(callee, &[candidate, best, depth, context])
        } else {
            self.child(element, candidate, best, depth)?
        };
        self.equality = equality;
        self.selection_depth(smaller, hash);
        let less = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, smaller, i64::from(ORDER_LESS));
        let not_found = self.b.ins().icmp_imm_s(IntCC::Equal, found, 0);
        let better = self.b.ins().bor(not_found, less);
        let choose = self.b.ins().band(eligible, better);
        let next_best = self.b.ins().select(choose, candidate, best);
        let next_found = self.b.ins().bor(found, eligible);
        let next_i = self.b.ins().iadd_imm_s(i, 1);
        self.b
            .ins()
            .jump(head, &[next_i.into(), next_best.into(), next_found.into()]);
        self.b.switch_to_block(done);
        Ok(best)
    }

    fn selection_depth(&mut self, code: Value, hash: Option<(Value, Value)>) {
        if let Some((state, deep)) = hash {
            let too = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, code, i64::from(ORDER_DEPTH));
            let failed = self.b.create_block();
            let next = self.b.create_block();
            self.b.ins().brif(too, failed, &[], next, &[]);
            self.b.switch_to_block(failed);
            let one = self.b.ins().iconst(types::I8, 1);
            self.b.ins().store(flags(), one, deep, 0);
            self.b.ins().return_(&[state]);
            self.b.switch_to_block(next);
        } else {
            self.depth_code(code);
        }
    }
    fn depth_code(&mut self, code: Value) {
        let deep = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, code, i64::from(ORDER_DEPTH));
        self.return_if(deep, code);
    }

    /// Compare the byte intervals in unsigned-byte order through `memcmp`; empty intervals
    /// call nothing.
    fn bytes(&mut self, left: Value, right: Value, count: Value) -> Generated {
        let (work, done) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(count, work, &[], done, &[]);
        self.b.switch_to_block(work);
        let mut sig = ir::Signature::new(self.em.call_conv);
        sig.params.extend([
            AbiParam::new(PTR),
            AbiParam::new(PTR),
            AbiParam::new(types::I64),
        ]);
        sig.returns.push(AbiParam::new(types::I32));
        let id = self.em.import_function("memcmp", &sig)?;
        let compared = self.call(id, &[left, right, count]);
        let less = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, compared, 0);
        let greater = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThan, compared, 0);
        let code = self.order(less, greater);
        self.decide(code);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }

    /// A loop over `0..count`, running `body` with the index.
    fn each(
        &mut self,
        count: Value,
        mut body: impl FnMut(&mut Self, Value) -> Generated,
    ) -> Generated {
        let (head, work, after) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let index = self.b.append_block_param(head, types::I64);
        let zero = self.u64(0);
        self.b.ins().jump(head, &[zero.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b.ins().brif(more, work, &[], after, &[]);
        self.b.switch_to_block(work);
        body(self, index)?;
        let next = self.b.ins().iadd_imm_s(index, 1);
        self.b.ins().jump(head, &[next.into()]);
        self.b.switch_to_block(after);
        Ok(())
    }

    /// Compares `count` elements of `element`, `stride` apart, each at `depth`.
    fn elements(
        &mut self,
        element: LayoutId,
        stride: u64,
        left: Value,
        right: Value,
        count: Value,
        depth: Value,
    ) -> Generated {
        let callee = self.callee(element)?;
        self.each(count, |g, i| {
            let offset = g.b.ins().imul_imm_s(i, stride as i64);
            let l = g.b.ins().iadd(left, offset);
            let r = g.b.ins().iadd(right, offset);
            let code = g.compare(element, callee, l, r, depth)?;
            g.decide(code);
            Ok(())
        })
    }

    /// Compares each field in ordinal order at `depth`.
    fn fields(
        &mut self,
        fields: &[FieldLayout],
        left: Value,
        right: Value,
        depth: Value,
    ) -> Generated {
        let mut fields: Vec<&FieldLayout> = fields.iter().collect();
        fields.sort_by_key(|f| f.ordinal);
        for field in fields {
            let (l, r) = (self.at(left, field.offset), self.at(right, field.offset));
            let code = self.child(field.layout, l, r, depth)?;
            self.decide(code);
        }
        Ok(())
    }

    /// How a child of `layout` is compared: inline for a scalar leaf, else its comparator.
    fn callee(&mut self, layout: LayoutId) -> Generated<Option<FuncId>> {
        Ok(match self.record(layout)?.kind {
            LayoutKind::Scalar(_) => None,
            _ => Some(self.em.declare_comparator(layout, self.equality)?),
        })
    }

    fn child(
        &mut self,
        layout: LayoutId,
        left: Value,
        right: Value,
        depth: Value,
    ) -> Generated<Value> {
        let callee = self.callee(layout)?;
        self.compare(layout, callee, left, right, depth)
    }

    /// The code of the pair at `depth`: a scalar leaf inline after its depth check, any
    /// other layout through its comparator.
    fn compare(
        &mut self,
        layout: LayoutId,
        callee: Option<FuncId>,
        left: Value,
        right: Value,
        depth: Value,
    ) -> Generated<Value> {
        match callee {
            Some(id) => {
                let context = *self.params.last().unwrap();
                Ok(self.call(id, &[left, right, depth, context]))
            }
            None => {
                let LayoutKind::Scalar(kind) = self.record(layout)?.kind else {
                    return self.invalid("an inline leaf that is not a scalar");
                };
                self.check_depth(depth);
                self.scalar(kind, left, right)
            }
        }
    }

    /// The order of two scalars of `kind` in memory.
    fn scalar(&mut self, kind: ScalarKind, left: Value, right: Value) -> Generated<Value> {
        use ScalarKind as K;
        let (ty, signed) = match kind {
            K::Bool | K::U8 => (types::I8, false),
            K::U16 => (types::I16, false),
            K::U32 => (types::I32, false),
            K::U64 => (types::I64, false),
            K::U128 => (types::I128, false),
            K::I8 => (types::I8, true),
            K::I16 => (types::I16, true),
            K::I32 => (types::I32, true),
            K::I64 => (types::I64, true),
            K::I128 => (types::I128, true),
            K::F32 | K::F64 => {
                let ty = if kind == K::F32 {
                    types::F32
                } else {
                    types::F64
                };
                let l = self.load(ty, left);
                let r = self.load(ty, right);
                // IEEE: -0 equals +0; an unordered pair decides as unordered.
                let equal = self.b.ins().fcmp(FloatCC::Equal, l, r);
                let less = self.b.ins().fcmp(FloatCC::LessThan, l, r);
                let greater = self.b.ins().fcmp(FloatCC::GreaterThan, l, r);
                let ordered = self.order(less, greater);
                let (e, u) = (self.code(ORDER_EQUAL), self.code(ORDER_UNORDERED));
                let other = self.b.ins().select(equal, e, u);
                let decided = self.b.ins().bor(less, greater);
                return Ok(self.b.ins().select(decided, ordered, other));
            }
            K::Pointer => return self.invalid("a structural comparison of an address"),
        };
        let l = self.load(ty, left);
        let r = self.load(ty, right);
        let (lt, gt) = if signed {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let less = self.b.ins().icmp(lt, l, r);
        let greater = self.b.ins().icmp(gt, l, r);
        Ok(self.order(less, greater))
    }

    // Hash functions.

    /// `hash_bytes(state, bytes, length)`.
    fn feed(&mut self, state: Value, bytes: Value, length: Value) -> Generated<Value> {
        let id = self.em.runtime(RuntimeEntry::HashUpdate)?;
        Ok(self.call(id, &[state, bytes, length]))
    }

    /// Feeds an integer value of `bytes` bytes through the scratch slot.
    fn feed_int(&mut self, state: Value, value: Value, bytes: u64) -> Generated<Value> {
        let slot = self
            .scratch
            .expect("the scratch slot is allocated at entry");
        self.b.ins().store(flags(), value, slot, 0);
        let length = self.u64(bytes);
        self.feed(state, slot, length)
    }

    /// Too deep: set the flag and return `state`; otherwise continue.
    fn hash_depth(&mut self, depth: Value, state: Value, deep: Value) {
        let too =
            self.b
                .ins()
                .icmp_imm_s(IntCC::UnsignedGreaterThan, depth, COMPARE_MAX_DEPTH as i64);
        let (fail, next) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(too, fail, &[], next, &[]);
        self.b.switch_to_block(fail);
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().store(flags(), one, deep, 0);
        self.b.ins().return_(&[state]);
        self.b.switch_to_block(next);
    }

    fn define_hasher(mut self, layout: LayoutId) -> Generated {
        let slot =
            self.b
                .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 4));
        self.scratch = Some(self.b.ins().stack_addr(PTR, slot, 0));
        let (at, state, depth, deep) = (
            self.params[0],
            self.params[1],
            self.params[2],
            self.params[3],
        );
        self.hash_depth(depth, state, deep);
        let record = self.record(layout)?.clone();
        let deeper = self.b.ins().iadd_imm_s(depth, 1);
        let state = match &record.kind {
            LayoutKind::Path => self.hash_scalar(ScalarKind::U64, at, state)?,
            LayoutKind::Unit => state,
            LayoutKind::Scalar(kind) => self.hash_scalar(*kind, at, state)?,
            LayoutKind::Aggregate => self.hash_fields(&record.fields, at, state, deeper, deep)?,
            LayoutKind::Variant => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant layout without a tag");
                };
                let ordinal = self.tag(at, tag.offset, tag.form);
                let state = self.feed_int(state, ordinal, 4)?;
                let done = self.b.create_block();
                let joined = self.b.append_block_param(done, types::I64);
                let mut switch = Switch::new();
                let mut arms: Vec<(Block, &[FieldLayout])> = Vec::new();
                for variant in &tag.variants {
                    let arm = self.b.create_block();
                    switch.set_entry(u128::from(variant.ordinal), arm);
                    arms.push((arm, &variant.fields));
                }
                let otherwise = self.b.create_block();
                let on = self.b.ins().uextend(types::I64, ordinal);
                switch.emit(&mut self.b, on, otherwise);
                self.b.switch_to_block(otherwise);
                self.b.ins().jump(done, &[state.into()]);
                for (arm, fields) in arms {
                    self.b.switch_to_block(arm);
                    let after = self.hash_fields(fields, at, state, deeper, deep)?;
                    self.b.ins().jump(done, &[after.into()]);
                }
                self.b.switch_to_block(done);
                joined
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(*element)?;
                let count = self.u64(*count);
                self.hash_elements(*element, stride, at, count, state, deeper, deep)?
            }
            LayoutKind::Simd { element, lanes } => {
                let stride = self.size(*element)?;
                let count = self.u64(u64::from(*lanes));
                self.hash_elements(*element, stride, at, count, state, deeper, deep)?
            }
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let pointer = self.load(PTR, at);
                let data = self.payload(pointer, &record, *payload)?;
                match self.record(*payload)?.kind {
                    // A List or Array: its count, then its elements one level deeper.
                    LayoutKind::Sequence { element } => {
                        let count = self.load(types::I64, data);
                        let state = self.feed_int(state, count, 8)?;
                        let start = self.element_offset(element)?;
                        let stride = self.size(element)?;
                        let first = self.at(data, start);
                        self.hash_elements(element, stride, first, count, state, deeper, deep)?
                    }
                    LayoutKind::Opaque { .. } => {
                        self.container_hash(*payload, data, state, deeper, deep)?
                    }
                    // A Box: its payload one level deeper.
                    _ => self.hash_child(*payload, data, state, deeper, deep)?,
                }
            }
            LayoutKind::Slice { element } => {
                // String and Bytes: the length, then the bytes.
                if self.size(*element)? != 1 {
                    return self.unsupported("a hash of a slice of elements wider than a byte");
                }
                let (data, length) = self.slice(&record, *element, at)?;
                let state = self.feed_int(state, length, 8)?;
                self.feed(state, data, length)?
            }
            LayoutKind::Opaque { .. } => self.container_hash(layout, at, state, deeper, deep)?,
            LayoutKind::Packed
            | LayoutKind::Flat
            | LayoutKind::SoaColumns { .. }
            | LayoutKind::Sequence { .. }
            | LayoutKind::Sequential { .. } => {
                return self.unsupported(format!("a hash of {:?}", record.kind));
            }
            LayoutKind::Environment { .. }
            | LayoutKind::Shared { .. }
            | LayoutKind::Frozen { .. }
            | LayoutKind::Handle
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header => {
                return self.invalid(format!("a hash of {:?}", record.kind));
            }
        };
        self.b.ins().return_(&[state]);
        self.finish();
        Ok(())
    }

    fn container_hash(
        &mut self,
        layout: LayoutId,
        at: Value,
        state: Value,
        depth: Value,
        deep: Value,
    ) -> Generated<Value> {
        let record = self.record(layout)?.clone();
        let LayoutKind::Opaque { owner } = &record.kind else {
            return self.invalid("container payload");
        };
        if !owner.0.package.name_is("core") || !matches!(owner.0.name.as_str(), "Map" | "Set") {
            return self.unsupported(format!("hash of opaque {owner:?}"));
        }
        let field = &record.fields[0];
        let entries = self.record(field.layout)?.clone();
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = entries.kind else {
            return self.invalid("container entries");
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return self.invalid("container sequence");
        };
        let field_at = self.at(at, field.offset);
        let pointer = self.load(PTR, field_at);
        let offset = match payload_offset(self.record(payload)?.align) {
            Ok(offset) => offset,
            Err(e) => return self.invalid(format!("{e:?}")),
        };
        let data = self.at(pointer, offset);
        let count_at = if owner.0.name == "Map" {
            self.at(at, record.fields[3].offset)
        } else {
            data
        };
        let count = self.load(types::I64, count_at);
        let state = self.feed_int(state, count, 8)?;
        let start = self.element_offset(element)?;
        let first = self.at(data, start);
        let stride = self.size(element)?;
        if owner.0.name == "Map" {
            let tag = self
                .record(element)?
                .tag
                .clone()
                .ok_or_else(|| CodegenError::Invalid {
                    definition: self.definition.clone(),
                    what: "map entry tag".into(),
                })?;
            let pair = tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0].clone();
            let head = self.b.create_block();
            let fold = self.b.create_block();
            let done = self.b.create_block();
            let i = self.b.append_block_param(head, types::I64);
            let pointer = self.b.append_block_param(head, PTR);
            let state_phi = self.b.append_block_param(head, types::I64);
            let zero = self.u64(0);
            self.b
                .ins()
                .jump(head, &[zero.into(), first.into(), state.into()]);
            self.b.switch_to_block(head);
            let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
            self.b.ins().brif(more, fold, &[], done, &[]);
            self.b.switch_to_block(fold);
            let entry = self.map_next(&tag, stride, pointer);
            let pair_at = self.at(entry, pair.offset);
            let callee = self.em.declare_hasher(pair.layout)?;
            let state = self.call(callee, &[pair_at, state_phi, depth, deep, self.params[4]]);
            let next = self.b.ins().iadd_imm_s(i, 1);
            let pointer = self.at(entry, stride);
            self.b
                .ins()
                .jump(head, &[next.into(), pointer.into(), state.into()]);
            self.b.switch_to_block(done);
            return Ok(state_phi);
        }
        // Canonical Set order feeds one stream and finalizes only at the root.
        let callee = self.em.declare_hasher(element)?;
        let head = self.b.create_block();
        let fold = self.b.create_block();
        let done = self.b.create_block();
        let i = self.b.append_block_param(head, types::I64);
        let previous = self.b.append_block_param(head, PTR);
        let state_phi = self.b.append_block_param(head, types::I64);
        let zero = self.u64(0);
        self.b
            .ins()
            .jump(head, &[zero.into(), first.into(), state.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
        self.b.ins().brif(more, fold, &[], done, &[]);
        self.b.switch_to_block(fold);
        let is_first = self.b.ins().icmp_imm_s(IntCC::Equal, i, 0);
        let entry = self.sorted_next(
            element,
            stride,
            first,
            count,
            previous,
            is_first,
            depth,
            Some((state_phi, deep)),
        )?;
        let state = self.call(callee, &[entry, state_phi, depth, deep, self.params[4]]);
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b
            .ins()
            .jump(head, &[next.into(), entry.into(), state.into()]);
        self.b.switch_to_block(done);
        Ok(state_phi)
    }

    /// A scalar leaf at `at`: its bytes, a float's `-0.0` as `+0.0`.
    fn hash_scalar(&mut self, kind: ScalarKind, at: Value, state: Value) -> Generated<Value> {
        use ScalarKind as K;
        let bytes: u64 = match kind {
            K::Bool | K::U8 | K::I8 => 1,
            K::U16 | K::I16 => 2,
            K::U32 | K::I32 => 4,
            K::U64 | K::I64 => 8,
            K::U128 | K::I128 => 16,
            K::F32 | K::F64 => {
                let (ty, bits_ty, width) = if kind == K::F32 {
                    (types::F32, types::I32, 4)
                } else {
                    (types::F64, types::I64, 8)
                };
                let value = self.load(ty, at);
                let zero = if kind == K::F32 {
                    self.b.ins().f32const(0.0)
                } else {
                    self.b.ins().f64const(0.0)
                };
                let is_zero = self.b.ins().fcmp(FloatCC::Equal, value, zero);
                let value = self.b.ins().select(is_zero, zero, value);
                let bits = self.b.ins().bitcast(bits_ty, flags(), value);
                return self.feed_int(state, bits, width);
            }
            K::Pointer => return self.invalid("a hash of an address"),
        };
        let length = self.u64(bytes);
        self.feed(state, at, length)
    }

    fn hash_fields(
        &mut self,
        fields: &[FieldLayout],
        at: Value,
        mut state: Value,
        depth: Value,
        deep: Value,
    ) -> Generated<Value> {
        let mut fields: Vec<&FieldLayout> = fields.iter().collect();
        fields.sort_by_key(|f| f.ordinal);
        for field in fields {
            let field_at = self.at(at, field.offset);
            state = self.hash_child(field.layout, field_at, state, depth, deep)?;
        }
        Ok(state)
    }

    fn hash_child(
        &mut self,
        layout: LayoutId,
        at: Value,
        state: Value,
        depth: Value,
        deep: Value,
    ) -> Generated<Value> {
        let callee = self.em.declare_hasher(layout)?;
        Ok(self.call(callee, &[at, state, depth, deep, self.params[4]]))
    }

    #[allow(clippy::too_many_arguments)]
    fn hash_elements(
        &mut self,
        element: LayoutId,
        stride: u64,
        first: Value,
        count: Value,
        state: Value,
        depth: Value,
        deep: Value,
    ) -> Generated<Value> {
        let callee = self.em.declare_hasher(element)?;
        let (head, work, after) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let index = self.b.append_block_param(head, types::I64);
        let running = self.b.append_block_param(head, types::I64);
        let zero = self.u64(0);
        self.b.ins().jump(head, &[zero.into(), state.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b.ins().brif(more, work, &[], after, &[]);
        self.b.switch_to_block(work);
        let offset = self.b.ins().imul_imm_s(index, stride as i64);
        let at = self.b.ins().iadd(first, offset);
        let next_state = self.call(callee, &[at, running, depth, deep]);
        let next = self.b.ins().iadd_imm_s(index, 1);
        self.b.ins().jump(head, &[next.into(), next_state.into()]);
        self.b.switch_to_block(after);
        Ok(running)
    }
}

/// Defines the comparator of `layout` with the builder `b`.
pub(crate) fn define_comparator(
    em: &mut Emitter<'_>,
    mut b: FunctionBuilder<'_>,
    layout: LayoutId,
    equality: bool,
    definition: &DefPath,
) -> Emitted {
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let params = b.block_params(entry).to_vec();
    Gen {
        em,
        b,
        definition,
        equality,
        params,
        scratch: None,
    }
    .define_comparator(layout)
}

/// Defines the hash function of `layout` with the builder `b`.
pub(crate) fn define_hasher(
    em: &mut Emitter<'_>,
    mut b: FunctionBuilder<'_>,
    layout: LayoutId,
    definition: &DefPath,
) -> Emitted {
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let params = b.block_params(entry).to_vec();
    Gen {
        em,
        b,
        definition,
        equality: false,
        params,
        scratch: None,
    }
    .define_hasher(layout)
}

impl FnEmitter<'_, '_, '_> {
    /// `Compare`: both operands spilled to slots of the layout and read in place by the
    /// layout's comparator.
    pub(crate) fn compare(
        &mut self,
        result: ValueId,
        op: CompareOp,
        left: ValueId,
        right: ValueId,
        layout: LayoutId,
        failure: FailureId,
    ) -> Emitted {
        let definition = self.definition();
        let comparator = self.em.comparator(
            layout,
            matches!(op, CompareOp::Eq | CompareOp::Ne),
            definition,
        )?;
        let shape = self.layout_shape(layout)?;
        let (l, r) = (self.stack(layout)?, self.stack(layout)?);
        let (left, right) = (self.counted_view(left)?, self.counted_view(right)?);
        self.store(shape, left, l)?;
        self.store(shape, right, r)?;
        let zero = self.u64c(0);
        let context = self.context;
        let code = self.call(comparator, &[l.ptr, r.ptr, zero, context])[0];
        let is = |this: &mut Self, c: u8| this.b.ins().icmp_imm_s(IntCC::Equal, code, i64::from(c));
        let deep = is(self, ORDER_DEPTH);
        self.guard(deep, failure)?;
        let (equal, less, greater) = (
            is(self, ORDER_EQUAL),
            is(self, ORDER_LESS),
            is(self, ORDER_GREATER),
        );
        let holds = match op {
            CompareOp::Eq => equal,
            CompareOp::Ne => self.b.ins().bxor_imm_s(equal, 1),
            CompareOp::Lt => less,
            CompareOp::Le => self.b.ins().bor(equal, less),
            CompareOp::Gt => greater,
            CompareOp::Ge => self.b.ins().bor(equal, greater),
        };
        self.define1(result, holds)
    }

    /// `Hash`: the value spilled to a slot of the layout and fed by its hash function.
    pub(crate) fn hash(
        &mut self,
        result: ValueId,
        value: ValueId,
        layout: LayoutId,
        failure: FailureId,
    ) -> Emitted {
        let at = self.stack(layout)?;
        let operand = self.counted_view(value)?;
        let shape = self.layout_shape(layout)?;
        self.store(shape, operand, at)?;
        let context = self.context;
        let seed = self.rt1(RuntimeEntry::HashBegin, &[context])?;
        if matches!(self.layout(layout)?.kind, LayoutKind::Scalar(kind)
            if !matches!(kind, ScalarKind::F32 | ScalarKind::F64 | ScalarKind::Pointer))
        {
            let size = self.fixed_size(layout)?;
            let length = self.u64c(size);
            let hash = self.rt1(RuntimeEntry::HashBytes, &[seed, at.ptr, length])?;
            return self.define(result, V::One(hash));
        }
        // The complete framing of a short byte slice fits in two words.
        let short = if matches!(self.layout(layout)?.kind, LayoutKind::Slice { element } if self.fixed_size(element)? == 1)
        {
            let (data, length) = self.slice_bytes(value)?;
            let fast = self.b.create_block();
            let long = self.b.create_block();
            let done = self.b.create_block();
            self.b.append_block_param(done, types::I64);
            let fits = self
                .b
                .ins()
                .icmp_imm(IntCC::UnsignedLessThanOrEqual, length, 8);
            self.b.ins().brif(fits, fast, &[], long, &[]);
            self.b.switch_to_block(fast);
            self.b.seal_block(fast);
            let (buffer, _) = self.stack_bytes(16, 8);
            self.store_at(length, buffer, 0);
            let tail = self.offset(buffer, 8);
            self.copy_dynamic(tail, data, length);
            let framed = self.b.ins().iadd_imm(length, 8);
            let hash = self.rt1(RuntimeEntry::HashBytes, &[seed, buffer, framed])?;
            self.b.ins().jump(done, &[hash.into()]);
            self.b.switch_to_block(long);
            self.b.seal_block(long);
            Some(done)
        } else {
            None
        };
        let definition = self.definition();
        let hasher = self.em.hasher(layout, definition)?;
        let (state, _) = self.stack_bytes(
            hirsel_lir::runtime_abi::HASH_STATE_BYTES,
            hirsel_lir::runtime_abi::HASH_STATE_ALIGN,
        );
        self.rt(RuntimeEntry::HashInit, &[state, seed])?;
        let (deep, _) = self.stack_bytes(1, 1);
        let zero = self.b.ins().iconst(types::I8, 0);
        self.store_at(zero, deep, 0);
        let depth = self.u64c(0);
        let hash = self.call(hasher, &[at.ptr, state, depth, deep, context])[0];
        let flag = self.load_ty(types::I8, deep, 0);
        self.guard(flag, failure)?;
        let hash = self.rt1(RuntimeEntry::HashFinish, &[hash])?;
        let hash = if let Some(done) = short {
            self.b.ins().jump(done, &[hash.into()]);
            self.b.switch_to_block(done);
            self.b.seal_block(done);
            self.b.block_params(done)[0]
        } else {
            hash
        };
        self.define(result, V::One(hash))
    }
}
