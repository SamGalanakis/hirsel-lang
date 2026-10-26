use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // The function.

    pub(super) fn emit(mut self) -> Emitted {
        if self.resume.is_some() {
            return self.emit_resume();
        }
        if !self.inputs.alternate
            && self.ownership.is_none()
            && let Some(plan) = self
                .inputs
                .continuation
                .and_then(|p| p.task_entry(self.unit))
        {
            return self.emit_resume_entry(plan);
        }
        self.find_versions()?;
        self.fresh_detaches = self
            .unit
            .instructions
            .iter()
            .filter_map(|inst| match inst {
                Inst::UniqueCopy { result, .. } => Some(*result),
                _ => None,
            })
            .collect();
        for inst in &self.unit.instructions {
            if let Inst::Dup { value } | Inst::Recast { value, .. } = *inst {
                self.fresh_detaches.remove(&value);
            }
        }
        for address in &self.unit.addresses {
            if let AddressBase::Value(value) = address.base {
                self.fresh_detaches.remove(&value);
            }
        }
        for field in &self.unit.pools.fields {
            self.fresh_detaches.remove(&field.value);
        }
        for value in &self.unit.pools.values {
            self.fresh_detaches.remove(value);
        }
        for argument in &self.unit.pools.call_arguments {
            if let ArgumentValue::Value(value) = argument.value {
                self.fresh_detaches.remove(&value);
            }
        }
        let order = self.reverse_postorder()?;
        for _ in &self.unit.blocks {
            self.blocks.push(self.module.append_block(self.function));
        }
        self.create_phis()?;
        self.allocate_slots()?;
        self.bind_parameters()?;
        self.allocate_sequence_states()?;
        self.ahead_init()?;
        self.ownership_init()?;
        for b in order {
            self.emit_block(BlockId(b as u32))?;
        }
        let entry = self.blocks[self.unit.entry.index()];
        // The prologue ends by storing the length of each root parameter that
        // arrives without a sequence state: its caller may have deferred it.
        self.builder.position(self.prologue_block);
        self.settle_unstated_roots()?;
        self.builder.br(entry);
        Ok(())
    }

    /// Storage version names: stores', replaces' and slot-backed parameters' versions, and
    /// every block parameter an edge passes one to.
    pub(super) fn find_versions(&mut self) -> Emitted {
        let unit = self.unit;
        if let Some(entry) = &unit.tables.receipt_entry {
            self.versions.extend(entry.versions());
        }
        for inst in &unit.instructions {
            match *inst {
                Inst::Store { version, .. }
                | Inst::SeqReserve { version, .. }
                | Inst::SeqSetLen { version, .. }
                | Inst::SeqAppend { version, .. }
                | Inst::FlatAppend { version, .. }
                | Inst::FlatField { version, .. }
                | Inst::FlatClose { version, .. } => {
                    self.versions.insert(version);
                }
                Inst::Replace {
                    version,
                    displaced_version,
                    ..
                } => {
                    self.versions.insert(version);
                    self.versions.insert(displaced_version);
                }
                _ => {}
            }
        }
        // A `mut` argument's write-back version names the root slot after the call.
        for terminator in &unit.terminators {
            let Terminator::Call { call, .. } = *terminator else {
                continue;
            };
            let Some(call) = unit.tables.calls.get(call.index()) else {
                return self.invalid("a call outside the call table");
            };
            for argument in self.pool(call.arguments, &unit.pools.call_arguments)? {
                if let ArgumentValue::Address { version, .. } = argument.value {
                    self.versions.insert(version);
                }
            }
            if let Some(receipts) = &call.receipts {
                self.versions
                    .extend(receipts.writebacks().map(|(_, version)| version));
            }
        }
        for parameter in &self.signature.abi.parameters {
            let backed = unit
                .slots
                .iter()
                .any(|s| s.kind == SlotKind::Parameter(parameter.source));
            if backed && let Some(&value) = unit.parameters.get(parameter.source as usize) {
                self.versions.insert(value);
            }
        }
        // Captures name their environment slots' initial storage versions.
        self.versions.extend(unit.captures.iter().copied());
        loop {
            let mut grew = false;
            for edge in &unit.tables.edges {
                let arguments = self.pool(edge.arguments, &unit.pools.values)?;
                let Some(block) = unit.blocks.get(edge.target.index()) else {
                    return self.invalid("an edge to a missing block");
                };
                let parameters = self.pool(block.parameters, &unit.pools.values)?;
                for (a, p) in arguments.iter().zip(parameters) {
                    if self.versions.contains(a) && self.versions.insert(*p) {
                        grew = true;
                    }
                }
            }
            if !grew {
                return Ok(());
            }
        }
    }

    /// Successor blocks of a block over normal and failure edges.
    pub(super) fn successors(&self, b: usize) -> Emitted<Vec<usize>> {
        let unit = self.unit;
        let block = &unit.blocks[b];
        let mut edges = Vec::new();
        let mut failures = Vec::new();
        for inst in self.pool(block.instructions, &unit.instructions)? {
            inst.visit(&mut |r| {
                if let Ref::Failure(f) = r {
                    failures.push(f);
                }
            });
        }
        let Some(terminator) = unit.terminators.get(b) else {
            return self.invalid("a block without a terminator");
        };
        let mut arms = None;
        terminator.visit(&mut |r| match r {
            Ref::Edge(e) => edges.push(e),
            Ref::Failure(f) => failures.push(f),
            Ref::SwitchArms(range) => arms = Some(range),
            _ => {}
        });
        if let Terminator::Call { call, .. } = terminator {
            failures.extend(unit.tables.calls[call.index()].pretransfer_failure);
        }
        if let Some(range) = arms {
            edges.extend(
                self.pool(range, &unit.pools.switch_arms)?
                    .iter()
                    .map(|a| a.edge),
            );
        }
        for f in failures {
            edges.push(self.failure_edge(f)?.cleanup);
        }
        edges
            .into_iter()
            .map(|e| self.edge(e).map(|e| e.target.index()))
            .collect()
    }

    /// Blocks in reverse postorder from the entry, so each definition is emitted before the
    /// uses it dominates. Every block is reachable (the validator checks it).
    pub(super) fn reverse_postorder(&self) -> Emitted<Vec<usize>> {
        let count = self.unit.blocks.len();
        if self.unit.entry.index() >= count {
            return self.invalid("no entry block");
        }
        let adjacent: Vec<Vec<usize>> = (0..count)
            .map(|b| self.successors(b))
            .collect::<Emitted<_>>()?;
        if adjacent.iter().flatten().any(|&b| b >= count) {
            return self.invalid("an edge to a missing block");
        }
        let post = hirsel_graph::reverse_postorder(&adjacent, self.unit.entry.index());
        if post.len() != count {
            return self.invalid("an unreachable block");
        }
        Ok(post)
    }

    pub(super) fn create_phis(&mut self) -> Emitted {
        let unit = self.unit;
        for (b, block) in unit.blocks.iter().enumerate() {
            self.builder.position(self.blocks[b]);
            let mut phis = Vec::new();
            for &p in self.pool(block.parameters, &unit.pools.values)? {
                if self.versions.contains(&p) {
                    phis.push(None);
                    continue;
                }
                let ty = self.llvm_type(self.value_shape(p)?);
                let phi = self.builder.phi(ty);
                self.define(p, phi)?;
                phis.push(Some(phi));
            }
            self.phis.push(phis);
        }
        Ok(())
    }

    /// The place of a constant slot: its bytes in a private read-only global named by their
    /// content, which every read indexes in place and no store writes.
    fn constant_data(&mut self, layout: LayoutId, bytes: &[u8]) -> Emitted<Place<'m>> {
        let align = self.layout(layout)?.align;
        let name = hirsel_lir::statics::constant_data_name(align, bytes);
        let ptr = match self.module.named_global(&name) {
            Some(global) => global,
            None => {
                let ty = self
                    .module
                    .array_type(self.module.int_type(8), bytes.len() as u64);
                let global = self.module.private_constant(&name, ty, align);
                self.module
                    .set_initializer(global, self.module.const_bytes(bytes));
                global
            }
        };
        Ok(Place {
            columns: None,
            ptr,
            layout,
            align,
            block: None,
        })
    }

    pub(super) fn allocate_slots(&mut self) -> Emitted {
        let unit = self.unit;
        for (s, slot) in unit.slots.iter().enumerate() {
            match slot.kind {
                SlotKind::ReceiptParameter(_)
                | SlotKind::ReceiptCapture(_)
                | SlotKind::ReceiptResult => continue,
                // Bound to their environment fields at entry.
                SlotKind::Parameter(_) | SlotKind::Capture(_) => continue,
                SlotKind::Local
                | SlotKind::Temporary
                | SlotKind::ReturnSlot
                | SlotKind::LoopCarried(_)
                | SlotKind::Displaced => {}
            }
            let layout = self.lir_type(slot.ty)?.layout;
            self.slots[s] = Some(match self.constant_slots[s] {
                Some(constant) => self.constant_data(layout, constant.bytes)?,
                None => self.stack(layout)?,
            });
        }
        for region in &unit.tables.regions {
            let record = self
                .module
                .array_type(self.module.int_type(8), region.record_bytes());
            self.regions.push(self.prologue.alloca(record, 8));
        }
        self.resume_storage()
    }
}
