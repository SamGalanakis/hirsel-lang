//! Instructions, as `hirsel-llvm`'s `instructions` lowers them without its deferred count
//! stores, uniqueness facts or adopted builder blocks: each count, allocation and release
//! goes through its runtime entry.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::emission::slice_storage_with_index;
use hirsel_lir::ir::*;
use hirsel_lir::layout::{
    SEQUENCE_CAPACITY_OFFSET, sequence_elements_offset, sequence_slice_start,
};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScalarKind, SizeSpec};
use hirsel_lir::runtime_abi::{COUNT_OFFSET, COUNT_UNIQUE, RuntimeEntry};

use crate::flat::FlatPart;
use crate::function::{FnEmitter, Place, Shape, V};
use crate::module::{Emitted, PTR};

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn instruction(&mut self, inst: &Inst) -> Emitted {
        if self.host_instruction(inst)? {
            return Ok(());
        }
        if let Inst::Release { layout, .. } = inst
            && !(self
                .em
                .inputs
                .unit()
                .ahead
                .and_then(|p| p.host_spine(self.unit.header.instance))
                .is_some()
                && hirsel_lir::ahead::scalar_image(self.em.inputs.unit().layouts, *layout))
        {
            self.ahead_barrier()?;
        }
        self.instruction_body(inst)
    }
    pub(crate) fn instruction_body(&mut self, inst: &Inst) -> Emitted {
        match *inst {
            Inst::Const { result, constant } => {
                let value = self.constant(constant)?;
                self.define(result, value)
            }
            Inst::ReadStatic { result, symbol } => {
                let value = self.read_static(result, symbol)?;
                self.define(result, value)
            }
            Inst::FunctionRef { result, symbol } => {
                let value = self.function_ref(result, symbol)?;
                self.define(result, value)
            }
            // Its environment allocation cannot fail, so the failure edge is never taken.
            Inst::Closure {
                result,
                closure,
                failure: _,
            } => {
                let value = self.closure(result, closure)?;
                self.define(result, value)
            }
            // The same bits; only the copy recipe changes.
            Inst::Weaken { result, value } => {
                let value = self.get(value)?;
                self.define(result, value)
            }
            Inst::IsUnique { result, value } => {
                let block = self.get1(value)?;
                let unique = self.is_unique(block);
                self.define1(result, unique)
            }
            Inst::IsWritable { result, value } => {
                let block = self.get1(value)?;
                let writable = self.is_writable(block);
                self.define1(result, writable)
            }
            Inst::Share { value } => self.share(value),
            Inst::ArenaPush {
                address,
                count,
                failure,
                ..
            } => self.arena_push(address, count, failure),
            Inst::Unary {
                result,
                op,
                operand,
                failure,
            } => {
                let kind = self.scalar_kind(operand)?;
                let operand = self.get1(operand)?;
                let value = self.unary(op, kind, operand, failure)?;
                self.define1(result, value)
            }
            Inst::Binary {
                result,
                op,
                left,
                right,
                failure,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind {
                    return self.invalid("binary operands of different types");
                }
                let (l, r) = (self.get1(left)?, self.get1(right)?);
                let value = self.binary(op, kind, l, r, failure)?;
                self.define1(result, value)
            }
            Inst::Convert {
                result,
                flag,
                operand,
                mode,
            } => {
                let (from, to) = (self.scalar_kind(operand)?, self.scalar_kind(result)?);
                let operand = self.get1(operand)?;
                let (value, status) = self.convert(mode, from, to, operand)?;
                self.define1(result, value)?;
                self.define_flag(flag, status)
            }
            Inst::IntArith {
                result,
                flag,
                op,
                mode,
                left,
                right,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind {
                    return self.invalid("integer operands of different types");
                }
                let (l, r) = (self.get1(left)?, self.get1(right)?);
                let (value, overflow) = self.int_arith(op, mode, kind, l, r)?;
                self.define1(result, value)?;
                self.define_flag(flag, overflow)
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                let kind = self.scalar_kind(left)?;
                if self.scalar_kind(right)? != kind || self.scalar_kind(addend)? != kind {
                    return self.invalid("mul_add operands of different types");
                }
                let operands = [self.get1(left)?, self.get1(right)?, self.get1(addend)?];
                let value = self.mul_add(kind, operands)?;
                self.define1(result, value)
            }
            Inst::ScalarUnary {
                result,
                op,
                operand,
            } => {
                let kind = self.scalar_kind(operand)?;
                let operand = self.get1(operand)?;
                let value = self.scalar_unary(op, kind, operand)?;
                self.define1(result, value)
            }
            Inst::Check { condition, failure } => {
                let condition = self.get1(condition)?;
                let failed = self.b.ins().bxor_imm_s(condition, 1);
                self.guard(failed, failure)
            }
            Inst::FrameEnter { failure } if self.room.enters.contains(&self.instruction_index) => {
                let failed = self.no_room();
                self.guard(failed, failure)
            }
            Inst::FrameLeave if self.room.leaves.contains(&self.instruction_index) => Ok(()),
            Inst::FrameEnter { failure }
                if self.counters.enters.contains(&self.instruction_index) =>
            {
                self.counter_enter(failure)
            }
            Inst::FrameLeave if self.counters.leaves.contains(&self.instruction_index) => {
                self.counter_leave();
                Ok(())
            }
            Inst::FrameEnter { failure } => {
                let context = self.context;
                let anchor = self.unit.tables.failures[failure.index()].anchor;
                let (caller, operation) = self.failure_sites(self.anchor_code(anchor)?);
                let status = self.rt1(
                    RuntimeEntry::DepthEnterAtSite,
                    &[context, caller, operation],
                )?;
                let failed = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
                self.guard(failed, failure)
            }
            Inst::FrameLeave => {
                let context = self.context;
                self.rt(RuntimeEntry::DepthLeave, &[context])?;
                Ok(())
            }
            // Static lifetime events: no machine bytes.
            Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Forget { .. }
            | Inst::Defuse { .. }
            | Inst::RegionPublish { .. } => Ok(()),
            Inst::Address { address, failure } => {
                self.flat_content = None;
                let place = self.prepare(address, failure)?;
                self.addresses[address.index()] = Some(place);
                self.flat_contents[address.index()] = self.flat_content.take();
                Ok(())
            }
            Inst::Read {
                result, address, ..
            } => {
                let place = self.place(address)?;
                let shape = self.value_shape(result)?;
                let place = self.gather(place)?;
                let value = self.load_bounded(shape, place)?;
                let value =
                    self.read_uncounted(value, self.unit.values[result.index()].ty, address)?;
                self.define(result, value)
            }
            // A constant slot's place already holds the stored static's bytes.
            Inst::Store { address, value, .. }
                if self
                    .whole_slot(address)
                    .and_then(|slot| self.constant_slots[slot.index()])
                    .is_some_and(|constant| constant.stored == value) =>
            {
                Ok(())
            }
            Inst::Store { address, value, .. } => {
                let place = self.place(address)?;
                let stored = if self.whole_slot(address).is_some() {
                    self.get(value)?
                } else {
                    self.counted_view(value)?
                };
                let shape = self.value_shape(value)?;
                self.scatter(place, shape, stored)
            }
            Inst::Replace {
                address,
                value,
                displaced,
                ..
            } => {
                // The exchange: old content to the displaced slot, then the new content in.
                let place = self.place(address)?;
                let shape = self.value_shape(value)?;
                let image = self.gather(place)?;
                let old = self.load(shape, image);
                let Some(Some(slot)) = self.slots.get(displaced.index()).copied() else {
                    return self.invalid("a Replace without its displaced slot");
                };
                let old =
                    self.read_uncounted(old, self.unit.slots[displaced.index()].ty, address)?;
                self.store(shape, old, slot)?;
                let stored = if self.whole_slot(address).is_some() {
                    self.get(value)?
                } else {
                    self.counted_view(value)?
                };
                self.scatter(place, shape, stored)
            }
            Inst::UpdateMask {
                result,
                mask,
                field,
                initialized,
            } => {
                let kind = self.scalar_kind(mask)?;
                let Some((width, _)) = crate::scalar::integer(kind) else {
                    return self.invalid("a mask that is not an integer");
                };
                let fields = match self.value_type(mask)?.repr {
                    ValueRepr::InitMask { fields } => fields,
                    _ => width,
                };
                if fields > LIVE_MASK_WORD_BITS {
                    return self
                        .unsupported(live_mask_word_limit("initialization mask", fields as usize));
                }
                if field >= width {
                    return self.unsupported(format!(
                        "initialization mask requires bit {field}; its physical word has {width} bits"
                    ));
                }
                let current = self.get1(mask)?;
                let ty = crate::module::scalar_type(kind);
                let value = if initialized {
                    let bit = self.int_const(ty, 1u128 << field);
                    self.b.ins().bor(current, bit)
                } else {
                    let cleared = self.int_const(ty, !(1u128 << field));
                    self.b.ins().band(current, cleared)
                };
                self.define1(result, value)
            }
            Inst::Construct {
                result,
                construct,
                failure,
            } => self.construct(result, construct, failure),
            Inst::FlatCopy {
                result,
                address,
                layout,
                failure,
            } => self.flat_copy(result, address, layout, failure),
            Inst::UniqueCopy {
                result,
                value,
                construct,
                deep,
                failure,
            } => self.unique_copy(result, value, construct, deep, failure),
            Inst::TokenNone { result } => {
                let null = self.b.ins().iconst(PTR, 0);
                self.define1(result, null)
            }
            Inst::Dup { value } => {
                let block = self.get1(value)?;
                if self.is_receipt_value(value)? {
                    self.rt(RuntimeEntry::ReceiptRetain, &[block])?;
                    return Ok(());
                }
                if self.is_root_value(value)? {
                    return self.root_dup_block(block);
                }
                self.dup_block(block)
            }
            Inst::Dispose { token } => {
                let token = self.get1(token)?;
                let context = self.context;
                self.rt(RuntimeEntry::Dispose, &[context, token])?;
                Ok(())
            }
            Inst::Compare {
                result,
                op,
                left,
                right,
                layout,
                failure,
            } => self.compare(result, op, left, right, layout, failure),
            Inst::SliceView {
                result,
                value,
                start,
                end,
                failure,
            } => {
                let layout = self.value_type(value)?.layout;
                let record = self.layout(layout)?;
                let V::Mem { addr, .. } = self.get(value)? else {
                    return self.invalid("slice view source");
                };
                let from = self.get1(start)?;
                let to = self.get1(end)?;
                let owner = self.load_ty(PTR, addr, record.fields[0].offset);
                let origin = self.load_ty(types::I64, addr, record.fields[1].offset);
                let length = self.load_ty(types::I64, addr, record.fields[2].offset);
                let reversed = self.b.ins().icmp(IntCC::UnsignedGreaterThan, from, to);
                let outside = self.b.ins().icmp(IntCC::UnsignedGreaterThan, to, length);
                let failed = self.b.ins().bor(reversed, outside);
                self.guard(failed, failure)?;
                let start = self.b.ins().iadd(origin, from);
                let length = self.b.ins().isub(to, from);
                let slice = self.stack(layout)?;
                for (field, word) in record.fields.iter().zip([owner, start, length]) {
                    self.store_at(word, slice.ptr, field.offset);
                }
                let shape = self.layout_shape(layout)?;
                let value = self.load(shape, slice);
                self.define(result, value)
            }
            Inst::FlatView { result, address } => {
                // A borrowed slice of the String or Bytes encoding at `address`.
                let place = self.place(address)?;
                let Some(block) = place.block else {
                    return self.invalid("a flat view whose address crosses no counted block");
                };
                let layout = self.value_type(result)?.layout;
                let record = self.layout(layout)?;
                let payload = self.payload_of(block, place.layout)?;
                let start = self.b.ins().isub(place.ptr, payload.ptr);
                let length = self.load_u64(place.ptr);
                let slice = self.stack(layout)?;
                for field in &record.fields {
                    let word = match field.ordinal {
                        0 => block,
                        1 => start,
                        2 => length,
                        _ => return self.invalid("a slice layout with a fourth word"),
                    };
                    self.store_at(word, slice.ptr, field.offset);
                }
                let shape = self.layout_shape(layout)?;
                let value = self.load(shape, slice);
                self.define(result, value)
            }
            Inst::FlatLength { result, address } => {
                let place = self.place(address)?;
                let count = self.flat_count(place)?;
                self.define1(result, count)
            }
            Inst::FlatNext { result, address } => {
                // The content offset just past the element.
                let place = self.place(address)?;
                let Some(content) = self.flat_contents[address.index()] else {
                    return self.invalid("the end of a place that is no variable flat element");
                };
                let extent = self.extent(place.layout, place.ptr)?;
                let start = self.b.ins().isub(place.ptr, content);
                let end = self.b.ins().iadd(start, extent);
                self.define1(result, end)
            }
            Inst::SeqLen { result, value } => {
                let count = match self.layout(self.value_type(value)?.layout)?.kind {
                    LayoutKind::Block { .. } | LayoutKind::Box { .. } => {
                        let data = match self.layout_payload(self.value_type(value)?.layout)? {
                            // A sequential Packed sequence counts its elements in its first
                            // word too (MEM-13).
                            Some(payload)
                                if matches!(
                                    self.layout(payload)?.kind,
                                    LayoutKind::Sequential { .. }
                                ) =>
                            {
                                let Some(pointer) = self.counted_view(value)?.one() else {
                                    return self.invalid("a length of a value that is no block");
                                };
                                self.payload_of(pointer, payload)?.ptr
                            }
                            _ => self.sequence_payload(value)?.0,
                        };
                        self.load_ty(types::I64, data, 0)
                    }
                    _ => self.slice_bytes(value)?.1,
                };
                self.define1(result, count)
            }
            Inst::SeqCapacity { result, value } => {
                let capacity = self.sequence_capacity(value)?;
                self.define1(result, capacity)
            }
            Inst::SeqReserve {
                address, capacity, ..
            } => self.seq_reserve(address, capacity),
            Inst::SeqAppend { address, value, .. } => {
                // MEM-13: the element's active extent past the used bytes.
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let temp = self.stack(element)?;
                let shape = self.value_shape(value)?;
                let v = self.get(value)?;
                self.store(shape, v, temp)?;
                let bytes = self.extent(element, temp.ptr)?;
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.b.ins().iadd_imm_s(used, first as i64);
                let at = self.offset_by(data, at);
                self.copy_dynamic(at, temp.ptr, bytes);
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::FlatAppend {
                address, construct, ..
            } => {
                // One element written in its flat encoding past the used bytes.
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts(site)?;
                let content = self.flat_extent(&parts)?;
                let eight = self.u64c(8);
                let bytes = self.flat_add(content, eight);
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.b.ins().iadd_imm_s(used, first as i64);
                let at = self.offset_by(data, at);
                self.store_at(content, at, 0);
                let fields = self.offset(at, 8);
                self.write_flat(fields, &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::FlatField {
                address, field, op, ..
            } => self.flat_field(address, field, op),
            Inst::FlatClose {
                address,
                construct,
                field,
                ..
            } => {
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let word = self.open_word(element, field)?;
                let (old, data, used) = self.sequential_used(place, payload)?;
                let start = self.b.ins().iadd_imm_s(used, first as i64);
                let at = self.b.ins().iadd_imm_s(start, word as i64);
                let at_ptr = self.offset_by(data, at);
                let length = self.load_u64(at_ptr);
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts_around(site, Some((field, length)))?;
                let content = self.flat_extent(&parts)?;
                let eight = self.u64c(8);
                let bytes = self.flat_add(content, eight);
                let trailing = !matches!(parts.last(), Some(FlatPart::InPlace { .. }));
                let data = if trailing {
                    let past = self.b.ins().iadd_imm_s(at, 8);
                    let keep = self.b.ins().iadd(past, length);
                    let need = self.b.ins().iadd(start, bytes);
                    self.sequential_grow(place, payload, old, keep, need)?
                } else {
                    data
                };
                let at = self.offset_by(data, start);
                self.store_at(content, at, 0);
                let fields = self.offset(at, 8);
                self.write_flat(fields, &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::SeqSetLen { address, len, .. } => {
                let place = self.place(address)?;
                let pointer = self.load_ptr(place.ptr);
                let Some(payload) = self.layout_payload(place.layout)? else {
                    return self.invalid("a length of a place that holds no block");
                };
                let data = self.payload_of(pointer, payload)?;
                let len = self.get1(len)?;
                self.store_at(len, data.ptr, 0);
                Ok(())
            }
            Inst::Hash {
                result,
                value,
                layout,
                failure,
            } => self.hash(result, value, layout, failure),
            // The same pointer at another payload layout: no code.
            // MEM-11: a fresh root block holding the value, whose blocks the runtime freezes.
            Inst::Freeze { result, value } => {
                let payload = self.frozen_payload(result)?;
                let size = self.fixed_size(payload)?;
                let align = self.layout(payload)?.align;
                let anchor = self.unit.blocks[self.unit.entry.index()].anchor;
                let extent = self.u64c(size);
                let block = self.alloc_block(payload, extent, align, anchor)?;
                let place = self.payload_of(block, payload)?;
                let shape = self.value_shape(value)?;
                let v = self.counted_view(value)?;
                self.store(shape, v, place)?;
                let context = self.context;
                self.rt(RuntimeEntry::Freeze, &[context, block])?;
                self.define1(result, block)
            }
            // The last root: thaw its value, move it out and free the root block shallowly.
            Inst::Thaw { result, value } => {
                let payload = self.frozen_payload(value)?;
                let block = self.get1(value)?;
                let context = self.context;
                self.rt(RuntimeEntry::Thaw, &[context, block])?;
                let place = self.payload_of(block, payload)?;
                let shape = self.value_shape(result)?;
                let thawed = self.load(shape, place);
                self.rt(RuntimeEntry::Free, &[context, block])?;
                self.define(result, thawed)
            }
            // SER-2: the runtime walks the plan; neither can fail.
            Inst::Encode {
                result,
                value,
                codec,
            }
            | Inst::Decode {
                result,
                value,
                codec,
            } => {
                let entry = match inst {
                    Inst::Encode { .. } => RuntimeEntry::CodecEncode,
                    _ => RuntimeEntry::CodecDecode,
                };
                let codec = self.codec(codec)?;
                let layout = self.value_type(value)?.layout;
                let at = self.stack(layout)?;
                let operand = self.counted_view(value)?;
                let shape = self.layout_shape(layout)?;
                self.store(shape, operand, at)?;
                let produced = self.value_type(result)?.layout;
                let out = self.stack(produced)?;
                let context = self.context;
                self.rt(entry, &[context, codec, at.ptr, out.ptr])?;
                let shape = self.layout_shape(produced)?;
                let value = self.load(shape, out);
                self.define(result, value)
            }
            Inst::Recast { result, value } => {
                let pointer = self.get(value)?;
                self.define(result, pointer)
            }
            Inst::Release {
                value,
                layout,
                live,
                ..
            } if self.is_receipt_value(value)? => {
                if self.value_type(value)?.layout != layout || live != MaskRef::All {
                    return self.invalid("a receipt release with another layout or partial mask");
                }
                let receipt = self.get1(value)?;
                self.rt(RuntimeEntry::ReceiptRelease, &[receipt])?;
                Ok(())
            }
            Inst::Reset { value, .. } if self.is_receipt_value(value)? => {
                self.invalid("receipt metadata is never a reuse donor")
            }
            // Mutex roots may run user finalizers on the last release.
            Inst::Release {
                value,
                anchor,
                failure,
                ..
            } if self.is_root_value(value)? => {
                let block = self.get1(value)?;
                let anchor = self.cleanup_code(anchor)?;
                let (anchor, operation_site) = self.failure_sites(anchor);
                let status = self.rt1(
                    RuntimeEntry::RootDrop,
                    &[self.context, block, anchor, operation_site],
                )?;
                if self.resume.is_some() {
                    self.resume_propagate(status)?;
                }
                self.status_guard(status, failure)
            }
            Inst::Reset { value, .. } if self.is_root_value(value)? => {
                self.invalid("a reset of a Shared root, which is never a reuse donor")
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } if self.value_type(value)?.repr != ValueRepr::Block => {
                self.release_inline(value, layout, live, anchor, failure)
            }
            // A constructor-tail box opens at its stand-in layout, which releases
            // nothing; the hole is its block (LANG-10, LIR schema revision 59).
            Inst::HoleOpen {
                result,
                hole,
                construct,
                open,
                ..
            } => {
                let Some(site) = self.unit.tables.constructs.get(construct.index()) else {
                    return self.invalid("a hole opening outside the construction table");
                };
                let anchor = site.anchor;
                let LayoutKind::Box { payload } = self.layout(open)?.kind else {
                    return self.invalid("a hole's stand-in is not a box");
                };
                let size = self.fixed_size(payload)?;
                let align = self.layout(payload)?.align;
                let extent = self.u64c(size);
                let block = self.alloc_block(payload, extent, align, anchor)?;
                self.define1(result, block)?;
                self.define1(hole, block)
            }
            // The value moves into the open payload; the header then takes the box's layout.
            Inst::HoleFill { hole, value } => {
                let LayoutKind::Box { payload } = self.layout(self.value_type(hole)?.layout)?.kind
                else {
                    return self.invalid("a hole of no box");
                };
                let block = self.get1(hole)?;
                let place = self.payload_of(block, payload)?;
                let shape = self.value_shape(value)?;
                let v = self.counted_view(value)?;
                self.store(shape, v, place)?;
                let descriptor = self.descriptor_value(payload)?;
                let context = self.context;
                self.rt(RuntimeEntry::HoleClose, &[context, block, descriptor])?;
                Ok(())
            }
            // The runtime releases the root by its blocks' headers, so no open payload is
            // read; compiled glue, which follows static layouts, never runs here. Nothing
            // it releases is destructible, so its status is success.
            Inst::HoleAbandon { root, .. } => {
                let Some(root) = root else {
                    return Ok(());
                };
                let block = self.get1(root)?;
                let descriptor = self.block_descriptor(block);
                let mask = self.live_mask(MaskRef::All)?;
                let zero = self.u64c(0);
                let context = self.context;
                self.rt1(
                    RuntimeEntry::Release,
                    &[context, block, descriptor, mask, zero, zero],
                )?;
                Ok(())
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } => {
                let block = self.get1(value)?;
                let certified = self.resume.as_ref().is_some_and(|r| {
                    r.plan
                        .cleanups
                        .iter()
                        .any(|s| s.value == value && s.anchor == anchor)
                });
                let descriptor = if certified {
                    self.descriptor_value(layout)?
                } else {
                    self.block_descriptor(block)
                };
                let mask = self.live_mask(live)?;
                let anchor = self.cleanup_code(anchor)?;
                let (anchor, operation_site) = self.failure_sites(anchor);
                let context = self.context;
                let status = self.rt1(
                    RuntimeEntry::Release,
                    &[context, block, descriptor, mask, anchor, operation_site],
                )?;
                if self.resume.is_some() {
                    self.resume_propagate(status)?;
                }
                self.status_guard(status, failure)
            }
            Inst::Reset {
                result,
                value,
                live,
                anchor,
                failure,
                ..
            } => {
                // A callable donates its environment block, which `layout` describes.
                let block = match self.value_type(value)?.repr {
                    ValueRepr::Callable => self.callable_parts(value)?.1,
                    _ => self.get1(value)?,
                };
                let descriptor = self.block_descriptor(block);
                let out = self.out_word();
                let mask = self.live_mask(live)?;
                let anchor = self.cleanup_code(anchor)?;
                let (anchor, operation_site) = self.failure_sites(anchor);
                let context = self.context;
                let status = self.rt1(
                    RuntimeEntry::Reset,
                    &[
                        context,
                        block,
                        descriptor,
                        mask,
                        anchor,
                        operation_site,
                        out,
                    ],
                )?;
                self.status_guard(status, failure)?;
                let token = self.load_ptr(out);
                self.define1(result, token)
            }
            Inst::Reuse {
                result,
                token,
                construct,
                kept,
                failure,
            } => {
                let site = self.construct_site(construct)?;
                let token = self.get1(token)?;
                self.construct_block(result, site, Some((token, kept)), failure)
            }
            Inst::RegionEnter { result, region } => {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                // An arena's region (its mark names an `ArenaToken`) enters warm.
                let arena =
                    self.unit.instructions.iter().any(
                        |inst| matches!(*inst, Inst::ArenaToken { mark, .. } if mark == result),
                    );
                let demanded = self.unit.tables.regions[region.index()].seal_demand;
                let entry = if demanded {
                    RuntimeEntry::ReceiptRegionEnter
                } else if arena {
                    RuntimeEntry::ArenaEnter
                } else {
                    RuntimeEntry::RegionEnter
                };
                let context = self.context;
                self.rt(entry, &[context, record])?;
                self.define1(result, record)
            }
            Inst::RegionReserve {
                result,
                mark,
                layout,
                extent,
                failure,
            } => {
                let size = match self.layout(layout)?.size {
                    SizeSpec::Fixed(size) => self.u64c(size),
                    SizeSpec::Dynamic(_) => self.get1(extent)?,
                };
                let mark = self.get1(mark)?;
                let storage = self.reserve_region(mark, layout, size, failure)?;
                self.define1(result, storage.ptr)
            }
            // A token is its region's record address.
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                let record = self.get1(mark)?;
                // A bounded arena's budget (MEM-6) is set before its token exists.
                if let Some(budget) = budget {
                    let budget = self.get1(budget)?;
                    let context = self.context;
                    self.rt(RuntimeEntry::ArenaBound, &[context, record, budget])?;
                }
                self.define1(result, record)
            }
            Inst::ArenaOpen { .. } => self.invalid("an arena opening after placement"),
            Inst::RegionExit {
                mark,
                region,
                failure,
            } => {
                let Some(instance) = self.unit.tables.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                let anchor = self.anchor_code(instance.anchor)?;
                let mark = self.get1(mark)?;
                let (anchor, operation_site) = self.failure_sites(anchor);
                let context = self.context;
                let entry = if instance.seal_demand {
                    RuntimeEntry::ReceiptRegionExit
                } else {
                    RuntimeEntry::RegionExit
                };
                let status = self.rt1(entry, &[context, mark, anchor, operation_site])?;
                self.status_guard(status, failure)
            }
            Inst::FinalizeBegin { anchor, .. } => {
                let anchor = self.cleanup_code(anchor)?;
                let anchor = self.u64c(anchor);
                let context = self.context;
                self.rt(RuntimeEntry::FinalizeBegin, &[context, anchor])?;
                Ok(())
            }
            Inst::FinalizeEnd { anchor, .. } => {
                let anchor = self.cleanup_code(anchor)?;
                let anchor = self.u64c(anchor);
                let context = self.context;
                self.rt(RuntimeEntry::FinalizeEnd, &[context, anchor])?;
                Ok(())
            }
            Inst::Copy {
                result, address, ..
            } => {
                // A semantic copy: the place's bits, then one count increment per counted
                // holder in scan order.
                let place = self.place(address)?;
                if self.value_type(result)?.layout != place.layout {
                    return self.invalid("a copy at another layout than its place");
                }
                let shape = self.layout_shape(place.layout)?;
                let image = self.gather(place)?;
                let value = self.load(shape, image);
                self.dup_holders(place)?;
                self.define(result, value)
            }
            Inst::BorrowSealedRoot { result, owner, .. } => self.borrow_sealed_root(result, owner),
            Inst::SealArena {
                result,
                grant,
                root,
                shape,
                failure,
                origin,
            } => self.seal_arena(result, grant, root, shape, failure, origin),
            Inst::PayloadReceipt {
                arena,
                result,
                value,
                shape,
                binding,
                inputs,
            } => self.payload_receipt(result, value, shape, binding, (inputs, arena.get())),
            Inst::ArenaInventory {
                receipt,
                arena,
                allocation,
                value,
                shape,
                previous,
            } => self.arena_inventory(arena, allocation, value, shape, (previous, receipt.get())),
            Inst::CopyValue { .. } | Inst::Drop { .. } | Inst::Unavailable { .. } => {
                self.invalid("an operation outside the Ready vocabulary")
            }
        }
    }

    /// Whether the counted block `block` has one holder.
    /// The `Frozen` payload layout of a `Shared` root reference value.
    /// Whether `value` is a `Shared` root reference, whose count only the root entries touch.
    pub(crate) fn is_root_value(&self, value: ValueId) -> Emitted<bool> {
        Ok(matches!(
            self.layout(self.value_type(value)?.layout)?.kind,
            LayoutKind::Shared { .. }
        ))
    }

    fn frozen_payload(&self, value: ValueId) -> Emitted<LayoutId> {
        match self.layout(self.value_type(value)?.layout)?.kind {
            LayoutKind::Shared { payload } => Ok(payload),
            _ => self.invalid("a freeze or thaw of a value that is not a Shared root"),
        }
    }

    pub(crate) fn is_unique(&mut self, block: Value) -> Value {
        let count = self.load_ty(types::I32, block, COUNT_OFFSET);
        self.b
            .ins()
            .icmp_imm_s(IntCC::Equal, count, COUNT_UNIQUE as i64)
    }

    fn seq_reserve(&mut self, address: AddressId, capacity: ValueId) -> Emitted {
        if self.cpu_reserve()? {
            return Ok(());
        }
        let place = self.place(address)?;
        if let Some(payload) = self.layout_payload(place.layout)?
            && let LayoutKind::SoaColumns { element } = self.layout(payload)?.kind
        {
            let wanted = self.get1(capacity)?;
            return self.reserve_columns(place, payload, element, wanted);
        }
        if let Some(payload) = self.layout_payload(place.layout)?
            && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
        {
            // A flat builder's reservation: room for `capacity` of its least element past
            // the count and used words, the bytes saturating.
            let first = self.sequential_offset(element)?;
            let record = self.layout(element)?;
            let least = match (&record.kind, &record.size, record.fields.as_slice()) {
                (LayoutKind::Flat, SizeSpec::Dynamic(_), [content]) => content
                    .offset
                    .saturating_add(hirsel_lir::layout::least_size(
                        &self.layout(content.layout)?.size,
                    )),
                _ => hirsel_lir::layout::least_size(&record.size),
            };
            let capacity = self.get1(capacity)?;
            let least = self.u64c(least);
            let (product, overflow) = self.b.ins().umul_overflow(capacity, least);
            let max = self.u64c(u64::MAX);
            let bytes = self.b.ins().select(overflow, max, product);
            let align = u64::from(self.layout(payload)?.align);
            let old = self.load_ptr(place.ptr);
            let data = self.payload_of(old, payload)?.ptr;
            let used = self.load_ty(types::I64, data, 8);
            let keep = self.b.ins().iadd_imm_s(used, first as i64);
            let first = self.u64c(first);
            let extent = self.saturating_add(64, false, bytes, first);
            let align = self.u64c(align);
            let context = self.context;
            let count = self.load_ty(types::I64, data, 0);
            let empty = self.b.ins().icmp_imm_s(IntCC::Equal, count, 0);
            let vacant = self.b.create_block();
            let occupied = self.b.create_block();
            let done = self.b.create_block();
            self.b.ins().brif(empty, vacant, &[], occupied, &[]);
            self.b.switch_to_block(vacant);
            let one = self.u64c(1);
            let fresh = self.rt1(RuntimeEntry::SeqReserve, &[context, old, one, align, bytes])?;
            let fresh_data = self.payload_of(fresh, payload)?.ptr;
            let zero = self.u64c(0);
            self.store_at(zero, fresh_data, 8);
            self.store_at(fresh, place.ptr, 0);
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(occupied);
            let grown = self.rt1(
                RuntimeEntry::BlockGrow,
                &[context, old, keep, extent, align],
            )?;
            self.store_at(grown, place.ptr, 0);
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(done);
            return Ok(());
        }
        let Some(element) = self.sequence_element(place.layout)? else {
            return self.invalid("a reserve of a place that holds no sequence block");
        };
        let (size, align) = (
            self.fixed_size(element)?,
            u64::from(self.layout(element)?.align),
        );
        let old = self.load_ptr(place.ptr);
        let size = self.u64c(size);
        let align = self.u64c(align);
        let capacity = self.get1(capacity)?;
        let context = self.context;
        let grown = self.rt1(
            RuntimeEntry::SeqReserve,
            &[context, old, size, align, capacity],
        )?;
        self.store_at(grown, place.ptr, 0);
        Ok(())
    }

    fn flat_field(&mut self, address: AddressId, field: u32, op: FlatFieldOp) -> Emitted {
        let (place, payload, element) = self.sequential_place(address)?;
        let first = self.sequential_offset(element)?;
        let word = first + self.open_word(element, field)?;
        let (old, data, used) = self.sequential_used(place, payload)?;
        let at = self.b.ins().iadd_imm_s(used, word as i64);
        if op == FlatFieldOp::Open {
            let need = self.b.ins().iadd_imm_s(at, 8);
            let keep = self.b.ins().iadd_imm_s(used, first as i64);
            let data = self.sequential_grow(place, payload, old, keep, need)?;
            let zero = self.u64c(0);
            let length_at = self.offset_by(data, at);
            self.store_at(zero, length_at, 0);
            return Ok(());
        }
        let length_at = self.offset_by(data, at);
        let length = self.load_u64(length_at);
        let past = self.b.ins().iadd_imm_s(at, 8);
        let keep = self.b.ins().iadd(past, length);
        match op {
            FlatFieldOp::Open => unreachable!("opened above"),
            FlatFieldOp::Reserve { capacity } => {
                let capacity = self.get1(capacity)?;
                let need = self.saturating_add(64, false, keep, capacity);
                self.sequential_grow(place, payload, old, keep, need)?;
            }
            FlatFieldOp::Push { value, spare } => {
                let data = if spare {
                    data
                } else {
                    let need = self.b.ins().iadd_imm_s(keep, 1);
                    self.sequential_grow(place, payload, old, keep, need)?
                };
                let value = self.get1(value)?;
                let byte_at = self.offset_by(data, keep);
                self.store_at(value, byte_at, 0);
                let longer = self.b.ins().iadd_imm_s(length, 1);
                let length_at = self.offset_by(data, at);
                self.store_at(longer, length_at, 0);
            }
        }
        Ok(())
    }

    fn flat_copy(
        &mut self,
        result: ValueId,
        address: AddressId,
        layout: hirsel_lir::layout_types::LayoutId,
        failure: FailureId,
    ) -> Emitted {
        let ty = self.value_type(result)?;
        if ty.layout != layout {
            let source = self.place(address)?;
            let length = self.load_u64(source.ptr);
            let from = self.offset(source.ptr, 8);
            let anchor = self.failure_edge(failure)?.anchor;
            match self.layout(ty.layout)?.kind.clone() {
                LayoutKind::Block { payload } => {
                    let align = self.layout(payload)?.align;
                    let owner = self.alloc_block(payload, length, align, anchor)?;
                    let to = self.payload_of(owner, payload)?;
                    self.copy_dynamic(to.ptr, from, length);
                    return self.define1(result, owner);
                }
                LayoutKind::Slice { element } => {
                    // A whole fresh byte sequence: its count, its capacity (the count) and
                    // the bytes.
                    let Some(payload) = slice_storage_with_index(
                        self.em.layouts(),
                        self.em.inputs.unit.and_then(|i| i.index),
                        element,
                    ) else {
                        return self.invalid("a flat copy slice without sequence storage");
                    };
                    let (first, whole) = match (
                        sequence_elements_offset(self.layout(element)?),
                        sequence_slice_start(self.layout(element)?),
                    ) {
                        (Ok(first), Ok(whole)) => (first, whole),
                        _ => return self.invalid("a byte sequence offset beyond u64"),
                    };
                    let extent = self.b.ins().iadd_imm_s(length, first as i64);
                    let align = self.layout(payload)?.align;
                    let owner = self.alloc_block(payload, extent, align, anchor)?;
                    let to = self.payload_of(owner, payload)?;
                    self.store_at(length, to.ptr, 0);
                    self.store_at(length, to.ptr, SEQUENCE_CAPACITY_OFFSET);
                    let data = self.offset(to.ptr, first);
                    self.copy_dynamic(data, from, length);
                    let slice = self.stack(ty.layout)?;
                    let whole = self.u64c(whole);
                    for field in &self.layout(ty.layout)?.fields {
                        let word = match field.ordinal {
                            0 => owner,
                            1 => whole,
                            2 => length,
                            _ => return self.invalid("a flat copy slice with a fourth word"),
                        };
                        self.store_at(word, slice.ptr, field.offset);
                    }
                    let shape = self.layout_shape(ty.layout)?;
                    let value = self.load(shape, slice);
                    return self.define(result, value);
                }
                _ => return self.invalid("a flat copy into another representation"),
            }
        }
        match (self.layout(layout)?.kind.clone(), ty.repr) {
            // A fixed flat extent is pointer-free bytes: the copy is its bytes, inline.
            (LayoutKind::Flat, ValueRepr::Aggregate) => {
                let place = self.place(address)?;
                let shape = self.layout_shape(layout)?;
                let image = self.gather(place)?;
                let value = self.load(shape, image);
                self.define(result, value)
            }
            // A variable flat value is a block of its extent: the copy is a new heap block
            // holding the same bytes.
            (LayoutKind::Block { payload } | LayoutKind::Box { payload }, ValueRepr::Block)
                if matches!(self.layout(payload)?.kind, LayoutKind::Flat) =>
            {
                let place = self.place(address)?;
                let block = self.load_ptr(place.ptr);
                // An uncounted region slot already holds the payload address.
                let regional = self.lir_type(self.unit.addresses[address.index()].ty)?.repr
                    == ValueRepr::Uncounted;
                let from = if regional {
                    self.uncounted_payload(block, payload)?
                } else {
                    self.payload_of(block, payload)?
                };
                let extent = self.extent(payload, from.ptr)?;
                let anchor = self.failure_edge(failure)?.anchor;
                let copy = self.alloc_block(payload, extent, from.align, anchor)?;
                let to = self.payload_of(copy, payload)?;
                self.copy_dynamic(to.ptr, from.ptr, extent);
                self.define1(result, copy)
            }
            _ => self.invalid("a flat copy of a layout that is not flat"),
        }
    }

    fn unique_copy(
        &mut self,
        result: ValueId,
        value: ValueId,
        construct: ConstructId,
        deep: bool,
        failure: FailureId,
    ) -> Emitted {
        if self.cpu_reserve()? {
            return Ok(());
        }
        let site = self.construct_site(construct)?;
        let ty = self.value_type(value)?;
        if !deep
            && !matches!(
                site.placement,
                Some(Placement::Fixed(StorageOwner::Heap(_)))
            )
        {
            return self.unsupported("a shallow detach outside the heap");
        }
        let destination = match site.placement {
            Some(Placement::Fixed(StorageOwner::Heap(_))) => self.b.ins().iconst(PTR, 0),
            Some(Placement::Fixed(StorageOwner::Arena(id))) => {
                let instance = &self.unit.tables.arenas[id.index()];
                let Some(slot) = self.slots[instance.slot.index()] else {
                    return self.invalid("arena token slot without storage");
                };
                self.load_ptr(slot.ptr)
            }
            _ => return self.unsupported("a unique copy outside heap or arena storage"),
        };
        // An inline value holding blocks is copied in place: a bit copy in a slot of its
        // layout whose holders the runtime rewrites to deep copies.
        if deep && matches!(ty.repr, ValueRepr::Aggregate | ValueRepr::Callable) {
            let shape = self.layout_shape(ty.layout)?;
            let slot = self.stack(ty.layout)?;
            let image = self.get(value)?;
            self.store(shape, image, slot)?;
            let descriptor = self.descriptor_value(ty.layout)?;
            let anchor = site.anchor;
            let site = self.anchor_code(anchor)?;
            let site = self.u64c(site);
            let context = self.context;
            let status = self.rt1(
                RuntimeEntry::UniqueCopyValue,
                &[context, descriptor, slot.ptr, destination, site],
            )?;
            self.copy_status(status, destination, anchor, failure)?;
            let copy = self.load(shape, slot);
            return self.define(result, copy);
        }
        if deep
            && matches!(
                ty.repr,
                ValueRepr::Unit | ValueRepr::Scalar(_) | ValueRepr::Handle | ValueRepr::Arena
            )
        {
            let v = self.get(value)?;
            return self.define(result, v);
        }
        if ty.repr != ValueRepr::Block {
            return self.invalid("a unique copy of a value that is not a block");
        }
        let out = self.out_word();
        let anchor = site.anchor;
        let flags = if self.tracks(site) {
            hirsel_lir::runtime_abi::ALLOC_TRACKED
        } else {
            0
        };
        let site = self.anchor_code(anchor)?;
        let site = self.u64c(site);
        let block = self.get1(value)?;
        let context = self.context;
        if deep {
            let flags = self.b.ins().iconst(types::I32, i64::from(flags));
            let status = self.rt1(
                RuntimeEntry::UniqueCopy,
                &[context, block, destination, flags, site, out],
            )?;
            self.copy_status(status, destination, anchor, failure)?;
        } else {
            let status = self.rt1(RuntimeEntry::Detach, &[context, block, site, out])?;
            // A tracked arena block is copied within its arena, charged (MEM-6): over its
            // budget, the arena it names records its own OutOfSpace.
            if self.tracking() {
                self.tracked_exhaustion(status, block, anchor, failure)?;
            } else {
                self.status_guard(status, failure)?;
            }
        }
        let copy = self.load_ptr(out);
        self.define1(result, copy)
    }

    /// A deep copy's status: with bounded arenas, `STATUS_EXHAUSTED` records the
    /// destination arena's own `OutOfSpace` and takes `failure`.
    pub(crate) fn copy_status(
        &mut self,
        status: Value,
        destination: Value,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted {
        if !self.tracking() {
            return self.status_guard(status, failure);
        }
        let exhausted = self.exhausted_block(destination, anchor, failure)?;
        self.branch_to(status, exhausted)
    }

    /// A pointer-sized scalar loaded from `place`.
    #[allow(dead_code)]
    pub(crate) fn load_pointer_at(&mut self, place: Place) -> Value {
        match self.load(Shape::Scalar(ScalarKind::Pointer), place) {
            V::One(v) => v,
            _ => unreachable!("a pointer loads as one value"),
        }
    }
}
