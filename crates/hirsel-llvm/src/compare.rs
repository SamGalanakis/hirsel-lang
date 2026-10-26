//! Structural comparison (`Inst::Compare`, LIR schema revision 3): one private comparator
//! function per layout, emitted once per module.
//!
//! The order is the interpreter's (`hirsel_interp::structural::compare`) bit for bit: pairs
//! depth first in field ordinal order, the first unequal pair decides; variant tag ordinals
//! before payloads; sequences element by element, then by length (a proper prefix first);
//! String and Bytes slices bytewise, then by length; floats by IEEE, an unordered pair
//! deciding at once. The root pair is at depth 0 and each field, element or Box payload one
//! deeper for private traversal bookkeeping. Complete logical admission happens
//! before entering this physical walker; representation wrappers set no language limit.
//!
//! A comparator `hirsel.compare.<layout key>(left, right, depth, context) -> i8` reads two values of its
//! layout at the given addresses and returns one of the `ORDER_*` codes. Loads are unaligned
//! (a `@packed` field may hold a comparable value at any offset).

use hirsel_lir::LayoutKeys;
use hirsel_lir::layout::{payload_offset, sequence_elements_offset, slice_data_offset};
use hirsel_lir::layout_types::{
    FieldLayout, LayoutId, LayoutKind, LayoutRecord, LayoutTable, ScalarKind, ScanStep, SizeSpec,
};

use crate::context::{
    Attribute, AttributeSlot, BasicBlock, Builder, Cast, FloatPredicate, Function, IntPredicate,
    MemoryEffects, Module, Value,
};
use crate::emit::load_tag;
use hirsel_lir::emission::slice_storage_with_index;

/// The pair is equal.
pub(crate) const ORDER_EQUAL: u8 = 0;
/// The left value is less.
pub(crate) const ORDER_LESS: u8 = 1;
/// The left value is greater.
pub(crate) const ORDER_GREATER: u8 = 2;
/// A float pair is unordered: every operator but `Ne` is false.
pub(crate) const ORDER_UNORDERED: u8 = 3;
/// A pair deeper than `COMPARE_MAX_DEPTH`: the MEM-3 Depth failure.
pub(crate) const ORDER_DEPTH: u8 = 4;

/// Why a layout has no comparator.
pub(crate) enum CompareError {
    /// A comparable layout this emitter cannot compare yet.
    Unsupported(String),
    /// A layout the validators never admit to `Compare`.
    Invalid(String),
}

type Compared<T> = Result<T, CompareError>;

/// A comparator's symbol: by the layout's content key, so a unit's object names no
/// number the program assigns.
fn name(keys: &LayoutKeys, layout: LayoutId) -> String {
    format!("hirsel.compare.{}", keys.key(layout).hex())
}

/// The comparator of `layout` in `module`, generating it and every comparator it calls.
pub(crate) fn comparator<'m>(
    module: &'m Module<'m>,
    layouts: &LayoutTable,
    keys: &LayoutKeys,
    index: Option<&hirsel_lir::LayoutIndex<'_>>,
    layout: LayoutId,
    equality: bool,
) -> Compared<Function<'m>> {
    let mut pending = Vec::new();
    let root = declare(module, keys, layout, equality, &mut pending);
    while let Some((layout, function, entry, equality)) = pending.pop() {
        Generator {
            module,
            layouts,
            keys,
            index,
            function,
            builder: Builder::new(module),
            pending: &mut pending,
            equality,
        }
        .define(layout, entry)?;
    }
    Ok(root)
}

/// A comparator whose body is still to be generated, with its entry block.
type Pending<'m> = (LayoutId, Function<'m>, BasicBlock<'m>, bool);

/// The comparator function of `layout`, declared (and queued for its body) if new. A queued
/// comparator already has its entry block, so the module counts it as defined.
fn declare<'m>(
    module: &'m Module<'m>,
    keys: &LayoutKeys,
    layout: LayoutId,
    equality: bool,
    pending: &mut Vec<Pending<'m>>,
) -> Function<'m> {
    let ptr = module.ptr_type();
    let ty = module.function_type(module.int_type(8), &[ptr, ptr, module.int_type(64), ptr]);
    let name = format!(
        "{}{}",
        name(keys, layout),
        if equality { ".eq" } else { "" }
    );
    let fresh = !module.defines(&name);
    let function = module
        .function(&name, ty)
        .expect("a comparator name is never declared with another type");
    if fresh {
        module.make_private(function);
        pending.push((layout, function, module.append_block(function), equality));
    }
    function
}

struct Generator<'m, 'g> {
    module: &'m Module<'m>,
    layouts: &'g LayoutTable,
    index: Option<&'g hirsel_lir::LayoutIndex<'g>>,
    keys: &'g LayoutKeys,
    function: Function<'m>,
    builder: Builder<'m>,
    pending: &'g mut Vec<Pending<'m>>,
    equality: bool,
}

impl<'m> Generator<'m, '_> {
    fn record(&self, layout: LayoutId) -> Compared<&LayoutRecord> {
        self.layouts
            .layout(layout)
            .ok_or_else(|| CompareError::Invalid(format!("no layout {}", layout.0)))
    }

    fn size(&self, layout: LayoutId) -> Compared<u64> {
        match self.record(layout)?.size {
            SizeSpec::Fixed(size) => Ok(size),
            SizeSpec::Dynamic(_) => Err(CompareError::Unsupported(
                "a comparison of a value of dynamic extent".into(),
            )),
        }
    }

    fn u64(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    fn code(&self, code: u8) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(8), u128::from(code))
    }

    fn block(&self) -> BasicBlock<'m> {
        self.module.append_block(self.function)
    }

    fn at(&self, base: Value<'m>, offset: u64) -> Value<'m> {
        if offset == 0 {
            base
        } else {
            self.builder.offset(base, self.u64(offset))
        }
    }

    /// Returns `code` from the comparator unless it is `ORDER_EQUAL`, then continues.
    fn decide(&self, code: Value<'m>) {
        let equal = self
            .builder
            .icmp(IntPredicate::Eq, code, self.code(ORDER_EQUAL));
        let next = self.block();
        let done = self.block();
        self.builder.cond_br(equal, next, done);
        self.builder.position(done);
        self.builder.ret(code);
        self.builder.position(next);
    }

    /// `ORDER_LESS` when `less`, `ORDER_GREATER` when `greater`, else `ORDER_EQUAL`.
    fn order(&self, less: Value<'m>, greater: Value<'m>) -> Value<'m> {
        let other = self
            .builder
            .select(greater, self.code(ORDER_GREATER), self.code(ORDER_EQUAL));
        self.builder.select(less, self.code(ORDER_LESS), other)
    }

    /// The pair `(left, right)` at `depth` is too deep: return the Depth code.
    fn check_depth(&self, _depth: Value<'m>) {
        // Logical preflight precedes this physical value traversal.
    }

    fn define(mut self, layout: LayoutId, entry: BasicBlock<'m>) -> Compared<()> {
        self.builder.position(entry);
        let (left, right, depth) = (
            self.module.parameter(self.function, 0),
            self.module.parameter(self.function, 1),
            self.module.parameter(self.function, 2),
        );
        self.check_depth(depth);
        let record = self.record(layout)?.clone();
        let deeper = self
            .builder
            .binary(crate::context::BinOp::Add, depth, self.u64(1));
        match &record.kind {
            LayoutKind::Path => {
                let code = if self.equality {
                    self.scalar(ScalarKind::U64, left, right)?
                } else {
                    let word = self.module.int_type(64);
                    let i32_ty = self.module.int_type(32);
                    let leaf = self
                        .module
                        .function(
                            "hirsel_rt_v2_path_compare",
                            self.module
                                .function_type(i32_ty, &[self.module.ptr_type(), word, word]),
                        )
                        .map_err(|_| CompareError::Invalid("Path compare type".into()))?;
                    let result = self.builder.call(
                        leaf,
                        &[
                            self.module.parameter(self.function, 3),
                            self.builder.load(word, left, 1),
                            self.builder.load(word, right, 1),
                        ],
                    );
                    let zero = self.module.const_int(i32_ty, 0);
                    self.order(
                        self.builder.icmp(IntPredicate::Slt, result, zero),
                        self.builder.icmp(IntPredicate::Sgt, result, zero),
                    )
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
                    return Err(CompareError::Invalid(
                        "a variant layout without a tag".into(),
                    ));
                };
                let unaligned = record.abi.unaligned;
                // Active ordinals in declaration order, whatever the tag's encoding.
                let l = load_tag(&self.builder, left, 1, tag.offset, tag.form, unaligned);
                let r = load_tag(&self.builder, right, 1, tag.offset, tag.form, unaligned);
                let less = self.builder.icmp(IntPredicate::Ult, l, r);
                let greater = self.builder.icmp(IntPredicate::Ugt, l, r);
                let code = self.order(less, greater);
                self.decide(code);
                let done = self.block();
                let mut cases = Vec::new();
                let here = self.builder.current();
                for variant in &tag.variants {
                    let arm = self.block();
                    self.builder.position(arm);
                    self.fields(&variant.fields, left, right, deeper)?;
                    self.builder.br(done);
                    cases.push((
                        self.module
                            .const_int(self.module.int_type(32), u128::from(variant.ordinal)),
                        arm,
                    ));
                }
                self.builder.position(here);
                self.builder.switch(l, done, &cases);
                self.builder.position(done);
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(*element)?;
                let count = self.u64(*count);
                self.elements(*element, stride, left, right, count, deeper)?;
            }
            LayoutKind::Simd { element, lanes } => {
                // GEN-1/MEM-8: structural equality compares scalar lanes in order.
                let stride = self.size(*element)?;
                let count = self.u64(u64::from(*lanes));
                self.elements(*element, stride, left, right, count, deeper)?;
            }
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let ptr = self.module.ptr_type();
                let l = self.builder.load(ptr, left, 1);
                let r = self.builder.load(ptr, right, 1);
                let (l, r) = (
                    self.payload(l, &record, *payload)?,
                    self.payload(r, &record, *payload)?,
                );
                let element = match self.record(*payload)?.kind {
                    LayoutKind::Sequence { element } => Some(element),
                    _ => None,
                };
                match element {
                    // A List or Array: its elements are one level deeper, then the lengths.
                    Some(element) => {
                        let start = self.element_offset(element)?;
                        let stride = self.size(element)?;
                        let i64_ty = self.module.int_type(64);
                        let (lc, rc) = (
                            self.builder.load(i64_ty, l, 1),
                            self.builder.load(i64_ty, r, 1),
                        );
                        let fewer = self.builder.icmp(IntPredicate::Ult, lc, rc);
                        let count = self.builder.select(fewer, lc, rc);
                        let (le, re) = (self.at(l, start), self.at(r, start));
                        self.elements(element, stride, le, re, count, deeper)?;
                        let less = self.builder.icmp(IntPredicate::Ult, lc, rc);
                        let greater = self.builder.icmp(IntPredicate::Ugt, lc, rc);
                        let code = self.order(less, greater);
                        self.decide(code);
                    }
                    // A Box: its payload is one level deeper.
                    None if matches!(self.record(*payload)?.kind, LayoutKind::Opaque { .. }) => {
                        self.container(*payload, l, r, deeper)?;
                    }
                    None => {
                        let code = self.child(*payload, l, r, deeper)?;
                        self.decide(code);
                    }
                }
            }
            LayoutKind::Slice { element } => {
                // String and Bytes: bytewise, then by length, with no depth below the value.
                if self.size(*element)? != 1 {
                    return Err(CompareError::Unsupported(
                        "a slice of elements wider than a byte".into(),
                    ));
                }
                let (lp, ll) = self.slice(&record, *element, left)?;
                let (rp, rl) = self.slice(&record, *element, right)?;
                let less = self.builder.icmp(IntPredicate::Ult, ll, rl);
                let greater = self.builder.icmp(IntPredicate::Ugt, ll, rl);
                let lengths = self.order(less, greater);
                if self.equality {
                    // Slices have no child depth check or observable byte reads.
                    // Unequal lengths therefore decide equality before any bytes.
                    self.decide(lengths);
                }
                let count = if self.equality {
                    ll
                } else {
                    self.builder.select(less, ll, rl)
                };
                self.bytes(lp, rp, count)?;
                if !self.equality {
                    self.decide(lengths);
                }
            }
            LayoutKind::Opaque { .. } => self.container(layout, left, right, deeper)?,
            LayoutKind::Packed
            | LayoutKind::Flat
            | LayoutKind::SoaColumns { .. }
            | LayoutKind::Sequence { .. }
            | LayoutKind::Sequential { .. } => {
                return Err(CompareError::Unsupported(format!(
                    "a structural comparison of {:?}",
                    record.kind
                )));
            }
            LayoutKind::Environment { .. }
            | LayoutKind::Handle
            | LayoutKind::HostCapability { .. }
            | LayoutKind::PayloadReceipt { .. }
            | LayoutKind::Shared { .. }
            | LayoutKind::Frozen { .. }
            | LayoutKind::SealedRegion { .. }
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header => {
                return Err(CompareError::Invalid(format!(
                    "a structural comparison of {:?}",
                    record.kind
                )));
            }
        }
        self.builder.ret(self.code(ORDER_EQUAL));
        Ok(())
    }

    fn container(
        &mut self,
        layout: LayoutId,
        left: Value<'m>,
        right: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<()> {
        let record = self.record(layout)?.clone();
        let LayoutKind::Opaque { owner } = &record.kind else {
            return Err(CompareError::Invalid("container payload".into()));
        };
        if !owner.0.package.name_is("core") || !matches!(owner.0.name.as_str(), "Map" | "Set") {
            return Err(CompareError::Unsupported(format!(
                "comparison of opaque {owner:?}"
            )));
        }
        let field = &record.fields[0];
        let entries = self.record(field.layout)?.clone();
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = entries.kind else {
            return Err(CompareError::Invalid("container entries".into()));
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return Err(CompareError::Invalid("container sequence".into()));
        };
        let ptr = self.module.ptr_type();
        let word = self.module.int_type(64);
        let l = self.builder.load(ptr, self.at(left, field.offset), 1);
        let r = self.builder.load(ptr, self.at(right, field.offset), 1);
        let (l, r) = (
            self.payload(l, &entries, payload)?,
            self.payload(r, &entries, payload)?,
        );
        let (lc, rc) = (
            self.builder
                .load(word, self.at(left, record.fields[3].offset), 1),
            self.builder
                .load(word, self.at(right, record.fields[3].offset), 1),
        );
        let fewer = self.builder.icmp(IntPredicate::Ult, lc, rc);
        let count = self.builder.select(fewer, lc, rc);
        let start = self.element_offset(element)?;
        let stride = self.size(element)?;
        let (l, r) = (self.at(l, start), self.at(r, start));
        if owner.0.name == "Map" {
            let tag = self
                .record(element)?
                .tag
                .clone()
                .ok_or_else(|| CompareError::Invalid("map entry tag".into()))?;
            let pair = tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0].clone();
            let from = self.builder.current();
            let head = self.block();
            let compare = self.block();
            let done = self.block();
            self.builder.br(head);
            self.builder.position(head);
            let i = self.builder.phi(word);
            let lp = self.builder.phi(ptr);
            let rp = self.builder.phi(ptr);
            self.builder.add_incoming(i, self.u64(0), from);
            self.builder.add_incoming(lp, l, from);
            self.builder.add_incoming(rp, r, from);
            let more = self.builder.icmp(IntPredicate::Ult, i, count);
            self.builder.cond_br(more, compare, done);
            self.builder.position(compare);
            let ln = map_next(&self.builder, self.function, &tag, stride, lp);
            let rn = map_next(&self.builder, self.function, &tag, stride, rp);
            let code = self.child(
                pair.layout,
                self.at(ln, pair.offset),
                self.at(rn, pair.offset),
                depth,
            )?;
            self.decide(code);
            let next = self
                .builder
                .binary(crate::context::BinOp::Add, i, self.u64(1));
            let next_left = self.at(ln, stride);
            let next_right = self.at(rn, stride);
            let from = self.builder.current();
            self.builder.br(head);
            self.builder.add_incoming(i, next, from);
            self.builder.add_incoming(lp, next_left, from);
            self.builder.add_incoming(rp, next_right, from);
            self.builder.position(done);
        } else {
            let tag = self.record(element)?.tag.as_ref().unwrap();
            let pair = &tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0];
            let key_layout = self.record(pair.layout)?.fields[0].layout;
            // SEM-5, spec #93. Selection yields canonical structural order
            // without allocating or mutating either borrowed set.
            let from = self.builder.current();
            let loop_ = self.block();
            let compare = self.block();
            let done = self.block();
            self.builder.br(loop_);
            self.builder.position(loop_);
            let i = self.builder.phi(word);
            self.builder.add_incoming(i, self.u64(0), from);
            let lp = self.builder.phi(ptr);
            self.builder.add_incoming(lp, l, from);
            let rp = self.builder.phi(ptr);
            self.builder.add_incoming(rp, r, from);
            let more = self.builder.icmp(IntPredicate::Ult, i, count);
            self.builder.cond_br(more, compare, done);
            self.builder.position(compare);
            let first = self.builder.icmp(IntPredicate::Eq, i, self.u64(0));
            let ln = self.sorted_next(element, stride, l, lc, lp, first, depth)?;
            let rn = self.sorted_next(element, stride, r, rc, rp, first, depth)?;
            let code = self.child(key_layout, ln, rn, depth)?;
            self.decide(code);
            let next = self
                .builder
                .binary(crate::context::BinOp::Add, i, self.u64(1));
            let from = self.builder.current();
            self.builder.br(loop_);
            self.builder.add_incoming(i, next, from);
            self.builder.add_incoming(lp, ln, from);
            self.builder.add_incoming(rp, rn, from);
            self.builder.position(done);
        }
        let less = self.builder.icmp(IntPredicate::Ult, lc, rc);
        let greater = self.builder.icmp(IntPredicate::Ugt, lc, rc);
        let code = self.order(less, greater);
        self.decide(code);
        Ok(())
    }
    fn sorted_next(
        &mut self,
        element: LayoutId,
        stride: u64,
        base: Value<'m>,
        count: Value<'m>,
        previous: Value<'m>,
        first: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<Value<'m>> {
        let tag = self
            .record(element)?
            .tag
            .clone()
            .ok_or_else(|| CompareError::Invalid("set entry tag".into()))?;
        let pair = &tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0];
        let key = self.record(pair.layout)?.fields[0].clone();
        let key_offset = pair.offset + key.offset;
        let element = key.layout;
        let first_entry = map_next(&self.builder, self.function, &tag, stride, base);
        let first_key = self.at(first_entry, key_offset);
        let previous = self.builder.select(first, first_key, previous);

        use crate::context::BinOp;
        let word = self.module.int_type(64);
        let ptr = self.module.ptr_type();
        let boolean = self.module.int_type(1);
        let from = self.builder.current();
        let loop_ = self.block();
        let check = self.block();
        let done = self.block();
        self.builder.br(loop_);
        self.builder.position(loop_);
        let i = self.builder.phi(word);
        self.builder.add_incoming(i, self.u64(0), from);
        let best = self.builder.phi(ptr);
        self.builder.add_incoming(best, first_key, from);
        let scan = self.builder.phi(ptr);
        self.builder.add_incoming(scan, base, from);
        let found = self.builder.phi(boolean);
        self.builder
            .add_incoming(found, self.module.const_int(boolean, 0), from);
        let more = self.builder.icmp(IntPredicate::Ult, i, count);
        self.builder.cond_br(more, check, done);
        self.builder.position(check);
        let entry = map_next(&self.builder, self.function, &tag, stride, scan);
        let candidate = self.at(entry, key_offset);
        let equality = self.equality;
        self.equality = false;
        let after = self.child(element, candidate, previous, depth)?;
        self.depth_code(after);
        let greater = self
            .builder
            .icmp(IntPredicate::Eq, after, self.code(ORDER_GREATER));
        let eligible = self.builder.binary(BinOp::Or, first, greater);
        let smaller = self.child(element, candidate, best, depth)?;
        self.equality = equality;
        self.depth_code(smaller);
        let less = self
            .builder
            .icmp(IntPredicate::Eq, smaller, self.code(ORDER_LESS));
        let not_found =
            self.builder
                .icmp(IntPredicate::Eq, found, self.module.const_int(boolean, 0));
        let better = self.builder.binary(BinOp::Or, not_found, less);
        let choose = self.builder.binary(BinOp::And, eligible, better);
        let next_best = self.builder.select(choose, candidate, best);
        let next_found = self.builder.binary(BinOp::Or, found, eligible);
        let next_i = self.builder.binary(BinOp::Add, i, self.u64(1));
        let next_scan = self.at(entry, stride);
        let from = self.builder.current();
        self.builder.br(loop_);
        self.builder.add_incoming(i, next_i, from);
        self.builder.add_incoming(scan, next_scan, from);
        self.builder.add_incoming(best, next_best, from);
        self.builder.add_incoming(found, next_found, from);
        self.builder.position(done);
        Ok(best)
    }
    fn depth_code(&self, code: Value<'m>) {
        let deep = self
            .builder
            .icmp(IntPredicate::Eq, code, self.code(ORDER_DEPTH));
        let fail = self.block();
        let next = self.block();
        self.builder.cond_br(deep, fail, next);
        self.builder.position(fail);
        self.builder.ret(code);
        self.builder.position(next);
    }

    /// The payload a block reference of `record` points at: behind the header of a counted
    /// block, at the reference itself for uncounted storage.
    fn payload(
        &self,
        pointer: Value<'m>,
        record: &LayoutRecord,
        payload: LayoutId,
    ) -> Compared<Value<'m>> {
        let uncounted = record
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if uncounted {
            return Ok(pointer);
        }
        let offset = payload_offset(self.record(payload)?.align)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        Ok(self.at(pointer, offset))
    }

    /// The first element's offset in a sequence payload: after the u64 count and capacity.
    fn element_offset(&self, element: LayoutId) -> Compared<u64> {
        sequence_elements_offset(self.record(element)?)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))
    }

    /// The offset in a slice owner's payload that the slice's start counts from.
    fn slice_offset(&self, element: LayoutId) -> Compared<u64> {
        slice_data_offset(self.record(element)?)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))
    }

    /// The data address and length of the slice at `at`.
    fn slice(
        &self,
        record: &LayoutRecord,
        element: LayoutId,
        at: Value<'m>,
    ) -> Compared<(Value<'m>, Value<'m>)> {
        let field = |ordinal: u32| -> Compared<&FieldLayout> {
            record
                .fields
                .iter()
                .find(|f| f.ordinal == ordinal)
                .ok_or_else(|| CompareError::Invalid("a slice without its fields".into()))
        };
        let (owner, start, length) = (field(0)?, field(1)?, field(2)?);
        // The owner word names the counted sequence block of the slice's element.
        let Some(payload) = slice_storage_with_index(self.layouts, self.index, element) else {
            return Err(CompareError::Invalid(
                "a slice without its element's sequence layout".into(),
            ));
        };
        let i64_ty = self.module.int_type(64);
        let block = self
            .builder
            .load(self.module.ptr_type(), self.at(at, owner.offset), 1);
        let offset = payload_offset(self.record(payload)?.align)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        let data = self.at(block, offset);
        let data = self.at(data, self.slice_offset(element)?);
        let first = self.builder.load(i64_ty, self.at(at, start.offset), 1);
        let data = self.builder.offset(data, first);
        let length = self.builder.load(i64_ty, self.at(at, length.offset), 1);
        Ok((data, length))
    }

    /// Compare the readable byte intervals through the target's unsigned-byte order.
    /// Empty slices do not call libc; short equality uses bounded overlapping
    /// words, adapted from Zig std.mem.eqlBytes. No load leaves either interval.
    fn bytes(&self, left: Value<'m>, right: Value<'m>, count: Value<'m>) -> Compared<()> {
        let work = self.block();
        let done = self.block();
        let nonempty = self.builder.icmp(IntPredicate::Ne, count, self.u64(0));
        self.builder.cond_br(nonempty, work, done);
        self.builder.position(work);
        if self.equality {
            let short = self.block();
            let bulk = self.block();
            let small = self.builder.icmp(IntPredicate::Ule, count, self.u64(16));
            self.builder.cond_br(small, short, bulk);
            self.builder.position(short);
            // Two overlapping words cover [0, count) when width <= count <= 2*width.
            // Read alignment 1: a slice may start at any byte of its owner.
            for width in [8, 4, 2] {
                let compare = self.block();
                let smaller = self.block();
                let enough = self.builder.icmp(IntPredicate::Uge, count, self.u64(width));
                self.builder.cond_br(enough, compare, smaller);
                self.builder.position(compare);
                let word = self.module.int_type(width as u32 * 8);
                let last = self
                    .builder
                    .binary(crate::context::BinOp::Sub, count, self.u64(width));
                let first_equal = self.builder.icmp(
                    IntPredicate::Eq,
                    self.builder.load(word, left, 1),
                    self.builder.load(word, right, 1),
                );
                let last_equal = self.builder.icmp(
                    IntPredicate::Eq,
                    self.builder.load(word, self.builder.offset(left, last), 1),
                    self.builder.load(word, self.builder.offset(right, last), 1),
                );
                let equal =
                    self.builder
                        .binary(crate::context::BinOp::And, first_equal, last_equal);
                self.decide(self.builder.select(
                    equal,
                    self.code(ORDER_EQUAL),
                    self.code(ORDER_GREATER),
                ));
                self.builder.br(done);
                self.builder.position(smaller);
            }
            // The only remaining nonempty interval has one byte.
            self.decide(self.scalar(ScalarKind::U8, left, right)?);
            self.builder.br(done);
            self.builder.position(bulk);
        }
        let ptr = self.module.ptr_type();
        let int = self.module.int_type(32);
        let function = self
            .module
            .function(
                "memcmp",
                self.module
                    .function_type(int, &[ptr, ptr, self.module.int_type(64)]),
            )
            .map_err(|_| CompareError::Invalid("memcmp type".into()))?;
        for attribute in [
            Attribute::NoUnwind,
            Attribute::WillReturn,
            Attribute::Memory(MemoryEffects::ArgumentRead),
        ] {
            self.module
                .add_attribute(function, AttributeSlot::Function, attribute);
        }
        for parameter in [0, 1] {
            self.module.add_attribute(
                function,
                AttributeSlot::Parameter(parameter),
                Attribute::CapturesNone,
            );
        }
        let compared = self.builder.call(function, &[left, right, count]);
        let zero = self.module.const_int(int, 0);
        let less = self.builder.icmp(IntPredicate::Slt, compared, zero);
        let greater = self.builder.icmp(IntPredicate::Sgt, compared, zero);
        self.decide(self.order(less, greater));
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// A loop over `0..count` (an `i64`), running `body` with the index in the loop block.
    fn each(
        &self,
        count: Value<'m>,
        mut body: impl FnMut(&Self, Value<'m>) -> Compared<()>,
    ) -> Compared<()> {
        let i64_ty = self.module.int_type(64);
        let before = self.builder.current();
        let head = self.block();
        let work = self.block();
        let after = self.block();
        self.builder.br(head);
        self.builder.position(head);
        let index = self.builder.phi(i64_ty);
        self.builder.add_incoming(index, self.u64(0), before);
        let more = self.builder.icmp(IntPredicate::Ult, index, count);
        self.builder.cond_br(more, work, after);
        self.builder.position(work);
        body(self, index)?;
        let next = self
            .builder
            .binary(crate::context::BinOp::Add, index, self.u64(1));
        self.builder
            .add_incoming(index, next, self.builder.current());
        self.builder.br(head);
        self.builder.position(after);
        Ok(())
    }

    /// Compares `count` elements of `element`, `stride` apart, each at `depth`.
    fn elements(
        &mut self,
        element: LayoutId,
        stride: u64,
        left: Value<'m>,
        right: Value<'m>,
        count: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<()> {
        let callee = self.callee(element)?;
        self.each(count, |g, i| {
            let offset = g
                .builder
                .binary(crate::context::BinOp::Mul, i, g.u64(stride));
            let (l, r) = (
                g.builder.offset(left, offset),
                g.builder.offset(right, offset),
            );
            let code = g.compare(element, callee, l, r, depth)?;
            g.decide(code);
            Ok(())
        })
    }

    /// Compares each field in ordinal order at `depth`.
    fn fields(
        &mut self,
        fields: &[FieldLayout],
        left: Value<'m>,
        right: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<()> {
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
    fn callee(&mut self, layout: LayoutId) -> Compared<Option<Function<'m>>> {
        Ok(match self.record(layout)?.kind {
            LayoutKind::Scalar(_) => None,
            _ => Some(declare(
                self.module,
                self.keys,
                layout,
                self.equality,
                self.pending,
            )),
        })
    }

    fn child(
        &mut self,
        layout: LayoutId,
        left: Value<'m>,
        right: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<Value<'m>> {
        let callee = self.callee(layout)?;
        self.compare(layout, callee, left, right, depth)
    }

    /// The code of the pair at `depth`: a scalar leaf inline after its depth check, any
    /// other layout through its comparator.
    fn compare(
        &self,
        layout: LayoutId,
        callee: Option<Function<'m>>,
        left: Value<'m>,
        right: Value<'m>,
        depth: Value<'m>,
    ) -> Compared<Value<'m>> {
        match callee {
            Some(function) => Ok(self.builder.call(
                function,
                &[left, right, depth, self.module.parameter(self.function, 3)],
            )),
            None => {
                let LayoutKind::Scalar(kind) = self.record(layout)?.kind else {
                    return Err(CompareError::Invalid(
                        "an inline leaf that is not a scalar".into(),
                    ));
                };
                self.check_depth(depth);
                self.scalar(kind, left, right)
            }
        }
    }

    /// The order of two scalars of `kind` in memory.
    fn scalar(&self, kind: ScalarKind, left: Value<'m>, right: Value<'m>) -> Compared<Value<'m>> {
        use ScalarKind as K;
        let (bits, signed) = match kind {
            K::Bool | K::U8 => (8, false),
            K::U16 => (16, false),
            K::U32 => (32, false),
            K::U64 => (64, false),
            K::U128 => (128, false),
            K::I8 => (8, true),
            K::I16 => (16, true),
            K::I32 => (32, true),
            K::I64 => (64, true),
            K::I128 => (128, true),
            K::F32 | K::F64 => {
                let ty = if kind == K::F32 {
                    self.module.f32_type()
                } else {
                    self.module.f64_type()
                };
                let l = self.builder.load(ty, left, 1);
                let r = self.builder.load(ty, right, 1);
                // IEEE: -0 equals +0; an unordered pair decides as unordered.
                let equal = self.builder.fcmp(FloatPredicate::Oeq, l, r);
                let less = self.builder.fcmp(FloatPredicate::Olt, l, r);
                let greater = self.builder.fcmp(FloatPredicate::Ogt, l, r);
                let ordered = self.order(less, greater);
                let other =
                    self.builder
                        .select(equal, self.code(ORDER_EQUAL), self.code(ORDER_UNORDERED));
                let decided = self
                    .builder
                    .binary(crate::context::BinOp::Or, less, greater);
                return Ok(self.builder.select(decided, ordered, other));
            }
            K::Pointer => {
                return Err(CompareError::Invalid(
                    "a structural comparison of an address".into(),
                ));
            }
        };
        let ty = self.module.int_type(bits);
        let l = self.builder.load(ty, left, 1);
        let r = self.builder.load(ty, right, 1);
        let (lt, gt) = if signed {
            (IntPredicate::Slt, IntPredicate::Sgt)
        } else {
            (IntPredicate::Ult, IntPredicate::Ugt)
        };
        let less = self.builder.icmp(lt, l, r);
        let greater = self.builder.icmp(gt, l, r);
        Ok(self.order(less, greater))
    }
}

// ---------------------------------------------------------------------------
// Structural hashing (`Inst::Hash`, LIR schema revision 7)

/// The hash function of `layout` in `module`, generating it and every hash function it calls.
///
/// A hash function `hirsel.hash.<layout>(at, state, depth, deep, context) -> ptr`
/// feeds framed leaves into one caller-owned SipHash stream through `hash_update`.
/// It returns the unchanged scratch pointer; only the root finalizes it. Floats feed `-0.0` as `+0.0`, so values `Compare Eq` calls equal hash equal; a
/// variant feeds its active ordinal, a sequence its count, a String or Bytes slice its length
/// and bytes. Complete logical admission happens before this physical traversal;
/// its private depth and failure parameters retain the helper calling convention. `Opaque` containers (Map, Set) are lib-map's.
type HashPending<'m> = (LayoutId, Function<'m>, BasicBlock<'m>);

pub(crate) fn hasher<'m>(
    module: &'m Module<'m>,
    layouts: &LayoutTable,
    keys: &LayoutKeys,
    index: Option<&hirsel_lir::LayoutIndex<'_>>,
    layout: LayoutId,
    hash_bytes: Function<'m>,
) -> Compared<Function<'m>> {
    let mut pending = Vec::new();
    let root = declare_hash(module, keys, layout, &mut pending);
    while let Some((layout, function, entry)) = pending.pop() {
        HashGenerator {
            module,
            layouts,
            keys,
            index,
            function,
            builder: Builder::new(module),
            pending: &mut pending,
            hash_bytes,
            scratch: None,
        }
        .define(layout, entry)?;
    }
    Ok(root)
}

fn declare_hash<'m>(
    module: &'m Module<'m>,
    keys: &LayoutKeys,
    layout: LayoutId,
    pending: &mut Vec<HashPending<'m>>,
) -> Function<'m> {
    let (ptr, i64_ty) = (module.ptr_type(), module.int_type(64));
    let ty = module.function_type(ptr, &[ptr, ptr, i64_ty, ptr, ptr]);
    let name = format!("hirsel.hash.{}", keys.key(layout).hex());
    let fresh = !module.defines(&name);
    let function = module
        .function(&name, ty)
        .expect("a hash function name is never declared with another type");
    if fresh {
        module.make_private(function);
        pending.push((layout, function, module.append_block(function)));
    }
    function
}

struct HashGenerator<'m, 'g> {
    module: &'m Module<'m>,
    layouts: &'g LayoutTable,
    index: Option<&'g hirsel_lir::LayoutIndex<'g>>,
    keys: &'g LayoutKeys,
    function: Function<'m>,
    builder: Builder<'m>,
    pending: &'g mut Vec<HashPending<'m>>,
    hash_bytes: Function<'m>,
    /// A 16-byte stack slot in the entry block that feeds a computed scalar.
    scratch: Option<Value<'m>>,
}

impl<'m> HashGenerator<'m, '_> {
    fn record(&self, layout: LayoutId) -> Compared<&LayoutRecord> {
        self.layouts
            .layout(layout)
            .ok_or_else(|| CompareError::Invalid(format!("no layout {}", layout.0)))
    }

    fn size(&self, layout: LayoutId) -> Compared<u64> {
        match self.record(layout)?.size {
            SizeSpec::Fixed(size) => Ok(size),
            SizeSpec::Dynamic(_) => Err(CompareError::Unsupported(
                "a hash of a value of dynamic extent".into(),
            )),
        }
    }

    fn u64(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    fn at(&self, base: Value<'m>, offset: u64) -> Value<'m> {
        if offset == 0 {
            base
        } else {
            self.builder.offset(base, self.u64(offset))
        }
    }

    /// `hash_bytes(state, bytes, length)`.
    fn feed(&self, state: Value<'m>, bytes: Value<'m>, length: Value<'m>) -> Value<'m> {
        self.builder.call(self.hash_bytes, &[state, bytes, length]);
        state
    }

    /// Feeds an integer value through a stack slot of its width.
    fn feed_int(&self, state: Value<'m>, value: Value<'m>, bytes: u64) -> Value<'m> {
        let slot = self
            .scratch
            .expect("the scratch slot is allocated at entry");
        self.builder.store(value, slot, bytes as u32);
        self.feed(state, slot, self.u64(bytes))
    }

    /// Too deep: set the flag and return `state`; otherwise continue.
    fn check_depth(&self, _depth: Value<'m>, _state: Value<'m>, _deep: Value<'m>) {
        // Logical preflight precedes this physical value traversal.
    }

    fn define(mut self, layout: LayoutId, entry: BasicBlock<'m>) -> Compared<()> {
        self.builder.position(entry);
        let scratch_ty = self.module.array_type(self.module.int_type(8), 16);
        self.scratch = Some(self.builder.alloca(scratch_ty, 16));
        let p = |i| self.module.parameter(self.function, i);
        let (at, state, depth, deep) = (p(0), p(1), p(2), p(3));
        self.check_depth(depth, state, deep);
        let record = self.record(layout)?.clone();
        let deeper = self
            .builder
            .binary(crate::context::BinOp::Add, depth, self.u64(1));
        let state = match &record.kind {
            LayoutKind::Path => self.scalar(ScalarKind::U64, at, state)?,
            LayoutKind::Unit => state,
            LayoutKind::Scalar(kind) => self.scalar(*kind, at, state)?,
            LayoutKind::Aggregate => self.fields(&record.fields, at, state, deeper, deep)?,
            LayoutKind::Variant => {
                let Some(tag) = &record.tag else {
                    return Err(CompareError::Invalid(
                        "a variant layout without a tag".into(),
                    ));
                };
                let ordinal = load_tag(
                    &self.builder,
                    at,
                    1,
                    tag.offset,
                    tag.form,
                    record.abi.unaligned,
                );
                let state = self.feed_int(state, ordinal, 4);
                let done = self.module.append_block(self.function);
                let here = self.builder.current();
                let state_ty = self.module.ptr_type();
                let mut arms = Vec::new();
                let mut incoming = vec![(state, here)];
                for variant in &tag.variants {
                    let arm = self.module.append_block(self.function);
                    self.builder.position(arm);
                    let after = self.fields(&variant.fields, at, state, deeper, deep)?;
                    incoming.push((after, self.builder.current()));
                    self.builder.br(done);
                    arms.push((
                        self.module
                            .const_int(self.module.int_type(32), u128::from(variant.ordinal)),
                        arm,
                    ));
                }
                self.builder.position(here);
                self.builder.switch(ordinal, done, &arms);
                self.builder.position(done);
                let joined = self.builder.phi(state_ty);
                for (value, from) in incoming {
                    self.builder.add_incoming(joined, value, from);
                }
                joined
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(*element)?;
                self.elements(*element, stride, at, self.u64(*count), state, deeper, deep)?
            }
            LayoutKind::Simd { element, lanes } => {
                let stride = self.size(*element)?;
                let count = self.u64(u64::from(*lanes));
                self.elements(*element, stride, at, count, state, deeper, deep)?
            }
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let pointer = self.builder.load(self.module.ptr_type(), at, 1);
                let uncounted = record
                    .scan
                    .iter()
                    .any(|step| matches!(step, ScanStep::Uncounted { .. }));
                let data = if uncounted {
                    pointer
                } else {
                    let offset = payload_offset(self.record(*payload)?.align)
                        .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
                    self.at(pointer, offset)
                };
                match self.record(*payload)?.kind {
                    // A List or Array: its count, then its elements one level deeper.
                    LayoutKind::Sequence { element } => {
                        let i64_ty = self.module.int_type(64);
                        let count = self.builder.load(i64_ty, data, 1);
                        let state = self.feed_int(state, count, 8);
                        let start = sequence_elements_offset(self.record(element)?)
                            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
                        let stride = self.size(element)?;
                        let first = self.at(data, start);
                        self.elements(element, stride, first, count, state, deeper, deep)?
                    }
                    LayoutKind::Opaque { .. } => {
                        self.container_hash(*payload, data, state, deeper, deep)?
                    }
                    // A Box: its payload one level deeper.
                    _ => self.child(*payload, data, state, deeper, deep)?,
                }
            }
            LayoutKind::Slice { element } => {
                // String and Bytes: the length, then the bytes; no depth below the value.
                if self.size(*element)? != 1 {
                    return Err(CompareError::Unsupported(
                        "a hash of a slice of elements wider than a byte".into(),
                    ));
                }
                let (data, length) = self.slice(&record, *element, at)?;
                let state = self.feed_int(state, length, 8);
                self.feed(state, data, length)
            }
            LayoutKind::Opaque { .. } => self.container_hash(layout, at, state, deeper, deep)?,
            LayoutKind::Packed
            | LayoutKind::Flat
            | LayoutKind::SoaColumns { .. }
            | LayoutKind::Sequence { .. }
            | LayoutKind::Sequential { .. } => {
                return Err(CompareError::Unsupported(format!(
                    "a hash of {:?}",
                    record.kind
                )));
            }
            LayoutKind::Environment { .. }
            | LayoutKind::Handle
            | LayoutKind::HostCapability { .. }
            | LayoutKind::PayloadReceipt { .. }
            | LayoutKind::Shared { .. }
            | LayoutKind::Frozen { .. }
            | LayoutKind::SealedRegion { .. }
            | LayoutKind::Arena
            | LayoutKind::Callable { .. }
            | LayoutKind::Iterator { .. }
            | LayoutKind::Resource
            | LayoutKind::Header => {
                return Err(CompareError::Invalid(format!(
                    "a hash of {:?}",
                    record.kind
                )));
            }
        };
        self.builder.ret(state);
        Ok(())
    }

    fn sorted_next(
        &mut self,
        element: LayoutId,
        stride: u64,
        base: Value<'m>,
        count: Value<'m>,
        previous: Value<'m>,
        first: Value<'m>,
        depth: Value<'m>,
        state: Value<'m>,
        deep: Value<'m>,
    ) -> Compared<Value<'m>> {
        let tag = self
            .record(element)?
            .tag
            .clone()
            .ok_or_else(|| CompareError::Invalid("set entry tag".into()))?;
        let pair = &tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0];
        let key = self.record(pair.layout)?.fields[0].clone();
        let key_offset = pair.offset + key.offset;
        let element = key.layout;
        let first_entry = map_next(&self.builder, self.function, &tag, stride, base);
        let first_key = self.at(first_entry, key_offset);
        let previous = self.builder.select(first, first_key, previous);

        use crate::context::BinOp;
        let word = self.module.int_type(64);
        let ptr = self.module.ptr_type();
        let boolean = self.module.int_type(1);
        let from = self.builder.current();
        let loop_ = self.module.append_block(self.function);
        let check = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.br(loop_);
        self.builder.position(loop_);
        let i = self.builder.phi(word);
        self.builder.add_incoming(i, self.u64(0), from);
        let best = self.builder.phi(ptr);
        self.builder.add_incoming(best, first_key, from);
        let scan = self.builder.phi(ptr);
        self.builder.add_incoming(scan, base, from);
        let found = self.builder.phi(boolean);
        self.builder
            .add_incoming(found, self.module.const_int(boolean, 0), from);
        let more = self.builder.icmp(IntPredicate::Ult, i, count);
        self.builder.cond_br(more, check, done);
        self.builder.position(check);
        let entry = map_next(&self.builder, self.function, &tag, stride, scan);
        let candidate = self.at(entry, key_offset);
        let comparator = comparator(
            self.module,
            self.layouts,
            self.keys,
            self.index,
            element,
            false,
        )?;
        let context = self.module.parameter(self.function, 4);
        let after = self
            .builder
            .call(comparator, &[candidate, previous, depth, context]);
        self.hash_order_depth(after, state, deep);
        let greater = self.builder.icmp(
            IntPredicate::Eq,
            after,
            self.module
                .const_int(self.module.int_type(8), u128::from(ORDER_GREATER)),
        );
        let eligible = self.builder.binary(BinOp::Or, first, greater);
        let smaller = self
            .builder
            .call(comparator, &[candidate, best, depth, context]);
        self.hash_order_depth(smaller, state, deep);
        let less = self.builder.icmp(
            IntPredicate::Eq,
            smaller,
            self.module
                .const_int(self.module.int_type(8), u128::from(ORDER_LESS)),
        );
        let not_found =
            self.builder
                .icmp(IntPredicate::Eq, found, self.module.const_int(boolean, 0));
        let better = self.builder.binary(BinOp::Or, not_found, less);
        let choose = self.builder.binary(BinOp::And, eligible, better);
        let next_best = self.builder.select(choose, candidate, best);
        let next_found = self.builder.binary(BinOp::Or, found, eligible);
        let next_i = self.builder.binary(BinOp::Add, i, self.u64(1));
        let next_scan = self.at(entry, stride);
        let from = self.builder.current();
        self.builder.br(loop_);
        self.builder.add_incoming(i, next_i, from);
        self.builder.add_incoming(scan, next_scan, from);
        self.builder.add_incoming(best, next_best, from);
        self.builder.add_incoming(found, next_found, from);
        self.builder.position(done);
        Ok(best)
    }
    fn hash_order_depth(&self, code: Value<'m>, state: Value<'m>, deep: Value<'m>) {
        let too = self.builder.icmp(
            IntPredicate::Eq,
            code,
            self.module
                .const_int(self.module.int_type(8), u128::from(ORDER_DEPTH)),
        );
        let failed = self.module.append_block(self.function);
        let next = self.module.append_block(self.function);
        self.builder.cond_br(too, failed, next);
        self.builder.position(failed);
        self.builder
            .store(self.module.const_int(self.module.int_type(8), 1), deep, 1);
        self.builder.ret(state);
        self.builder.position(next);
    }
    fn container_hash(
        &mut self,
        layout: LayoutId,
        at: Value<'m>,
        state: Value<'m>,
        depth: Value<'m>,
        deep: Value<'m>,
    ) -> Compared<Value<'m>> {
        use crate::context::BinOp;
        let record = self.record(layout)?.clone();
        let LayoutKind::Opaque { owner } = &record.kind else {
            return Err(CompareError::Invalid("container payload".into()));
        };
        if !owner.0.package.name_is("core") || !matches!(owner.0.name.as_str(), "Map" | "Set") {
            return Err(CompareError::Unsupported(format!(
                "hash of opaque {owner:?}"
            )));
        }
        let field = &record.fields[0];
        let entries = self.record(field.layout)?;
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = entries.kind else {
            return Err(CompareError::Invalid("container entries".into()));
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return Err(CompareError::Invalid("container sequence".into()));
        };
        let pointer = self
            .builder
            .load(self.module.ptr_type(), self.at(at, field.offset), 1);
        let offset = payload_offset(self.record(payload)?.align)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        let data = self.at(pointer, offset);
        let word = self.module.int_type(64);
        let count = if record.fields.len() == 4 {
            self.builder
                .load(word, self.at(at, record.fields[3].offset), 1)
        } else {
            self.builder.load(word, data, 1)
        };
        let state = self.feed_int(state, count, 8);
        let start = sequence_elements_offset(self.record(element)?)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        let first = self.at(data, start);
        let stride = self.size(element)?;
        if owner.0.name == "Map" {
            let tag = self
                .record(element)?
                .tag
                .clone()
                .ok_or_else(|| CompareError::Invalid("map entry tag".into()))?;
            let pair = tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0].clone();
            let from = self.builder.current();
            let head = self.module.append_block(self.function);
            let fold = self.module.append_block(self.function);
            let done = self.module.append_block(self.function);
            self.builder.br(head);
            self.builder.position(head);
            let i = self.builder.phi(word);
            let pointer = self.builder.phi(self.module.ptr_type());
            let state_phi = self.builder.phi(self.module.ptr_type());
            self.builder.add_incoming(i, self.u64(0), from);
            self.builder.add_incoming(pointer, first, from);
            self.builder.add_incoming(state_phi, state, from);
            let more = self.builder.icmp(IntPredicate::Ult, i, count);
            self.builder.cond_br(more, fold, done);
            self.builder.position(fold);
            let entry = map_next(&self.builder, self.function, &tag, stride, pointer);
            let state = self.child(
                pair.layout,
                self.at(entry, pair.offset),
                state_phi,
                depth,
                deep,
            )?;
            let next = self.builder.binary(BinOp::Add, i, self.u64(1));
            let next_entry = self.at(entry, stride);
            let from = self.builder.current();
            self.builder.br(head);
            self.builder.add_incoming(i, next, from);
            self.builder.add_incoming(pointer, next_entry, from);
            self.builder.add_incoming(state_phi, state, from);
            self.builder.position(done);
            return Ok(state_phi);
        }
        let tag = self.record(element)?.tag.as_ref().unwrap();
        let pair = &tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0];
        let key_layout = self.record(pair.layout)?.fields[0].layout;
        // Canonical structural order makes a set's stream independent of
        // insertion order, with one finalization for the complete key.
        let from = self.builder.current();
        let head = self.module.append_block(self.function);
        let fold = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.br(head);
        self.builder.position(head);
        let i = self.builder.phi(word);
        let previous = self.builder.phi(self.module.ptr_type());
        let state_phi = self.builder.phi(self.module.ptr_type());
        self.builder.add_incoming(i, self.u64(0), from);
        self.builder.add_incoming(previous, first, from);
        self.builder.add_incoming(state_phi, state, from);
        let more = self.builder.icmp(IntPredicate::Ult, i, count);
        self.builder.cond_br(more, fold, done);
        self.builder.position(fold);
        let is_first = self.builder.icmp(IntPredicate::Eq, i, self.u64(0));
        let entry = self.sorted_next(
            element, stride, first, count, previous, is_first, depth, state_phi, deep,
        )?;
        let state = self.child(key_layout, entry, state_phi, depth, deep)?;
        let next = self.builder.binary(BinOp::Add, i, self.u64(1));
        let from = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(i, next, from);
        self.builder.add_incoming(previous, entry, from);
        self.builder.add_incoming(state_phi, state, from);
        self.builder.position(done);
        Ok(state_phi)
    }

    /// A scalar leaf at `at`: its bytes, a float's `-0.0` as `+0.0`.
    fn scalar(&self, kind: ScalarKind, at: Value<'m>, state: Value<'m>) -> Compared<Value<'m>> {
        use ScalarKind as K;
        let bytes: u64 = match kind {
            K::Bool | K::U8 | K::I8 => 1,
            K::U16 | K::I16 => 2,
            K::U32 | K::I32 => 4,
            K::U64 | K::I64 => 8,
            K::U128 | K::I128 => 16,
            K::F32 | K::F64 => {
                let ty = if kind == K::F32 {
                    self.module.f32_type()
                } else {
                    self.module.f64_type()
                };
                let value = self.builder.load(ty, at, 1);
                let zero = self.module.const_zero(ty);
                let is_zero = self.builder.fcmp(FloatPredicate::Oeq, value, zero);
                let value = self.builder.select(is_zero, zero, value);
                let width = if kind == K::F32 { 4 } else { 8 };
                let bits = self
                    .builder
                    .cast(Cast::Bits, value, self.module.int_type(width * 8));
                return Ok(self.feed_int(state, bits, u64::from(width)));
            }
            K::Pointer => {
                return Err(CompareError::Invalid("a hash of an address".into()));
            }
        };
        Ok(self.feed(state, at, self.u64(bytes)))
    }

    fn fields(
        &mut self,
        fields: &[FieldLayout],
        at: Value<'m>,
        mut state: Value<'m>,
        depth: Value<'m>,
        deep: Value<'m>,
    ) -> Compared<Value<'m>> {
        let mut fields: Vec<&FieldLayout> = fields.iter().collect();
        fields.sort_by_key(|f| f.ordinal);
        for field in fields {
            state = self.child(field.layout, self.at(at, field.offset), state, depth, deep)?;
        }
        Ok(state)
    }

    fn child(
        &mut self,
        layout: LayoutId,
        at: Value<'m>,
        state: Value<'m>,
        depth: Value<'m>,
        deep: Value<'m>,
    ) -> Compared<Value<'m>> {
        let callee = declare_hash(self.module, self.keys, layout, self.pending);
        Ok(self.builder.call(
            callee,
            &[
                at,
                state,
                depth,
                deep,
                self.module.parameter(self.function, 4),
            ],
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn elements(
        &mut self,
        element: LayoutId,
        stride: u64,
        first: Value<'m>,
        count: Value<'m>,
        state: Value<'m>,
        depth: Value<'m>,
        deep: Value<'m>,
    ) -> Compared<Value<'m>> {
        let callee = declare_hash(self.module, self.keys, element, self.pending);
        let i64_ty = self.module.int_type(64);
        let before = self.builder.current();
        let head = self.module.append_block(self.function);
        let work = self.module.append_block(self.function);
        let after = self.module.append_block(self.function);
        self.builder.br(head);
        self.builder.position(head);
        let index = self.builder.phi(i64_ty);
        let running = self.builder.phi(self.module.ptr_type());
        self.builder.add_incoming(index, self.u64(0), before);
        self.builder.add_incoming(running, state, before);
        let more = self.builder.icmp(IntPredicate::Ult, index, count);
        self.builder.cond_br(more, work, after);
        self.builder.position(work);
        let offset = self
            .builder
            .binary(crate::context::BinOp::Mul, index, self.u64(stride));
        let at = self.builder.offset(first, offset);
        let next_state = self.builder.call(
            callee,
            &[
                at,
                running,
                depth,
                deep,
                self.module.parameter(self.function, 4),
            ],
        );
        let next = self
            .builder
            .binary(crate::context::BinOp::Add, index, self.u64(1));
        let from = self.builder.current();
        self.builder.add_incoming(index, next, from);
        self.builder.add_incoming(running, next_state, from);
        self.builder.br(head);
        self.builder.position(after);
        Ok(running)
    }

    /// The data address and length of the slice at `at`.
    fn slice(
        &self,
        record: &LayoutRecord,
        element: LayoutId,
        at: Value<'m>,
    ) -> Compared<(Value<'m>, Value<'m>)> {
        let field = |ordinal: u32| -> Compared<&FieldLayout> {
            record
                .fields
                .iter()
                .find(|f| f.ordinal == ordinal)
                .ok_or_else(|| CompareError::Invalid("a slice without its fields".into()))
        };
        let (owner, start, length) = (field(0)?, field(1)?, field(2)?);
        let Some(payload) = slice_storage_with_index(self.layouts, self.index, element) else {
            return Err(CompareError::Invalid(
                "a slice without its element's sequence layout".into(),
            ));
        };
        let i64_ty = self.module.int_type(64);
        let block = self
            .builder
            .load(self.module.ptr_type(), self.at(at, owner.offset), 1);
        let offset = payload_offset(self.record(payload)?.align)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        let data = self.at(block, offset);
        // A slice's start counts from the slice data offset, not the first element.
        let base = slice_data_offset(self.record(element)?)
            .map_err(|e| CompareError::Invalid(format!("{e:?}")))?;
        let data = self.at(data, base);
        let first = self.builder.load(i64_ty, self.at(at, start.offset), 1);
        let data = self.builder.offset(data, first);
        let length = self.builder.load(i64_ty, self.at(at, length.offset), 1);
        Ok((data, length))
    }
}

/// Private Map holes do not contribute a structural field or a depth level.
fn map_next<'m>(
    builder: &Builder<'m>,
    function: Function<'m>,
    tag: &hirsel_lir::layout_types::TagEncoding,
    stride: u64,
    first: Value<'m>,
) -> Value<'m> {
    let module = builder.module();
    let from = builder.current();
    let head = module.append_block(function);
    let skip = module.append_block(function);
    let done = module.append_block(function);
    builder.br(head);
    builder.position(head);
    let at = builder.phi(module.ptr_type());
    builder.add_incoming(at, first, from);
    let ordinal = load_tag(builder, at, 1, tag.offset, tag.form, true);
    let live = builder.icmp(
        IntPredicate::Eq,
        ordinal,
        module.const_int(module.int_type(32), 1),
    );
    builder.cond_br(live, done, skip);
    builder.position(skip);
    let next = builder.offset(
        at,
        module.const_int(module.int_type(64), u128::from(stride)),
    );
    builder.br(head);
    builder.add_incoming(at, next, skip);
    builder.position(done);
    at
}
