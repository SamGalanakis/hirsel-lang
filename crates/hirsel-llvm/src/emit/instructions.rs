use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Instructions.

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
        if matches!(inst, Inst::UniqueCopy { .. } | Inst::RegionExit { .. }) {
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
            Inst::Check { condition, failure } => {
                let condition = self.get(condition)?;
                let one = self.module.const_int(self.module.int_type(1), 1);
                let failed = self.builder.binary(BinOp::Xor, condition, one);
                self.guard(failed, failure)
            }
            // Static lifetime events: no machine bytes. Frame admission has no
            // numeric charge until spec #16 rules, so it cannot fail.
            Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave => Ok(()),
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
            Inst::Store { address, value, .. } => {
                let place = self.place(address)?;
                let stored = if self.whole_slot(address).is_some() {
                    self.get(value)?
                } else {
                    self.counted_view(value)?
                };
                self.store(self.value_shape(value)?, stored, place);
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
                let old = self.load(shape, place);
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
                self.store(shape, stored, place);
                if let Some(state) = self.sequence_states[displaced.index()] {
                    let length = self.sequence_length(old, slot.layout)?;
                    self.builder.store(length, state.length, 8);
                    self.builder.store(
                        self.module.const_zero(self.module.int_type(1)),
                        state.unique,
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
                            let Some(payload) = slice_storage(self.inputs.layouts, element) else {
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
                        let from = self.payload_of(block, payload)?;
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
                let site = self.const_u64(self.anchor_code(anchor)?);
                let arguments = if deep {
                    vec![self.context, self.get(value)?, destination, site, out]
                } else {
                    vec![self.context, self.get(value)?, site, out]
                };
                let status = self.builder.call(entry, &arguments);
                if deep {
                    self.copy_status(status, destination, anchor, failure)?;
                } else {
                    self.status_guard(status, failure)?;
                }
                let copy = self.builder.load(self.module.ptr_type(), out, 8);
                if !deep {
                    // copy.rs::hirsel_rt_v2_detach allocates count-one storage,
                    // copies the payload verbatim and only increments children.
                    // Its result joins through local storage-version tokens,
                    // not LLVM pointer/length phis, so direct SSA forwarding
                    // cannot represent the postconditions at those joins here.
                    // These assumptions apply only after successful detach;
                    // Dup and ordinary writes invalidate the tagged loads.
                    let unique = crate::runtime::is_unique(&self.builder, copy);
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
                let comparator = match compare::comparator(
                    self.module,
                    self.inputs.layouts,
                    layout,
                    matches!(op, CompareOp::Eq | CompareOp::Ne),
                ) {
                    Ok(comparator) => comparator,
                    Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                    Err(CompareError::Invalid(what)) => return self.invalid(what),
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
                let code = self
                    .builder
                    .call(comparator, &[l.ptr, r.ptr, self.const_u64(0), self.context]);
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
                    match self.value_type(value)?.repr {
                        ValueRepr::Block => {
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
                    let grow = self.runtime(RuntimeEntry::BlockGrow)?;
                    let grown = self.builder.call(
                        grow,
                        &[self.context, old, keep, extent, self.const_u64(align)],
                    );
                    self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
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
                let grown = self.builder.call(
                    reserve,
                    &[
                        self.context,
                        old,
                        self.const_u64(size),
                        self.const_u64(align),
                        self.get(capacity)?,
                    ],
                );
                self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
                // Growth either retains the unique input or allocates count-one
                // storage. It preserves length on both paths.
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(
                        self.module.const_int(self.module.int_type(1), 1),
                        state.unique,
                        1,
                    );
                }
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
                let hash_bytes = self.runtime(RuntimeEntry::HashBytes)?;
                let hasher =
                    match compare::hasher(self.module, self.inputs.layouts, layout, hash_bytes) {
                        Ok(hasher) => hasher,
                        Err(CompareError::Unsupported(what)) => return self.unsupported(what),
                        Err(CompareError::Invalid(what)) => return self.invalid(what),
                    };
                // The value is read in place: spilled to a slot of the layout.
                let at = self.stack(layout)?;
                let operand = self.counted_view(value)?;
                self.store(self.layout_shape(layout)?, operand, at);
                let begin = self.runtime(RuntimeEntry::HashBegin)?;
                let state = self.builder.call(begin, &[self.context]);
                let i8_ty = self.module.int_type(8);
                let deep = self.prologue.alloca(i8_ty, 1);
                self.builder.store(self.module.const_int(i8_ty, 0), deep, 1);
                let hash = self
                    .builder
                    .call(hasher, &[at.ptr, state, self.const_u64(0), deep]);
                let flag = self.builder.load(i8_ty, deep, 1);
                let too_deep =
                    self.builder
                        .icmp(IntPredicate::Ne, flag, self.module.const_int(i8_ty, 0));
                self.guard(too_deep, failure)?;
                self.define(result, hash)
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
            } if live == MaskRef::All && glue::covered(self.inputs.layouts, layout) => {
                // Compiled drop glue; the runtime walks the step program of every other
                // layout and of a partly moved block.
                let block = self.get(value)?;
                let anchor = self.const_u64(self.cleanup_code(anchor)?);
                let status = self.drop_glue(anchor, |walk| walk.release(block, Some(layout)))?;
                self.status_guard(status, failure)
            }
            Inst::Release {
                value,
                layout: _,
                live,
                anchor,
                failure,
            } => {
                let release = self.runtime(RuntimeEntry::Release)?;
                let block = self.get(value)?;
                let descriptor = self.block_descriptor(block);
                let arguments = [
                    self.context,
                    block,
                    descriptor,
                    self.live_mask(live)?,
                    self.const_u64(self.cleanup_code(anchor)?),
                ];
                let status = self.builder.call(release, &arguments);
                self.status_guard(status, failure)
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
                let reset = self.runtime(RuntimeEntry::Reset)?;
                let descriptor = self.block_descriptor(block);
                let out = self.prologue.alloca(self.module.ptr_type(), 8);
                let arguments = [
                    self.context,
                    block,
                    descriptor,
                    self.live_mask(live)?,
                    self.const_u64(self.cleanup_code(anchor)?),
                    out,
                ];
                let status = self.builder.call(reset, &arguments);
                self.status_guard(status, failure)?;
                let token = self.builder.load(self.module.ptr_type(), out, 8);
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
                let enter = self.runtime(if arena {
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
                let anchor = self.anchor_code(instance.anchor)?;
                let exit = self.runtime(RuntimeEntry::RegionExit)?;
                let status = self.builder.call(
                    exit,
                    &[self.context, self.get(mark)?, self.const_u64(anchor)],
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
                let value = self.load(self.layout_shape(place.layout)?, place);
                self.dup_holders(place)?;
                self.define(result, value)
            }
            Inst::Drop { .. } | Inst::Unavailable { .. } => {
                self.invalid("an operation outside the Ready vocabulary")
            }
        }
    }
}
