use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// A local root initialized once and never handed out as an address. Its
    /// length cannot change through an erased call: another holder must detach
    /// before changing the sequence. Mutable captures are excluded by the
    /// caller, and C callbacks retain ordinary memory semantics.
    fn immutable_sequence_root(&self, slot: SlotId) -> bool {
        if self.inputs.callbacks
            || self.unit.header.origin == UnitOrigin::Library
            || matches!(
                self.unit.slots[slot.index()].kind,
                SlotKind::Parameter(_) | SlotKind::Capture(_)
            )
        {
            return false;
        }
        let mut stores = 0;
        for inst in &self.unit.instructions {
            let mut reaches = false;
            inst.visit(&mut |reference| {
                if let Ref::Address(address) = reference {
                    reaches |= self.address_slot(address) == Some(slot);
                }
            });
            if !reaches {
                continue;
            }
            match *inst {
                Inst::Store { address, .. } if self.whole_slot(address) == Some(slot) => {
                    stores += 1
                }
                Inst::Address { .. }
                | Inst::Read { .. }
                | Inst::Copy { .. }
                | Inst::BindOrigin { .. }
                | Inst::BindFields { .. }
                | Inst::BeginLoan { mutable: false, .. } => {}
                _ => return false,
            }
        }
        stores == 1 && self.unit.tables.calls.iter().all(|call| {
            call.arguments.get(&self.unit.pools.call_arguments).is_some_and(|args| {
                args.iter().all(|arg| !matches!(arg.value,
                    ArgumentValue::Address { address, .. } if self.address_slot(address) == Some(slot)))
            })
        })
    }

    /// Only a whole, private root has companion state. Captured addresses and
    /// externally passed roots retain ordinary memory semantics. Restrict the
    /// state to sequences of pointer-free, silently destroyed elements, so an
    /// element release/finalizer cannot change the root's count or length.
    pub(super) fn whole_slot(&self, address: AddressId) -> Option<SlotId> {
        let info = self.unit.addresses.get(address.index())?;
        if info.projection.len != 0 {
            return None;
        }
        match info.base {
            AddressBase::Slot(slot) => Some(slot),
            AddressBase::Address(base) => self.whole_slot(base),
            AddressBase::Value(_) => None,
        }
    }

    pub(super) fn root_call(&self, call: &CallSite) -> bool {
        if let Callee::Direct(symbol) = call.callee {
            let symbol = &self.unit.tables.symbols[symbol.index()];
            let name = if self.inputs.alternate
                && symbol.instance.is_some_and(|id| {
                    self.inputs
                        .ahead
                        .is_some_and(|plan| plan.has_private_edge(self.unit.header.instance, id))
                }) {
                hirsel_lir::ahead::alternate_name(&symbol.definition, symbol.instance.unwrap())
            } else {
                symbol_name(&symbol.definition, symbol.instance.as_ref())
            };
            self.manifested
                .library
                .contains(&format!("{name}{ROOTS_SUFFIX}"))
        } else {
            false
        }
    }

    pub(super) fn allocate_sequence_states(&mut self) -> Emitted {
        // Mutable captures can name storage behind an erased environment.
        if !self.unit.tables.closures.is_empty() {
            return Ok(());
        }
        for (i, slot) in self.unit.slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Capture(_)) {
                continue;
            }
            if let SlotKind::Parameter(source) = slot.kind
                && !self
                    .signature
                    .roots
                    .iter()
                    .any(|&i| self.signature.abi.parameters[i].source == source)
            {
                continue;
            }
            let ty = self.lir_type(slot.ty)?;
            if ty.repr != ValueRepr::Block {
                continue;
            }
            let layout = ty.layout;
            // Companion ABIs and deferred counts belong to ordinary sequence
            // roots. Column payloads retain their count in their own header.
            let Some(payload) = self.layout_payload(layout)? else {
                continue;
            };
            if !matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. }) {
                continue;
            }
            let Some(element) = self.sequence_element(layout)? else {
                continue;
            };
            let record = self.layout(element)?;
            if (record.finalizer.is_some() || !record.scan.is_empty() || !record.drop.is_empty())
                && !self.immutable_sequence_root(SlotId(i as u32))
            {
                continue;
            }
            let escaped = self.unit.tables.calls.iter().any(|call| {
                !self.root_call(call) && call.arguments.get(&self.unit.pools.call_arguments).is_some_and(|args| {
                    args.iter().any(|arg| matches!(arg.value, ArgumentValue::Address { address, .. } if self.whole_slot(address) == Some(SlotId(i as u32))))
                })
            });
            if escaped {
                continue;
            }
            let state = SequenceState {
                unique: self.prologue.alloca(self.module.int_type(1), 1),
                untracked: self.prologue.alloca(self.module.int_type(1), 1),
                length: self.prologue.alloca(self.module.int_type(64), 8),
                dirty: self.prologue.alloca(self.module.int_type(1), 1),
            };
            self.prologue.store(
                self.module.const_zero(self.module.int_type(1)),
                state.unique,
                1,
            );
            self.prologue.store(
                self.module.const_zero(self.module.int_type(1)),
                state.untracked,
                1,
            );
            self.prologue.store(self.const_u64(0), state.length, 8);
            self.prologue.store(
                self.module.const_zero(self.module.int_type(1)),
                state.dirty,
                1,
            );
            if let SlotKind::Parameter(source) = slot.kind {
                let field = self
                    .signature
                    .roots
                    .iter()
                    .position(|&p| self.signature.abi.parameters[p].source == source)
                    .expect("private parameter");
                let first = self.signature.companions[field].expect("sequence inputs");
                self.builder
                    .store(self.module.parameter(self.function, first), state.length, 8);
                let unique = self.module.parameter(self.function, first + 1);
                self.builder.store(unique, state.unique, 1);
                let untracked = self.module.parameter(self.function, first + 3);
                self.builder.store(untracked, state.untracked, 1);
                // A deferring caller hands over a length its block may lag.
                let defer = self.module.parameter(self.function, first + 2);
                self.builder.store(defer, state.dirty, 1);
                self.defer_params[i] = Some(defer);
                self.companion_slots[i] = true;
                self.deferred[i] = true;
            }
            self.sequence_states[i] = Some(state);
        }
        let unit = self.unit;
        for inst in &unit.instructions {
            match *inst {
                Inst::SeqSetLen { address, .. } => {
                    if let Some(slot) = self.whole_slot(address) {
                        self.deferred[slot.index()] = true;
                    }
                }
                // A borrowed or bitwise view keeps reading the slot's block.
                Inst::Read { address, mode, .. } if mode != ReadMode::Take => {
                    if let Some(slot) = self.whole_slot(address) {
                        self.viewed[slot.index()] = true;
                    }
                }
                _ => {}
            }
        }
        for call in &unit.tables.calls {
            if !self.root_call(call) {
                continue;
            }
            for argument in self.pool(call.arguments, &unit.pools.call_arguments)? {
                if let ArgumentValue::Address { address, .. } = argument.value
                    && let Some(slot) = self.whole_slot(address)
                {
                    self.deferred[slot.index()] = true;
                }
            }
        }
        for (i, state) in self.sequence_states.iter().enumerate() {
            self.deferred[i] &= state.is_some();
            self.viewed[i] &= self.deferred[i];
        }
        Ok(())
    }

    /// Stores `length` into the count word of the sequence block `pointer` (at
    /// `layout`) where the two differ: never into a static or shared block,
    /// whose count a deferred length always equals.
    pub(super) fn store_length_if_differs(
        &self,
        pointer: Value<'m>,
        layout: LayoutId,
        length: Value<'m>,
    ) -> Emitted {
        let Some(payload) = self.layout_payload(layout)? else {
            return self.invalid("a length of a place that holds no block");
        };
        let data = self.payload_of(pointer, payload)?;
        let count = self
            .builder
            .load_access(self.module.int_type(64), data.ptr, 8, Access::Length);
        let differs = self.builder.icmp(IntPredicate::Ne, count, length);
        let write = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(differs, write, done);
        self.builder.position(write);
        self.builder
            .store_access(length, data.ptr, 8, Access::Length);
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// Leaving the entry: each deferred slot's block stores its length, but a root
    /// parameter's, whose length its caller receives. A function that can hand a
    /// view out (through a result, a destination, an environment or a parameter
    /// holding a reference) stores its borrowed root parameters' too.
    pub(super) fn flush_on_return(&self) -> Emitted {
        let abi = &self.signature.abi;
        let plain = |layout: LayoutId| {
            self.inputs
                .layouts
                .layout(layout)
                .is_some_and(|l| l.scan.is_empty() && l.drop.is_empty() && l.finalizer.is_none())
        };
        let isolated = !abi.environment
            && !abi.destination
            && abi.result.as_ref().is_none_or(|r| plain(r.layout))
            && abi
                .parameters
                .iter()
                .enumerate()
                .all(|(i, p)| self.signature.roots.contains(&i) || plain(p.layout));
        for slot in 0..self.deferred.len() {
            if !self.companion_slots[slot] || (self.viewed[slot] && !isolated) {
                self.flush_length(slot)?;
            } else if let Some(defer) = self.defer_params[slot] {
                // A caller that keeps the count in the block reads it there.
                let flush = self.module.append_block(self.function);
                let done = self.module.append_block(self.function);
                self.builder.cond_br(defer, done, flush);
                self.builder.position(flush);
                self.flush_length(slot)?;
                self.builder.br(done);
                self.builder.position(done);
            }
        }
        Ok(())
    }

    /// The slot an address projects from, through its address bases.
    pub(super) fn address_slot(&self, address: AddressId) -> Option<SlotId> {
        match self.unit.addresses.get(address.index())?.base {
            AddressBase::Slot(slot) => Some(slot),
            AddressBase::Address(base) => self.address_slot(base),
            AddressBase::Value(_) => None,
        }
    }

    /// Brings the count word of deferred slot `slot` up to its length, if dirty.
    pub(super) fn flush_length(&self, slot: usize) -> Emitted {
        let (Some(state), Some(Some(place))) =
            (self.sequence_states[slot], self.slots.get(slot).copied())
        else {
            return Ok(());
        };
        if !self.deferred[slot] {
            return Ok(());
        }
        let i1 = self.module.int_type(1);
        let dirty = self.builder.load(i1, state.dirty, 1);
        let flush = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(dirty, flush, done);
        self.builder.position(flush);
        let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let length = self.builder.load(self.module.int_type(64), state.length, 8);
        self.store_length_if_differs(pointer, place.layout, length)?;
        self.builder.br(done);
        self.builder.position(done);
        self.builder
            .store(self.module.const_zero(i1), state.dirty, 1);
        Ok(())
    }

    /// Flushes every deferred slot (but the root parameters, unless `companions`).
    pub(super) fn flush_lengths(&self, companions: bool) -> Emitted {
        for slot in 0..self.deferred.len() {
            if companions || !self.companion_slots[slot] {
                self.flush_length(slot)?;
            }
        }
        Ok(())
    }

    /// Before a count read from memory through a view of unknown origin: flushes
    /// each borrowed deferred slot whose block has the sequence payload `payload`.
    pub(super) fn flush_viewed(&self, payload: LayoutId) -> Emitted {
        for slot in 0..self.deferred.len() {
            if !self.viewed[slot] {
                continue;
            }
            let Some(Some(place)) = self.slots.get(slot).copied() else {
                continue;
            };
            if self.layout_payload(place.layout)? == Some(payload) {
                self.flush_length(slot)?;
            }
        }
        Ok(())
    }

    /// Sets slot `slot`'s count word as current: its block's count is its length.
    pub(super) fn settled(&self, slot: usize) {
        if let Some(state) = self.sequence_states[slot] {
            self.builder.store(
                self.module.const_zero(self.module.int_type(1)),
                state.dirty,
                1,
            );
        }
    }

    /// A root parameter without a sequence state keeps its count in its block:
    /// store the length its caller handed over, which the block may lag.
    pub(super) fn settle_unstated_roots(&mut self) -> Emitted {
        for (field, &i) in self.signature.roots.iter().enumerate() {
            let Some(first) = self.signature.companions[field] else {
                continue;
            };
            let source = self.signature.abi.parameters[i].source;
            let stated = self
                .unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(source))
                .is_some_and(|s| self.sequence_states[s].is_some());
            if stated {
                continue;
            }
            let pointer = self
                .module
                .parameter(self.function, self.signature.first[i]);
            let length = self.module.parameter(self.function, first);
            self.store_length_if_differs(pointer, self.signature.abi.parameters[i].layout, length)?;
        }
        Ok(())
    }

    pub(super) fn sequence_length(
        &self,
        pointer: Value<'m>,
        layout: LayoutId,
    ) -> Emitted<Value<'m>> {
        let Some(payload) = self.layout_payload(layout)? else {
            return self.invalid("sequence without payload");
        };
        let data = self.payload_of(pointer, payload)?;
        Ok(self
            .builder
            .load_access(self.module.int_type(64), data.ptr, 8, Access::Length))
    }

    pub(super) fn sequence_state_at(&self, place: Place<'m>) -> Option<SequenceState<'m>> {
        self.slots
            .iter()
            .position(|slot| slot.is_some_and(|s| s.ptr == place.ptr))
            .and_then(|i| self.sequence_states[i])
    }

    pub(super) fn invalidate_uniqueness(&self) {
        for state in self.sequence_states.iter().flatten() {
            self.builder.store(
                self.module.const_zero(self.module.int_type(1)),
                state.unique,
                1,
            );
        }
    }

    /// A test found the slot's block unique: then it is no tracked arena block either.
    pub(super) fn mark_untracked(&self, state: SequenceState<'m>, unique: Value<'m>) {
        let i1 = self.module.int_type(1);
        let known = self.builder.load(i1, state.untracked, 1);
        let untracked = self.builder.binary(BinOp::Or, known, unique);
        self.builder.store(untracked, state.untracked, 1);
    }

    /// Whether `block` is no tracked arena block, given the fact `known` that says so: its
    /// count is read only when the fact is unknown, and a static block's read folds.
    pub(super) fn untracked(&self, known: Value<'m>, block: Value<'m>) -> Value<'m> {
        let tracked = crate::runtime::is_tracked(&self.builder, block);
        let untracked = self.builder.binary(
            BinOp::Xor,
            tracked,
            self.module.const_int(self.module.int_type(1), 1),
        );
        self.builder.select(known, known, untracked)
    }

    /// The facts of `state` after growing its block `old` (read before the growth, which
    /// may free it): growth hands back count-one storage for any block but a tracked arena
    /// block, which it leaves alone (MEM-6), so the block stays as tracked as it was.
    pub(super) fn grown_facts(
        &self,
        state: SequenceState<'m>,
        old: Value<'m>,
    ) -> (Value<'m>, Value<'m>) {
        let i1 = self.module.int_type(1);
        if !self.tracking() {
            let yes = self.module.const_int(i1, 1);
            return (yes, yes);
        }
        let unique = self.builder.load(i1, state.unique, 1);
        let untracked = self.builder.load(i1, state.untracked, 1);
        let known = self.builder.binary(BinOp::Or, unique, untracked);
        let grown = self.untracked(known, old);
        (grown, grown)
    }

    pub(super) fn store_sequence_state(&self, address: AddressId, value: ValueId) -> Emitted {
        let Some(slot) = self.whole_slot(address) else {
            return Ok(());
        };
        let Some(state) = self.sequence_states[slot.index()] else {
            return Ok(());
        };
        let length = match self.preserved_lengths[value.index()] {
            Some(length) => length,
            None => self.sequence_length(self.get(value)?, self.value_type(value)?.layout)?,
        };
        self.builder.store(length, state.length, 8);
        // A detach result goes straight into its new owning root; if another use
        // can increase its count, retain the actual test instead of a must fact.
        // A tracked arena block's detach is unshared but never unique (MEM-6).
        let unique = if !self.fresh_detaches.contains(&value) {
            self.module.const_zero(self.module.int_type(1))
        } else if let Some(&untracked) = self.detach_untracked.get(&value) {
            untracked
        } else {
            crate::runtime::is_unique(&self.builder, self.get(value)?)
        };
        self.builder.store(unique, state.unique, 1);
        self.builder.store(unique, state.untracked, 1);
        Ok(())
    }
}
