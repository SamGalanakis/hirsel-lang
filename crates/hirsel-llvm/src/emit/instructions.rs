use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Instructions.

    /// The `Frozen` payload layout of a `Shared` root reference value.
    fn frozen_payload(&self, value: ValueId) -> Emitted<LayoutId> {
        match self.layout(self.value_type(value)?.layout)?.kind {
            LayoutKind::Shared { payload } => Ok(payload),
            _ => self.invalid("a freeze or thaw of a value that is not a Shared root"),
        }
    }

    /// Whether a block of covered payload `layout` under the static live mask `mask` leaves
    /// its reset or release nothing to run (`rt_program::masked_inert`): every field with
    /// drop steps has moved out.
    fn inert_mask(&self, layout: LayoutId, mask: MaskId) -> Emitted<bool> {
        if !crate::glue::covered(self.inputs.layouts, layout) {
            return Ok(false);
        }
        let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
            return self.invalid("a mask outside the mask table");
        };
        let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
        match hirsel_lir::rt_program::masked_inert(self.inputs.layouts, layout, words) {
            Ok(inert) => Ok(inert),
            Err(error) => self.invalid(format!("the drop program of a reset: {error:?}")),
        }
    }

    /// T12 Reset through `hirsel_rt_v2_reset`: the token, or null for a shared donor.
    fn reset_call(
        &mut self,
        block: Value<'m>,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        let reset = self.runtime(RuntimeEntry::Reset)?;
        let descriptor = self.block_descriptor(block);
        let out = self.prologue.alloca(self.module.ptr_type(), 8);
        let (anchor, operation_site) = self.failure_sites(self.cleanup_code(anchor)?);
        let arguments = [
            self.context,
            block,
            descriptor,
            self.live_mask(live)?,
            anchor,
            operation_site,
            out,
        ];
        let status = self.builder.call(reset, &arguments);
        self.status_guard(status, failure)?;
        Ok(self.builder.load(self.module.ptr_type(), out, 8))
    }

    /// T12 Reset of a block of covered payload `layout`, fully live or with every counted field
    /// moved out ([`Self::inert_mask`]). While the context frees inline, a unique heap donor
    /// runs its drop program through compiled glue, in the walk's order, and is the token as
    /// it is: the runtime would only record the state that diagnostic mode, measurement
    /// builds, the system allocator and memcheck keep, and those turn the inline paths off.
    /// If a finalizer below it fails, its storage is freed and no token escapes, as in
    /// `hirsel_rt_v2_reset`. Every other donor goes to the runtime.
    fn reset_glue(
        &mut self,
        block: Value<'m>,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        use hirsel_lir::runtime_abi::{
            COUNT_OFFSET, COUNT_UNIQUE, CTX_FREE_INLINE_OFFSET, KIND_SHIFT, KIND_STATIC,
            LAYOUT_OFFSET,
        };
        let (ptr, i32_ty) = (self.module.ptr_type(), self.module.int_type(32));
        let half = |n: u64| self.module.const_int(i32_ty, u128::from(n));
        let at = |base, offset| self.builder.offset(base, self.const_u64(offset));
        let enabled = self
            .builder
            .load(i32_ty, at(self.context, CTX_FREE_INLINE_OFFSET), 4);
        let count = self.builder.load_access(
            i32_ty,
            at(block, COUNT_OFFSET),
            4,
            crate::context::Access::Count,
        );
        let descriptor = self.builder.load(i32_ty, at(block, LAYOUT_OFFSET), 4);
        let kind = self
            .builder
            .binary(BinOp::LShr, descriptor, half(u64::from(KIND_SHIFT)));
        let heap = self
            .builder
            .icmp(IntPredicate::Ult, kind, half(u64::from(KIND_STATIC)));
        let unique = self
            .builder
            .icmp(IntPredicate::Eq, count, half(COUNT_UNIQUE));
        let enabled = self.builder.icmp(IntPredicate::Ne, enabled, half(0));
        let fast = self.builder.binary(
            BinOp::And,
            self.builder.binary(BinOp::And, enabled, unique),
            heap,
        );
        let [inline, slow, done] = std::array::from_fn(|_| self.module.append_block(self.function));
        self.builder.cond_br(fast, inline, slow);
        self.builder.position(slow);
        let token = self.reset_call(block, live, anchor, failure)?;
        let from_slow = self.builder.current();
        self.builder.br(done);
        self.builder.position(inline);
        let record = self.layout(layout)?;
        let mut kept = block;
        if live == MaskRef::All && !record.drop.is_empty() {
            let payload = self.payload_of(block, layout)?;
            let code = self.const_u64(self.cleanup_code(anchor)?);
            let scope = glue::Scope::of(layout);
            let status = self.drop_glue(code, |walk| {
                walk.steps(&scope, payload.ptr, payload.align, &record.drop)
            })?;
            let failed = self.builder.icmp(
                IntPredicate::Ne,
                status,
                self.module.const_int(self.module.int_type(8), 0),
            );
            let [dispose, guard] = std::array::from_fn(|_| self.module.append_block(self.function));
            self.builder.cold_cond_br(failed, dispose, guard);
            self.builder.position(dispose);
            let free = self.runtime(RuntimeEntry::Free)?;
            self.builder.call(free, &[self.context, block]);
            self.builder.br(guard);
            self.builder.position(guard);
            kept = self
                .builder
                .select(failed, self.module.const_zero(ptr), block);
            self.status_guard(status, failure)?;
        }
        let from_inline = self.builder.current();
        self.builder.br(done);
        self.builder.position(done);
        let phi = self.builder.phi(ptr);
        self.builder.add_incoming(phi, kept, from_inline);
        self.builder.add_incoming(phi, token, from_slow);
        Ok(phi)
    }

    pub(super) fn scalar_kind(&self, value: ValueId) -> Emitted<ScalarKind> {
        match self.value_shape(value)? {
            Shape::Scalar(kind) => Ok(kind),
            _ => self.invalid(format!("value {} is not a scalar", value.0)),
        }
    }

    /// Brings deferred count words up to date before `inst` where it may read one
    /// from memory: a block moving or copied out of its slot, a view of unknown
    /// origin, or anything else that reaches blocks (runtime entries, structural
    /// operations). Instructions that read no count, and scalar arithmetic, keep
    /// the deferral, so a push loop stores its lengths once, after the loop.
    pub(super) fn flush_before(&mut self, inst: &Inst) -> Emitted {
        let whole = |address| {
            self.whole_slot(address)
                .map(SlotId::index)
                .filter(|&s| self.deferred[s])
        };
        match *inst {
            Inst::Const { .. }
            | Inst::FunctionRef { .. }
            | Inst::ReadStatic { .. }
            | Inst::Unary { .. }
            | Inst::Binary { .. }
            | Inst::Convert { .. }
            | Inst::IntArith { .. }
            | Inst::MulAdd { .. }
            | Inst::ScalarUnary { .. }
            | Inst::Check { .. }
            | Inst::StorageLive { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave
            | Inst::Forget { .. }
            | Inst::Weaken { .. }
            | Inst::IsUnique { .. }
            | Inst::IsWritable { .. }
            | Inst::Share { .. }
            | Inst::ArenaPush { .. }
            | Inst::SeqCapacity { .. }
            | Inst::SeqSetLen { .. }
            | Inst::Dup { .. }
            | Inst::UpdateMask { .. }
            | Inst::TokenNone { .. }
            | Inst::Construct { .. }
            | Inst::Closure { .. }
            | Inst::Release { .. }
            | Inst::Reset { .. }
            | Inst::Reuse { .. }
            | Inst::Dispose { .. }
            | Inst::Drop { .. }
            | Inst::Unavailable { .. }
            | Inst::Store { .. }
            | Inst::Address { .. } => Ok(()),
            // Moved, copied or exchanged out of its slot: the block leaves with its
            // count current.
            Inst::Read { address, mode, .. } => match whole(address) {
                Some(slot) if mode != ReadMode::Borrow => self.flush_length(slot),
                _ => Ok(()),
            },
            Inst::Copy { address, .. } | Inst::Replace { address, .. } => match whole(address) {
                Some(slot) => self.flush_length(slot),
                None => Ok(()),
            },
            Inst::StorageDead { slot, .. } => self.flush_length(slot.index()),
            Inst::SeqLen { value, .. } => {
                if self.preserved_lengths[value.index()].is_some()
                    || self.value_type(value)?.repr != ValueRepr::Block
                {
                    return Ok(());
                }
                match self.layout_payload(self.value_type(value)?.layout)? {
                    Some(payload) => self.flush_viewed(payload),
                    None => Ok(()),
                }
            }
            _ => self.flush_lengths(true),
        }
    }

    pub(super) fn instruction(&mut self, inst: &Inst) -> Emitted {
        // A shallow copy can add a child credit; regional cleanup can run user
        // finalizers. Neither may leave a previously observed uniqueness fact.
        if matches!(
            inst,
            Inst::UniqueCopy { .. }
                | Inst::RegionExit { .. }
                | Inst::Freeze { .. }
                | Inst::Thaw { .. }
        ) {
            self.invalidate_uniqueness();
        }
        if let Inst::Release { layout, .. } | Inst::Reset { layout, .. } = *inst {
            let record = self.layout(layout)?;
            if record.finalizer.is_some() || !record.drop.is_empty() {
                self.invalidate_uniqueness();
                self.flush_lengths(true)?;
            }
        }
        self.flush_before(inst)?;
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
            // Its environment allocation cannot fail (runtime ABI 3), so the failure edge is
            // never taken.
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
                self.read_roots[result.index()] = self.read_roots[value.index()];
                self.preserved_lengths[result.index()] = self.preserved_lengths[value.index()];
                let value = self.get(value)?;
                self.define(result, value)
            }
            Inst::IsUnique { result, value } => {
                let state =
                    self.read_roots[value.index()].and_then(|s| self.sequence_states[s.index()]);
                let unique = if let Some(state) = state {
                    let known = self.builder.load(self.module.int_type(1), state.unique, 1);
                    let here = self.builder.current();
                    let test = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    self.builder.cond_br(known, done, test);
                    self.builder.position(test);
                    let tested = crate::runtime::is_unique(&self.builder, self.get(value)?);
                    self.builder.store(tested, state.unique, 1);
                    self.mark_untracked(state, tested);
                    self.builder.br(done);
                    self.builder.position(done);
                    let unique = self.builder.phi(self.module.int_type(1));
                    self.builder.add_incoming(unique, known, here);
                    self.builder.add_incoming(unique, tested, test);
                    unique
                } else {
                    crate::runtime::is_unique(&self.builder, self.get(value)?)
                };
                self.define(result, unique)
            }
            // A must-unique fact is a writable block's; a test that fails it caches
            // whether the block is unique.
            Inst::IsWritable { result, value } => {
                let state =
                    self.read_roots[value.index()].and_then(|s| self.sequence_states[s.index()]);
                let writable = if let Some(state) = state {
                    let known = self.builder.load(self.module.int_type(1), state.unique, 1);
                    let here = self.builder.current();
                    let test = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    self.builder.cond_br(known, done, test);
                    self.builder.position(test);
                    let block = self.get(value)?;
                    // An untracked block is writable exactly when it is unique.
                    let untracked = if self.tracking() {
                        self.builder
                            .load(self.module.int_type(1), state.untracked, 1)
                    } else {
                        self.module.const_int(self.module.int_type(1), 1)
                    };
                    let unique = crate::runtime::is_unique(&self.builder, block);
                    self.builder.store(unique, state.unique, 1);
                    self.mark_untracked(state, unique);
                    let writable = crate::runtime::is_writable(&self.builder, block);
                    let tested = self.builder.select(untracked, unique, writable);
                    self.builder.br(done);
                    self.builder.position(done);
                    let writable = self.builder.phi(self.module.int_type(1));
                    self.builder.add_incoming(writable, known, here);
                    self.builder.add_incoming(writable, tested, test);
                    writable
                } else if self.tracking() {
                    crate::runtime::is_writable(&self.builder, self.get(value)?)
                } else {
                    crate::runtime::is_unique(&self.builder, self.get(value)?)
                };
                self.define(result, writable)
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
                let operand = self.get(operand)?;
                let mut guard = self.guard_for(failure)?;
                let value = scalar::unary(&self.builder, op, kind, operand, &mut guard)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
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
                let (l, r) = (self.get(left)?, self.get(right)?);
                let mut guard = self.guard_for(failure)?;
                let value = scalar::binary(&self.builder, op, kind, l, r, &mut guard)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::Convert {
                result,
                flag,
                operand,
                mode,
            } => {
                let (from, to) = (self.scalar_kind(operand)?, self.scalar_kind(result)?);
                let operand = self.get(operand)?;
                let (value, status) = scalar::convert(&self.builder, mode, from, to, operand)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)?;
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
                let (l, r) = (self.get(left)?, self.get(right)?);
                let (value, overflow) = scalar::int_arith(&self.builder, op, mode, kind, l, r)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)?;
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
                let operands = [self.get(left)?, self.get(right)?, self.get(addend)?];
                let value = scalar::mul_add(&self.builder, kind, operands)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::ScalarUnary {
                result,
                op,
                operand,
            } => {
                let kind = self.scalar_kind(operand)?;
                let operand = self.get(operand)?;
                let value = scalar::scalar_unary(&self.builder, op, kind, operand)
                    .or_else(|what| self.invalid(what))?;
                self.define(result, value)
            }
            Inst::Check { condition, failure } => {
                let condition = self.get(condition)?;
                let one = self.module.const_int(self.module.int_type(1), 1);
                let failed = self.builder.binary(BinOp::Xor, condition, one);
                self.guard(failed, failure)
            }
            Inst::FrameEnter { failure }
                if self.room.enters.contains(&self.instruction_index()) =>
            {
                let failed = self.no_room();
                self.guard(failed, failure)
            }
            Inst::FrameLeave if self.room.leaves.contains(&self.instruction_index()) => Ok(()),
            Inst::FrameEnter { failure }
                if self.counters.enters.contains(&self.instruction_index()) =>
            {
                self.counter_enter(failure)
            }
            Inst::FrameLeave if self.counters.leaves.contains(&self.instruction_index()) => {
                self.counter_leave();
                Ok(())
            }
            Inst::FrameEnter { failure } => {
                let anchor = self.unit.tables.failures[failure.index()].anchor;
                let (caller, operation) = self.failure_sites(self.anchor_code(anchor)?);
                let entry = self.runtime(RuntimeEntry::DepthEnterAtSite)?;
                let status = self.builder.call(entry, &[self.context, caller, operation]);
                let failed = self.builder.icmp(
                    crate::context::IntPredicate::Ne,
                    status,
                    self.module.const_int(self.module.int_type(8), 0),
                );
                self.guard(failed, failure)
            }
            Inst::FrameLeave => {
                let leave = self.runtime(RuntimeEntry::DepthLeave)?;
                self.builder.call(leave, &[self.context]);
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
            | Inst::EndOrigin { .. } => Ok(()),
            // Explicit suppression: the credit is abandoned and nothing is released.
            Inst::Forget { .. } => Ok(()),
            Inst::Address { address, failure } => {
                self.flat_content = None;
                let place = self.prepare(address, failure)?;
                self.addresses[address.index()] = Some(place);
                self.flat_contents[address.index()] = self.flat_content.take();
                Ok(())
            }
            Inst::Read {
                result,
                address,
                mode,
                ..
            } => {
                let place = self.place(address)?;
                let value = self.load_bounded(self.value_shape(result)?, place)?;
                let value =
                    self.read_uncounted(value, self.unit.values[result.index()].ty, address)?;
                if let Some(slot) = self.whole_slot(address)
                    && let Some(state) = self.sequence_states[slot.index()]
                {
                    if mode == ReadMode::Borrow {
                        self.read_roots[result.index()] = Some(slot);
                    }
                    self.preserved_lengths[result.index()] =
                        Some(self.builder.load(self.module.int_type(64), state.length, 8));
                    if mode == ReadMode::Take {
                        self.settled(slot.index());
                    }
                }
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
                self.scatter(place, self.value_shape(value)?, stored)?;
                if let Some(slot) = self.whole_slot(address) {
                    self.settled(slot.index());
                }
                self.store_sequence_state(address, value)
            }
            Inst::Replace {
                address,
                value,
                displaced,
                ..
            } => {
                // The exchange: old content to the displaced slot, then the new content in,
                // with nothing that can fail or call between the two stores.
                let place = self.place(address)?;
                let shape = self.value_shape(value)?;
                let old = self.load(shape, self.gather(place)?);
                let Some(Some(slot)) = self.slots.get(displaced.index()).copied() else {
                    return self.invalid("a Replace without its displaced slot");
                };
                let old =
                    self.read_uncounted(old, self.unit.slots[displaced.index()].ty, address)?;
                self.store(shape, old, slot);
                let stored = if self.whole_slot(address).is_some() {
                    self.get(value)?
                } else {
                    self.counted_view(value)?
                };
                self.scatter(place, shape, stored)?;
                if let Some(state) = self.sequence_states[displaced.index()] {
                    let length = self.sequence_length(old, slot.layout)?;
                    self.builder.store(length, state.length, 8);
                    self.builder.store(
                        self.module.const_zero(self.module.int_type(1)),
                        state.unique,
                        1,
                    );
                    self.builder.store(
                        self.module.const_zero(self.module.int_type(1)),
                        state.untracked,
                        1,
                    );
                }
                self.settled(displaced.index());
                if let Some(slot) = self.whole_slot(address) {
                    self.settled(slot.index());
                }
                self.store_sequence_state(address, value)
            }
            Inst::UpdateMask {
                result,
                mask,
                field,
                initialized,
            } => {
                let Shape::Scalar(kind) = self.value_shape(mask)? else {
                    return self.invalid("a mask that is not a scalar");
                };
                let Some((width, _)) = scalar::integer(kind) else {
                    return self.invalid("a mask that is not an integer");
                };
                let fields = match self.value_type(mask)?.repr {
                    ValueRepr::InitMask { fields } => fields,
                    _ => width,
                };
                if fields > hirsel_lir::ir::LIVE_MASK_WORD_BITS {
                    return self.unsupported(hirsel_lir::ir::live_mask_word_limit(
                        "initialization mask",
                        fields as usize,
                    ));
                }
                if field >= width {
                    return self.unsupported(format!("initialization mask requires bit {field}; its physical word has {width} bits"));
                }
                let ty = self.module.int_type(width);
                let bit = self.module.const_int(ty, 1u128 << field);
                let current = self.get(mask)?;
                let value = if initialized {
                    self.builder.binary(BinOp::Or, current, bit)
                } else {
                    let cleared = self.module.const_int(ty, !(1u128 << field));
                    self.builder.binary(BinOp::And, current, cleared)
                };
                self.define(result, value)
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
            } => {
                let ty = self.value_type(result)?;
                if ty.layout != layout {
                    let source = self.place(address)?;
                    let length = self.load_u64(source.ptr);
                    let from = self.builder.offset(source.ptr, self.const_u64(8));
                    let anchor = self.failure_edge(failure)?.anchor;
                    match self.layout(ty.layout)?.kind.clone() {
                        LayoutKind::Block { payload } => {
                            let align = self.layout(payload)?.align;
                            let owner = self.alloc_block(payload, length, align, anchor)?;
                            let to = self.payload_of(owner, payload)?;
                            self.builder.memcpy(to.ptr, to.align, from, 1, length);
                            return self.define(result, owner);
                        }
                        LayoutKind::Slice { element } => {
                            // A whole fresh byte sequence: its count, its capacity (the
                            // count) and the bytes (runtime ABI 5).
                            let Some(payload) = slice_storage_with_index(
                                self.inputs.layouts,
                                self.inputs.index,
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
                            let extent =
                                self.builder
                                    .binary(BinOp::Add, length, self.const_u64(first));
                            let align = self.layout(payload)?.align;
                            let owner = self.alloc_block(payload, extent, align, anchor)?;
                            let to = self.payload_of(owner, payload)?;
                            self.builder.store(length, to.ptr, to.align);
                            let capacity = self
                                .builder
                                .offset(to.ptr, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
                            self.builder
                                .store_access(length, capacity, 8, Access::Capacity);
                            let data = self.builder.offset(to.ptr, self.const_u64(first));
                            self.builder.memcpy(data, 1, from, 1, length);
                            let slice = self.stack(ty.layout)?;
                            for field in &self.layout(ty.layout)?.fields {
                                let at =
                                    self.builder.offset(slice.ptr, self.const_u64(field.offset));
                                let word = match field.ordinal {
                                    0 => owner,
                                    1 => self.const_u64(whole),
                                    2 => length,
                                    _ => {
                                        return self
                                            .invalid("a flat copy slice with a fourth word");
                                    }
                                };
                                self.builder.store(word, at, slice.align.min(8));
                            }
                            let value = self.load(self.layout_shape(ty.layout)?, slice);
                            return self.define(result, value);
                        }
                        _ => return self.invalid("a flat copy into another representation"),
                    }
                }
                match (self.layout(layout)?.kind.clone(), ty.repr) {
                    // A fixed flat extent is pointer-free bytes: the copy is its bytes, inline
                    // (C types the result at the flat layout itself).
                    (LayoutKind::Flat, ValueRepr::Aggregate) => {
                        let value = self.load(self.layout_shape(layout)?, self.place(address)?);
                        self.define(result, value)
                    }
                    // A variable flat value is a block of its extent: the copy is a new heap
                    // block holding the same bytes.
                    (
                        LayoutKind::Block { payload } | LayoutKind::Box { payload },
                        ValueRepr::Block,
                    ) if matches!(self.layout(payload)?.kind, LayoutKind::Flat) => {
                        let place = self.place(address)?;
                        let block =
                            self.builder
                                .load(self.module.ptr_type(), place.ptr, place.align);
                        // Region slots hold payload pointers, while counted slots
                        // hold header pointers. The copy must start at the payload
                        // in either representation.
                        let source_ty = self.lir_type(self.unit.addresses[address.index()].ty)?;
                        let from = if source_ty.repr == ValueRepr::Uncounted {
                            self.uncounted_payload(block, payload)?
                        } else {
                            self.payload_of(block, payload)?
                        };
                        let extent = self.extent(payload, from.ptr)?;
                        let anchor = self.failure_edge(failure)?.anchor;
                        let copy = self.alloc_block(payload, extent, from.align, anchor)?;
                        let to = self.payload_of(copy, payload)?;
                        self.builder
                            .memcpy(to.ptr, to.align, from.ptr, from.align, extent);
                        self.define(result, copy)
                    }
                    _ => self.invalid("a flat copy of a layout that is not flat"),
                }
            }
            Inst::UniqueCopy {
                result,
                value,
                construct,
                deep,
                failure,
            } => {
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
                    Some(Placement::Fixed(StorageOwner::Heap(_))) => {
                        self.module.const_zero(self.module.ptr_type())
                    }
                    Some(Placement::Fixed(StorageOwner::Arena(id))) => {
                        let instance = &self.unit.tables.arenas[id.index()];
                        let slot = self.slots[instance.slot.index()].ok_or_else(|| {
                            CodegenError::Invalid {
                                definition: self.definition().clone(),
                                what: "arena token slot without storage".into(),
                            }
                        })?;
                        self.load(Shape::Scalar(ScalarKind::Pointer), slot)
                    }
                    _ => return self.unsupported("a unique copy outside heap or arena storage"),
                };
                // An inline value (an aggregate, variant or inline array holding blocks) is
                // copied in place: a bit copy in a slot of its layout whose holders the
                // runtime rewrites to deep copies.
                if deep && matches!(ty.repr, ValueRepr::Aggregate | ValueRepr::Callable) {
                    let shape = self.layout_shape(ty.layout)?;
                    let slot = self.stack(ty.layout)?;
                    let image = self.get(value)?;
                    self.store(shape, image, slot);
                    let entry = self.runtime(RuntimeEntry::UniqueCopyValue)?;
                    let descriptor = self.descriptor(ty.layout)?;
                    let anchor = site.anchor;
                    let site = self.const_u64(self.anchor_code(anchor)?);
                    let status = self.builder.call(
                        entry,
                        &[self.context, descriptor, slot.ptr, destination, site],
                    );
                    self.copy_status(status, destination, anchor, failure)?;
                    let copy = self.load(shape, slot);
                    return self.define(result, copy);
                }
                if deep
                    && matches!(
                        ty.repr,
                        ValueRepr::Unit
                            | ValueRepr::Scalar(_)
                            | ValueRepr::Handle
                            | ValueRepr::Arena
                    )
                {
                    return self.define(result, self.get(value)?);
                }
                if ty.repr != ValueRepr::Block {
                    return self.invalid("a unique copy of a value that is not a block");
                }
                // Whether the copy is no tracked arena block (MEM-6): a deep copy's root is
                // tracked only when its site says so and it lands in an arena; a detach is as
                // tracked as its original, read before the call.
                let untracked = if !self.tracking() || deep && !self.tracks(site) {
                    self.module.const_int(self.module.int_type(1), 1)
                } else if deep {
                    self.builder.icmp(
                        IntPredicate::Eq,
                        destination,
                        self.module.const_zero(self.module.ptr_type()),
                    )
                } else {
                    let known = match self.read_roots[value.index()]
                        .and_then(|s| self.sequence_states[s.index()])
                    {
                        Some(state) => {
                            self.builder
                                .load(self.module.int_type(1), state.untracked, 1)
                        }
                        None => self.module.const_zero(self.module.int_type(1)),
                    };
                    self.untracked(known, self.get(value)?)
                };
                self.detach_untracked.insert(result, untracked);
                // A shallow detach preserves the entire sequence payload. Save
                // the old length in SSA before the opaque allocation call.
                let old_length = if !deep {
                    match self.layout(ty.layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload }
                            if matches!(
                                self.layout(payload)?.kind,
                                LayoutKind::Sequence { .. }
                            ) =>
                        {
                            let data = self.payload_of(self.get(value)?, payload)?;
                            Some((
                                payload,
                                self.preserved_lengths[value.index()].unwrap_or_else(|| {
                                    self.builder.load_access(
                                        self.module.int_type(64),
                                        data.ptr,
                                        8,
                                        Access::Length,
                                    )
                                }),
                            ))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((_, length)) = old_length {
                    self.preserved_lengths[result.index()] = Some(length);
                }
                let entry = self.runtime(if deep {
                    RuntimeEntry::UniqueCopy
                } else {
                    RuntimeEntry::Detach
                })?;
                let out = self.prologue.alloca(self.module.ptr_type(), 8);
                let anchor = site.anchor;
                let flags = if self.tracks(site) {
                    hirsel_lir::runtime_abi::ALLOC_TRACKED
                } else {
                    0
                };
                let site = self.const_u64(self.anchor_code(anchor)?);
                let arguments = if deep {
                    let flags = self
                        .module
                        .const_int(self.module.int_type(32), u128::from(flags));
                    vec![
                        self.context,
                        self.get(value)?,
                        destination,
                        flags,
                        site,
                        out,
                    ]
                } else {
                    vec![self.context, self.get(value)?, site, out]
                };
                let status = self.builder.call(entry, &arguments);
                if deep {
                    self.copy_status(status, destination, anchor, failure)?;
                } else if self.tracking() {
                    // A tracked arena block is copied within its arena, charged (MEM-6):
                    // over its budget, the arena it names records its own OutOfSpace.
                    let zero = self.module.const_int(self.module.int_type(8), 0);
                    let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
                    let (over, next) = (
                        self.module.append_block(self.function),
                        self.module.append_block(self.function),
                    );
                    self.builder.cold_cond_br(failed, over, next);
                    self.builder.position(over);
                    let region = self.tracked_region(self.get(value)?);
                    let exhausted = self.exhausted_block(region, anchor, failure)?;
                    self.builder.br(exhausted);
                    self.builder.position(next);
                } else {
                    self.status_guard(status, failure)?;
                }
                let copy = self.builder.load(self.module.ptr_type(), out, 8);
                if !deep {
                    // copy.rs::hirsel_rt_v2_detach allocates count-one storage, or
                    // an unshared tracked block within an arena (MEM-6), copies the
                    // payload verbatim and only increments children.
                    // Its result joins through local storage-version tokens,
                    // not LLVM pointer/length phis, so direct SSA forwarding
                    // cannot represent the postconditions at those joins here.
                    // These assumptions apply only after successful detach;
                    // Dup and ordinary writes invalidate the tagged loads.
                    let unique = if self.tracking() {
                        crate::runtime::is_writable(&self.builder, copy)
                    } else {
                        crate::runtime::is_unique(&self.builder, copy)
                    };
                    self.builder
                        .assume(unique)
                        .map_err(|what| CodegenError::Invalid {
                            definition: self.definition().clone(),
                            what,
                        })?;
                    if let Some((payload, length)) = old_length {
                        let data = self.payload_of(copy, payload)?;
                        let copied_length = self.builder.load_access(
                            self.module.int_type(64),
                            data.ptr,
                            8,
                            Access::Length,
                        );
                        let equal = self.builder.icmp(IntPredicate::Eq, copied_length, length);
                        self.builder
                            .assume(equal)
                            .map_err(|what| CodegenError::Invalid {
                                definition: self.definition().clone(),
                                what,
                            })?;
                    }
                }

                self.define(result, copy)
            }
            Inst::TokenNone { result } => {
                let null = self.module.const_zero(self.module.ptr_type());
                self.define(result, null)
            }
            Inst::Dup { value } => {
                self.invalidate_uniqueness();
                let block = self.get(value)?;
                if self.is_receipt_value(value)? {
                    let retain = self.runtime(RuntimeEntry::ReceiptRetain)?;
                    self.builder.call(retain, &[block]);
                    return Ok(());
                }
                if self.is_root_value(value)? {
                    return self.root_dup_block(block);
                }
                self.dup_block(block)
            }
            Inst::Dispose { token } => {
                let dispose = self.runtime(RuntimeEntry::Dispose)?;
                self.builder
                    .call(dispose, &[self.context, self.get(token)?]);
                Ok(())
            }
            Inst::Compare {
                result,
                op,
                left,
                right,
                layout,
                failure,
            } => {
                let comparator = if self.needs_value_program(layout) {
                    None
                } else {
                    Some(
                        match compare::comparator(
                            self.module,
                            self.inputs.layouts,
                            self.inputs.keys,
                            self.inputs.index,
                            layout,
                            matches!(op, CompareOp::Eq | CompareOp::Ne),
                        ) {
                            Ok(comparator) => comparator,
                            Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                            Err(CompareError::Invalid(what)) => return self.invalid(what),
                        },
                    )
                };
                if self.module.declares("hirsel_rt_v2_path_compare") {
                    self.manifested
                        .imports
                        .entry("hirsel_rt_v2_path_compare".into())
                        .or_insert_with(|| ObjectSymbol {
                            name: "hirsel_rt_v2_path_compare".into(),
                            kind: ObjectSymbolKind::Runtime,
                            definition: None,
                            abi: None,
                        });
                }
                // Both operands are read in place: each spilled to a slot of the layout.
                let shape = self.layout_shape(layout)?;
                let (l, r) = (self.stack(layout)?, self.stack(layout)?);
                let (left, right) = (self.counted_view(left)?, self.counted_view(right)?);
                self.store(shape, left, l);
                self.store(shape, right, r);
                self.structural_preflight_at(layout, l.ptr, failure)?;
                self.structural_preflight_at(layout, r.ptr, failure)?;
                let code = if let Some(comparator) = comparator {
                    self.builder
                        .call(comparator, &[l.ptr, r.ptr, self.const_u64(0), self.context])
                } else {
                    self.structural_order_at(layout, l.ptr, r.ptr)?
                };
                let i8_ty = self.module.int_type(8);
                let is = |c: u8| {
                    self.builder.icmp(
                        IntPredicate::Eq,
                        code,
                        self.module.const_int(i8_ty, u128::from(c)),
                    )
                };
                let deep = is(ORDER_DEPTH);
                let (equal, less, greater) = (is(ORDER_EQUAL), is(ORDER_LESS), is(ORDER_GREATER));
                self.guard(deep, failure)?;
                let holds = match op {
                    CompareOp::Eq => equal,
                    CompareOp::Ne => self.builder.binary(
                        BinOp::Xor,
                        equal,
                        self.module.const_int(self.module.int_type(1), 1),
                    ),
                    CompareOp::Lt => less,
                    CompareOp::Le => self.builder.binary(BinOp::Or, equal, less),
                    CompareOp::Gt => greater,
                    CompareOp::Ge => self.builder.binary(BinOp::Or, equal, greater),
                };
                self.define(result, holds)
            }
            Inst::SliceView {
                result,
                value,
                start,
                end,
                failure,
            } => {
                let layout = self.value_type(value)?.layout;
                let record = self.layout(layout)?;
                let source = self.stack(layout)?;
                self.store(self.value_shape(value)?, self.get(value)?, source);
                let word = |ordinal: usize| {
                    self.builder
                        .offset(source.ptr, self.const_u64(record.fields[ordinal].offset))
                };
                let owner = self.builder.load(self.module.ptr_type(), word(0), 8);
                let origin = self.load_u64(word(1));
                let length = self.load_u64(word(2));
                let from = self.get(start)?;
                let to = self.get(end)?;
                let reversed = self.builder.icmp(IntPredicate::Ugt, from, to);
                let outside = self.builder.icmp(IntPredicate::Ugt, to, length);
                let failed = self.builder.binary(BinOp::Or, reversed, outside);
                self.guard(failed, failure)?;
                let start = self.builder.binary(BinOp::Add, origin, from);
                let length = self.builder.binary(BinOp::Sub, to, from);
                let slice = self.stack(layout)?;
                for (field, word) in record.fields.iter().zip([owner, start, length]) {
                    let at = self.builder.offset(slice.ptr, self.const_u64(field.offset));
                    self.builder.store(word, at, slice.align.min(8));
                }
                let value = self.load(self.layout_shape(layout)?, slice);
                self.define(result, value)
            }
            Inst::FlatView { result, address } => {
                // A borrowed slice of the String or Bytes encoding at `address`: the block
                // holding it is the owner, the encoding's offset in its payload the start,
                // and its prefix the length (LIR schema revision 4). No credit moves.
                let place = self.place(address)?;
                let Some(block) = place.block else {
                    return self.invalid("a flat view whose address crosses no counted block");
                };
                let layout = self.value_type(result)?.layout;
                let record = self.layout(layout)?;
                let i64_ty = self.module.int_type(64);
                let payload = self.payload_of(block, place.layout)?;
                let to_int = |ptr| self.builder.cast(Cast::PtrToInt, ptr, i64_ty);
                let start = self
                    .builder
                    .binary(BinOp::Sub, to_int(place.ptr), to_int(payload.ptr));
                let length = self.load_u64(place.ptr);
                let slice = self.stack(layout)?;
                for field in &record.fields {
                    let at = self.builder.offset(slice.ptr, self.const_u64(field.offset));
                    let word = match field.ordinal {
                        0 => block,
                        1 => start,
                        2 => length,
                        _ => return self.invalid("a slice layout with a fourth word"),
                    };
                    self.builder.store(word, at, slice.align.min(8));
                }
                let value = self.load(self.layout_shape(layout)?, slice);
                self.define(result, value)
            }
            Inst::FlatLength { result, address } => {
                let count = self.flat_count(self.place(address)?)?;
                self.define(result, count)
            }
            Inst::FlatNext { result, address } => {
                // The content offset just past the element: its end less where its
                // sequence's elements begin.
                let place = self.place(address)?;
                let Some(content) = self.flat_contents[address.index()] else {
                    return self.invalid("the end of a place that is no variable flat element");
                };
                let extent = self.extent(place.layout, place.ptr)?;
                let i64_ty = self.module.int_type(64);
                let to_int = |ptr| self.builder.cast(Cast::PtrToInt, ptr, i64_ty);
                let start = self
                    .builder
                    .binary(BinOp::Sub, to_int(place.ptr), to_int(content));
                let end = self.builder.binary(BinOp::Add, start, extent);
                self.define(result, end)
            }
            Inst::SeqLen { result, value } => {
                let count = if let Some(length) = self.preserved_lengths[value.index()] {
                    length
                } else {
                    match self.layout(self.value_type(value)?.layout)?.kind {
                        LayoutKind::Block { .. } | LayoutKind::Box { .. } => {
                            let data = match self.layout_payload(self.value_type(value)?.layout)? {
                                // A sequential Packed sequence counts its elements in its
                                // first word too (MEM-13).
                                Some(payload)
                                    if matches!(
                                        self.layout(payload)?.kind,
                                        LayoutKind::Sequential { .. }
                                    ) =>
                                {
                                    let pointer = self.counted_view(value)?;
                                    self.payload_of(pointer, payload)?.ptr
                                }
                                _ => self.sequence_payload(value)?.0,
                            };
                            self.builder.load_access(
                                self.module.int_type(64),
                                data,
                                8,
                                Access::Length,
                            )
                        }
                        _ => self.slice_bytes(value)?.1,
                    }
                };
                self.define(result, count)
            }
            Inst::SeqCapacity { result, value } => {
                let capacity = self.sequence_capacity(value)?;
                self.define(result, capacity)
            }
            Inst::SeqReserve {
                address, capacity, ..
            } => {
                let place = self.place(address)?;
                if let Some(payload) = self.layout_payload(place.layout)?
                    && let LayoutKind::SoaColumns { element } = self.layout(payload)?.kind
                {
                    return self.reserve_columns(place, payload, element, self.get(capacity)?);
                }
                if let Some(payload) = self.layout_payload(place.layout)?
                    && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
                {
                    // A flat builder's reservation: room for `capacity` of its least
                    // element in all, past the count and used words, the bytes
                    // saturating (LIR schema revision 22).
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
                    let i64_ty = self.module.int_type(64);
                    let multiply = self
                        .module
                        .intrinsic("llvm.umul.with.overflow", &[i64_ty])
                        .or_else(|e| self.invalid(e))?;
                    let pair = self
                        .builder
                        .call(multiply, &[self.get(capacity)?, self.const_u64(least)]);
                    let bytes = self.builder.select(
                        self.builder.extract(pair, 1),
                        self.const_u64(u64::MAX),
                        self.builder.extract(pair, 0),
                    );
                    let align = u64::from(self.layout(payload)?.align);
                    let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                    let data = self.payload_of(old, payload)?.ptr;
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                    let (extent, _) = scalar::int_arith(
                        &self.builder,
                        BinaryOp::Add,
                        ArithMode::Saturating,
                        ScalarKind::U64,
                        bytes,
                        self.const_u64(first),
                    )
                    .or_else(|e| self.invalid(e))?;
                    // An empty static sequence has no unique backing yet.
                    // The existing sequence grow entry can reserve byte storage
                    // once; restore Sequential's used word after its capacity
                    // word is written. Nonempty builders keep their old path.
                    let count = self.load_u64(data);
                    let empty = self
                        .builder
                        .icmp(IntPredicate::Eq, count, self.const_u64(0));
                    let vacant = self.module.append_block(self.function);
                    let occupied = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    self.builder.cond_br(empty, vacant, occupied);
                    self.builder.position(vacant);
                    let reserve = self.runtime(RuntimeEntry::SeqReserve)?;
                    let fresh = self.builder.call(
                        reserve,
                        &[
                            self.context,
                            old,
                            self.const_u64(1),
                            self.const_u64(align),
                            bytes,
                        ],
                    );
                    let data = self.payload_of(fresh, payload)?.ptr;
                    self.builder.store(
                        self.const_u64(0),
                        self.builder.offset(data, self.const_u64(8)),
                        8,
                    );
                    self.store(Shape::Scalar(ScalarKind::Pointer), fresh, place);
                    self.builder.br(done);
                    self.builder.position(occupied);
                    let grow = self.runtime(RuntimeEntry::BlockGrow)?;
                    let grown = self.builder.call(
                        grow,
                        &[self.context, old, keep, extent, self.const_u64(align)],
                    );
                    self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
                    self.builder.br(done);
                    self.builder.position(done);
                    return Ok(());
                }
                let Some(element) = self.sequence_element(place.layout)? else {
                    return self.invalid("a reserve of a place that holds no sequence block");
                };
                let (size, align) = (
                    self.fixed_size(element)?,
                    u64::from(self.layout(element)?.align),
                );
                let reserve = self.runtime(RuntimeEntry::SeqReserve)?;
                let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                // A unique block (count one is a heap block: statics and regions never
                // carry it) whose capacity word already holds the request is what
                // `hirsel_rt_v2_seq_grow` returns unchanged, so it skips the call.
                // Capacity is unobservable, so the word need not become exact here.
                // Like the inline Dup, it runs only below the context's dup bound,
                // which diagnostic and measurement runs keep zero: they still call.
                let wanted = self.get(capacity)?;
                let held = if size == 0 {
                    self.const_u64(u64::MAX)
                } else {
                    let Some(payload) = self.layout_payload(place.layout)? else {
                        return self.invalid("a reserve of a sequence without payload");
                    };
                    let data = self.payload_of(old, payload)?.ptr;
                    let at = self
                        .builder
                        .offset(data, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
                    self.builder
                        .load_access(self.module.int_type(64), at, 8, Access::Capacity)
                };
                let unique = crate::runtime::is_unique(&self.builder, old);
                let fits = self.builder.icmp(IntPredicate::Uge, held, wanted);
                let i32_ty = self.module.int_type(32);
                let bound = self.builder.load(
                    i32_ty,
                    self.builder
                        .offset(self.context, self.const_u64(CTX_DUP_MAX_OFFSET)),
                    4,
                );
                let inline = self.builder.icmp(
                    IntPredicate::Ugt,
                    bound,
                    self.module.const_int(i32_ty, COUNT_UNIQUE.into()),
                );
                let held_already = self.builder.binary(
                    BinOp::And,
                    inline,
                    self.builder.binary(BinOp::And, unique, fits),
                );
                let short = self.builder.binary(
                    BinOp::Xor,
                    held_already,
                    self.module.const_int(self.module.int_type(1), 1),
                );
                let grow = self.module.append_block(self.function);
                let done = self.module.append_block(self.function);
                self.builder.cold_cond_br(short, grow, done);
                self.builder.position(grow);
                // Read before the call, which may free `old`.
                let kept = self
                    .sequence_state_at(place)
                    .map(|state| self.grown_facts(state, old));
                let grown = self.builder.call(
                    reserve,
                    &[
                        self.context,
                        old,
                        self.const_u64(size),
                        self.const_u64(align),
                        wanted,
                    ],
                );
                self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
                // Growth retains the unique input, allocates count-one storage, or
                // leaves a tracked arena block (MEM-6), which is never unique. It
                // preserves length on every path.
                if let (Some(state), Some((unique, untracked))) =
                    (self.sequence_state_at(place), kept)
                {
                    self.builder.store(unique, state.unique, 1);
                    self.builder.store(untracked, state.untracked, 1);
                }
                self.builder.br(done);
                self.builder.position(done);
                Ok(())
            }
            Inst::SeqAppend { address, value, .. } => {
                // MEM-13: the element's active extent past the used bytes, growing the
                // storage geometrically when it is full (LIR schema revision 19).
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let temp = self.stack(element)?;
                self.store(self.value_shape(value)?, self.get(value)?, temp);
                let bytes = self.extent(element, temp.ptr)?;
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.builder.offset(
                    data,
                    self.builder.binary(BinOp::Add, used, self.const_u64(first)),
                );
                self.builder
                    .memcpy(at, temp.align, temp.ptr, temp.align, bytes);
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::FlatAppend {
                address, construct, ..
            } => {
                // One element written in its flat encoding past the used bytes: its
                // content's length, then the construction's parts (LIR schema
                // revision 22). Growth is `SeqAppend`'s.
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts(site)?;
                let content = self.flat_extent(&parts)?;
                let bytes = self.flat_add(content, self.const_u64(8))?;
                let (data, used) = self.sequential_room(place, payload, first, bytes)?;
                let at = self.builder.offset(
                    data,
                    self.builder.binary(BinOp::Add, used, self.const_u64(first)),
                );
                self.builder.store(content, at, 1);
                self.write_flat(self.builder.offset(at, self.const_u64(8)), &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            // The open element's writes past the used bytes, whose field's length
            // word lies at `word` in the element (LIR schema revision 24). Growth
            // keeps the bytes written so far.
            Inst::FlatField {
                address, field, op, ..
            } => {
                let (place, payload, element) = self.sequential_place(address)?;
                let first = self.sequential_offset(element)?;
                let word = first + self.open_word(element, field)?;
                let (old, data, used) = self.sequential_used(place, payload)?;
                let at = self.builder.binary(BinOp::Add, used, self.const_u64(word));
                if op == FlatFieldOp::Open {
                    let need = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                    let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                    let data = self.sequential_grow(place, payload, old, keep, need)?;
                    self.preserved_builder_used(data, used)?;
                    self.builder.store_access(
                        self.const_u64(0),
                        self.builder.offset(data, at),
                        1,
                        Access::Length,
                    );
                    return Ok(());
                }
                let length = self.builder.load_access(
                    self.module.int_type(64),
                    self.builder.offset(data, at),
                    1,
                    Access::Length,
                );
                let past = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                let keep = self.builder.binary(BinOp::Add, past, length);
                match op {
                    FlatFieldOp::Open => unreachable!("opened above"),
                    FlatFieldOp::Reserve { capacity } => {
                        let (need, _) = scalar::int_arith(
                            &self.builder,
                            BinaryOp::Add,
                            ArithMode::Saturating,
                            ScalarKind::U64,
                            keep,
                            self.get(capacity)?,
                        )
                        .or_else(|e| self.invalid(e))?;
                        let data = self.sequential_grow(place, payload, old, keep, need)?;
                        self.preserved_builder_used(data, used)?;
                    }
                    FlatFieldOp::Push { value, spare } => {
                        let data = if spare {
                            data
                        } else {
                            let need = self.builder.binary(BinOp::Add, keep, self.const_u64(1));
                            self.sequential_grow(place, payload, old, keep, need)?
                        };
                        self.builder.store_access(
                            self.get(value)?,
                            self.builder.offset(data, keep),
                            1,
                            Access::Value,
                        );
                        let longer = self.builder.binary(BinOp::Add, length, self.const_u64(1));
                        self.builder.store_access(
                            longer,
                            self.builder.offset(data, at),
                            1,
                            Access::Length,
                        );
                    }
                }
                Ok(())
            }
            // The open element completed: its prefix and the construction's other
            // fields written around the bytes in place, as `FlatAppend` writes an
            // element. Only fields after the open one need more room.
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
                let start = self.builder.binary(BinOp::Add, used, self.const_u64(first));
                let at = self.builder.binary(BinOp::Add, start, self.const_u64(word));
                let length = self.builder.load_access(
                    self.module.int_type(64),
                    self.builder.offset(data, at),
                    1,
                    Access::Length,
                );
                let site = self.construct_site(construct)?;
                let parts = self.flat_parts_around(site, Some((field, length)))?;
                let content = self.flat_extent(&parts)?;
                let bytes = self.flat_add(content, self.const_u64(8))?;
                let trailing = !matches!(parts.last(), Some(FlatPart::InPlace { .. }));
                let data = if trailing {
                    let past = self.builder.binary(BinOp::Add, at, self.const_u64(8));
                    let keep = self.builder.binary(BinOp::Add, past, length);
                    let need = self.builder.binary(BinOp::Add, start, bytes);
                    self.sequential_grow(place, payload, old, keep, need)?
                } else {
                    data
                };
                let at = self.builder.offset(data, start);
                self.builder.store(content, at, 1);
                self.write_flat(self.builder.offset(at, self.const_u64(8)), &parts)?;
                self.sequential_grew(data, used, bytes);
                Ok(())
            }
            Inst::SeqSetLen { address, len, .. } => {
                let place = self.place(address)?;
                // A deferred slot's block, unique here, takes its count at the next
                // flush; in a push loop, after the loop.
                if let Some(state) = self.sequence_state_at(place)
                    && self
                        .whole_slot(address)
                        .is_some_and(|s| self.deferred[s.index()])
                {
                    let slot = self.whole_slot(address).expect("deferred slot").index();
                    self.builder.store(self.get(len)?, state.length, 8);
                    let dirty = || {
                        self.builder.store(
                            self.module.const_int(self.module.int_type(1), 1),
                            state.dirty,
                            1,
                        )
                    };
                    let Some(defer) = self.defer_params[slot] else {
                        dirty();
                        return Ok(());
                    };
                    // A caller that keeps the count in the block gets it stored here.
                    let deferred = self.module.append_block(self.function);
                    let write = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    self.builder.cond_br(defer, deferred, write);
                    self.builder.position(deferred);
                    dirty();
                    self.builder.br(done);
                    self.builder.position(write);
                    let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                    let Some(payload) = self.layout_payload(place.layout)? else {
                        return self.invalid("a length of a place that holds no block");
                    };
                    let data = self.payload_of(pointer, payload)?;
                    self.builder
                        .store_access(self.get(len)?, data.ptr, 8, Access::Length);
                    self.builder.br(done);
                    self.builder.position(done);
                    return Ok(());
                }
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                let Some(payload) = self.layout_payload(place.layout)? else {
                    return self.invalid("a length of a place that holds no block");
                };
                let data = self.payload_of(pointer, payload)?;
                self.builder
                    .store_access(self.get(len)?, data.ptr, 8, Access::Length);
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(self.get(len)?, state.length, 8);
                }
                Ok(())
            }
            Inst::Hash {
                result,
                value,
                layout,
                failure,
            } => {
                self.structural_preflight_value(layout, value, failure)?;
                // Scalars remain a single one-shot leaf. Composite values
                // share one stream across all layout helpers.
                let begin = self.runtime(RuntimeEntry::HashBegin)?;
                let seed = self.builder.call(begin, &[self.context]);
                if self.needs_value_program(layout) {
                    let at = self.stack(layout)?;
                    let operand = self.counted_view(value)?;
                    self.store(self.layout_shape(layout)?, operand, at);
                    let hash = self.structural_hash_at(layout, at.ptr, seed)?;
                    return self.define(result, hash);
                }
                if matches!(self.layout(layout)?.kind, LayoutKind::Scalar(kind)
                    if !matches!(kind, ScalarKind::F32 | ScalarKind::F64 | ScalarKind::Pointer))
                {
                    let LayoutKind::Scalar(kind) = self.layout(layout)?.kind else {
                        unreachable!()
                    };
                    let bits = if kind == ScalarKind::Bool {
                        1
                    } else {
                        crate::scalar::integer(kind).expect("integer hash").0
                    };
                    let hash =
                        crate::scalar::hash_integer(&self.builder, self.get(value)?, seed, bits);
                    return self.define(result, hash);
                }
                let at = self.stack(layout)?;
                let operand = self.counted_view(value)?;
                self.store(self.layout_shape(layout)?, operand, at);
                // A short byte slice's complete framing fits in two words.
                // Feed exactly length || bytes through the one-shot leaf; longer
                // slices retain the streaming path without a proportional buffer.
                let short = if matches!(self.layout(layout)?.kind, LayoutKind::Slice { element } if self.fixed_size(element)? == 1)
                {
                    let (data, length) = self.slice_bytes(value)?;
                    let fast = self.module.append_block(self.function);
                    let long = self.module.append_block(self.function);
                    let done = self.module.append_block(self.function);
                    let fits = self
                        .builder
                        .icmp(IntPredicate::Ule, length, self.const_u64(8));
                    self.builder.cond_br(fits, fast, long);
                    self.builder.position(fast);
                    let ty = self.module.array_type(self.module.int_type(8), 16);
                    let buffer = self.prologue.alloca(ty, 8);
                    self.builder.store(length, buffer, 8);
                    let tail = self.builder.offset(buffer, self.const_u64(8));
                    self.builder.memcpy(tail, 1, data, 1, length);
                    let framed = self.builder.binary(BinOp::Add, length, self.const_u64(8));
                    let bytes = self.runtime(RuntimeEntry::HashBytes)?;
                    let hash = self.builder.call(bytes, &[seed, buffer, framed]);
                    let from = self.builder.current();
                    self.builder.br(done);
                    self.builder.position(long);
                    Some((done, from, hash))
                } else {
                    None
                };
                let update = self.runtime(RuntimeEntry::HashUpdate)?;
                let update = crate::runtime::hash_stream(self.module, update, false);
                let hasher = match compare::hasher(
                    self.module,
                    self.inputs.layouts,
                    self.inputs.keys,
                    self.inputs.index,
                    layout,
                    update,
                ) {
                    Ok(hasher) => hasher,
                    Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                    Err(CompareError::Invalid(what)) => return self.invalid(what),
                };
                let state_ty = self.module.array_type(
                    self.module.int_type(8),
                    hirsel_lir::runtime_abi::HASH_STATE_BYTES,
                );
                let storage = self
                    .prologue
                    .alloca(state_ty, hirsel_lir::runtime_abi::HASH_STATE_ALIGN);
                let init = self.runtime(RuntimeEntry::HashInit)?;
                self.builder.call(init, &[storage, seed]);
                let state = storage;
                let i8_ty = self.module.int_type(8);
                let deep = self.prologue.alloca(i8_ty, 1);
                self.builder.store(self.module.const_int(i8_ty, 0), deep, 1);
                let hash = self.builder.call(
                    hasher,
                    &[at.ptr, state, self.const_u64(0), deep, self.context],
                );
                let flag = self.builder.load(i8_ty, deep, 1);
                let too_deep =
                    self.builder
                        .icmp(IntPredicate::Ne, flag, self.module.const_int(i8_ty, 0));
                self.guard(too_deep, failure)?;
                let finish = self.runtime(RuntimeEntry::HashFinish)?;
                let finish = crate::runtime::hash_stream(self.module, finish, true);
                let hash = self.builder.call(finish, &[hash]);
                let hash = if let Some((done, fast, short)) = short {
                    let long = self.builder.current();
                    self.builder.br(done);
                    self.builder.position(done);
                    let joined = self.builder.phi(self.module.int_type(64));
                    self.builder.add_incoming(joined, short, fast);
                    self.builder.add_incoming(joined, hash, long);
                    joined
                } else {
                    hash
                };
                self.define(result, hash)
            }
            // MEM-11: a fresh root block holding the value, whose blocks the runtime freezes.
            Inst::Freeze { result, value } => {
                let payload = self.frozen_payload(result)?;
                let record = self.layout(payload)?;
                let SizeSpec::Fixed(size) = record.size else {
                    return self.invalid("a frozen value of dynamic extent");
                };
                let align = record.align;
                let anchor = self.unit.blocks[self.unit.entry.index()].anchor;
                let block = self.alloc_block(payload, self.const_u64(size), align, anchor)?;
                let place = self.payload_of(block, payload)?;
                self.store(self.value_shape(value)?, self.counted_view(value)?, place);
                let freeze = self.runtime(RuntimeEntry::Freeze)?;
                self.builder.call(freeze, &[self.context, block]);
                self.define(result, block)
            }
            // The last root: thaw its value, move it out and free the root block shallowly.
            Inst::Thaw { result, value } => {
                let payload = self.frozen_payload(value)?;
                let block = self.get(value)?;
                let thaw = self.runtime(RuntimeEntry::Thaw)?;
                self.builder.call(thaw, &[self.context, block]);
                let place = self.payload_of(block, payload)?;
                let thawed = self.load(self.value_shape(result)?, place);
                let free = self.runtime(RuntimeEntry::Free)?;
                self.builder.call(free, &[self.context, block]);
                self.define(result, thawed)
            }
            // The runtime walks the operand in place against the unit's codec
            // program and writes the produced value into a slot of its layout.
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
                self.store(self.layout_shape(layout)?, operand, at);
                let produced = self.value_type(result)?.layout;
                let out = self.stack(produced)?;
                let call = self.runtime(entry)?;
                self.builder
                    .call(call, &[self.context, codec, at.ptr, out.ptr]);
                let value = self.load(self.layout_shape(produced)?, out);
                self.define(result, value)
            }
            // The same pointer at another payload layout: no code.
            Inst::Recast { result, value } => {
                let pointer = self.get(value)?;
                self.define(result, pointer)
            }
            Inst::Release { value, .. } if self.adopted.contains_key(&value) => {
                // A builder whose block a flat record took over is not released.
                let taken = self.adopted.remove(&value).expect("an adopted release");
                let (release, done) = (
                    self.module.append_block(self.function),
                    self.module.append_block(self.function),
                );
                self.builder.cond_br(taken, done, release);
                self.builder.position(release);
                self.instruction(inst)?;
                self.builder.br(done);
                self.builder.position(done);
                Ok(())
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
                let receipt = self.get(value)?;
                let release = self.runtime(RuntimeEntry::ReceiptRelease)?;
                self.builder.call(release, &[receipt]);
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
                let release = self.runtime(RuntimeEntry::RootDrop)?;
                let block = self.get(value)?;
                let (anchor, operation_site) = self.failure_sites(self.cleanup_code(anchor)?);
                let status = self
                    .builder
                    .call(release, &[self.context, block, anchor, operation_site]);
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
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } if self.resume.is_none()
                && live == MaskRef::All
                && glue::covered(self.inputs.layouts, layout) =>
            {
                // Compiled drop glue; the runtime walks the step program of every other
                // layout and of a partly moved block.
                let block = self.get(value)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                let status = self.drop_glue(anchor, |walk| walk.release(block, Some(layout)))?;
                self.status_guard(status, failure)
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
                let record = self.layout(payload)?;
                let SizeSpec::Fixed(size) = record.size else {
                    return self.invalid("an open payload of dynamic extent");
                };
                let align = record.align;
                let block = self.alloc_block(payload, self.const_u64(size), align, anchor)?;
                self.define(result, block)?;
                self.define(hole, block)
            }
            // The value moves into the open payload; the header then takes the box's layout.
            Inst::HoleFill { hole, value } => {
                let LayoutKind::Box { payload } = self.layout(self.value_type(hole)?.layout)?.kind
                else {
                    return self.invalid("a hole of no box");
                };
                let block = self.get(hole)?;
                let place = self.payload_of(block, payload)?;
                self.store(self.value_shape(value)?, self.counted_view(value)?, place);
                let descriptor = self.descriptor(payload)?;
                let close = self.runtime(RuntimeEntry::HoleClose)?;
                self.builder.call(close, &[self.context, block, descriptor]);
                Ok(())
            }
            // The runtime releases the root by its blocks' headers, so no open payload is
            // read; compiled glue, which follows static layouts, never runs here. Nothing
            // it releases is destructible, so its status is success.
            Inst::HoleAbandon { root, .. } => {
                let Some(root) = root else {
                    return Ok(());
                };
                self.invalidate_uniqueness();
                let release = self.runtime(RuntimeEntry::Release)?;
                let block = self.get(root)?;
                let descriptor = self.block_descriptor(block);
                let null = self.module.const_zero(self.module.ptr_type());
                let zero = self.const_u64(0);
                self.builder.call(
                    release,
                    &[self.context, block, descriptor, null, zero, zero],
                );
                Ok(())
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } => {
                let release = self.runtime(RuntimeEntry::Release)?;
                let block = self.get(value)?;
                let certified = self.resume.as_ref().is_some_and(|r| {
                    r.plan
                        .cleanups
                        .iter()
                        .any(|s| s.value == value && s.anchor == anchor)
                });
                let descriptor = if certified {
                    self.descriptor(layout)?
                } else {
                    self.block_descriptor(block)
                };
                let (anchor, operation_site) = self.failure_sites(self.cleanup_code(anchor)?);
                let arguments = [
                    self.context,
                    block,
                    descriptor,
                    self.live_mask(live)?,
                    anchor,
                    operation_site,
                ];
                let status = self.builder.call(release, &arguments);
                if self.resume.is_some() {
                    self.resume_propagate(status)?;
                }
                self.status_guard(status, failure)
            }
            Inst::Reset {
                result,
                value,
                layout,
                live,
                anchor,
                failure,
            } if self.value_type(value)?.repr == ValueRepr::Block
                && glue::covered(self.inputs.layouts, layout)
                && match live {
                    MaskRef::All => true,
                    MaskRef::Static(mask) => self.inert_mask(layout, mask)?,
                    MaskRef::Dynamic(_) => false,
                } =>
            {
                let block = self.get(value)?;
                let token = self.reset_glue(block, layout, live, anchor, failure)?;
                self.define(result, token)
            }
            Inst::Reset {
                result,
                value,
                layout: _,
                live,
                anchor,
                failure,
            } => {
                // A callable donates its environment block (LIR schema revision 5), which
                // `layout` describes.
                let block = match self.value_type(value)?.repr {
                    ValueRepr::Callable => self.callable_parts(value)?.1,
                    _ => self.get(value)?,
                };
                let token = self.reset_call(block, live, anchor, failure)?;
                self.define(result, token)
            }
            Inst::Reuse {
                result,
                token,
                construct,
                kept,
                failure,
            } => {
                let site = self.construct_site(construct)?;
                let token = self.get(token)?;
                self.construct_block(result, site, Some((token, kept)), failure)
            }
            Inst::RegionEnter { result, region } => {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                // An arena's region (its mark names an `ArenaToken`) enters warm, its cursor
                // seeded from the pool, so its first construction bumps inline.
                let arena =
                    self.unit.instructions.iter().any(
                        |inst| matches!(*inst, Inst::ArenaToken { mark, .. } if mark == result),
                    );
                let demanded = self.unit.tables.regions[region.index()].seal_demand;
                let enter = self.runtime(if demanded {
                    RuntimeEntry::ReceiptRegionEnter
                } else if arena {
                    RuntimeEntry::ArenaEnter
                } else {
                    RuntimeEntry::RegionEnter
                })?;
                self.builder.call(enter, &[self.context, record]);
                self.define(result, record)
            }
            Inst::RegionReserve {
                result,
                mark,
                layout,
                extent,
                failure,
            } => {
                let size = match self.layout(layout)?.size {
                    SizeSpec::Fixed(size) => self.const_u64(size),
                    SizeSpec::Dynamic(_) => self.get(extent)?,
                };
                let storage = self.reserve_region(self.get(mark)?, layout, size, failure)?;
                self.define(result, storage.ptr)
            }
            // Publication is static: nothing moves or writes published storage afterwards.
            Inst::RegionPublish { .. } => Ok(()),
            // A token is its region's record address.
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                // A bounded arena's budget (MEM-6) is set before its token exists.
                if let Some(budget) = budget {
                    let bound = self.runtime(RuntimeEntry::ArenaBound)?;
                    let record = self.get(mark)?;
                    self.builder
                        .call(bound, &[self.context, record, self.get(budget)?]);
                }
                let record = self.get(mark)?;
                self.define(result, record)
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
                let (anchor, operation_site) =
                    self.failure_sites(self.anchor_code(instance.anchor)?);
                let exit = self.runtime(if instance.seal_demand {
                    RuntimeEntry::ReceiptRegionExit
                } else {
                    RuntimeEntry::RegionExit
                })?;
                let status = self.builder.call(
                    exit,
                    &[self.context, self.get(mark)?, anchor, operation_site],
                );
                self.status_guard(status, failure)
            }
            // The resource's state moves (Live -> Finalizing -> FieldsPending); the drop body
            // is an explicit call between the two, shielded by the runtime.
            // Defuse is a validated ownership-state transition. Native storage
            // already holds the fields, and their ordinary releases follow.
            Inst::Defuse { .. } => Ok(()),
            Inst::FinalizeBegin { anchor, .. } => {
                let begin = self.runtime(RuntimeEntry::FinalizeBegin)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                self.builder.call(begin, &[self.context, anchor]);
                Ok(())
            }
            Inst::FinalizeEnd { anchor, .. } => {
                let end = self.runtime(RuntimeEntry::FinalizeEnd)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                self.builder.call(end, &[self.context, anchor]);
                Ok(())
            }
            Inst::Copy {
                result, address, ..
            } => {
                // A semantic copy by the Dup or Fields recipe: the place's bits, then one count
                // increment per counted holder in scan order. It never allocates and never
                // fails, so its failure edge is never taken.
                let place = self.place(address)?;
                if self.value_type(result)?.layout != place.layout {
                    return self.invalid("a copy at another layout than its place");
                }
                let value = self.load(self.layout_shape(place.layout)?, self.gather(place)?);
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
                result,
                value,
                shape,
                binding,
                inputs,
            } => self.payload_receipt(result, value, shape, binding, inputs),
            Inst::ArenaInventory {
                arena,
                allocation,
                value,
                shape,
                previous,
            } => self.arena_inventory(arena, allocation, value, shape, previous),
            Inst::CopyValue { .. } | Inst::Drop { .. } | Inst::Unavailable { .. } => {
                self.invalid("an operation outside the Ready vocabulary")
            }
        }
    }
}
