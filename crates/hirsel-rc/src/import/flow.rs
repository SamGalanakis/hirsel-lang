use super::*;

impl Importer<'_> {
    /// One emission pass from the given TIR entry states (None: not yet known).
    /// Returns every block's entry state joined over all its edges, which
    /// equals `prev` at the fixpoint, so each emitted block assumed a state
    /// covering every edge into it.
    pub(super) fn pass(&mut self, prev: &[Option<State>]) -> R<Vec<Option<State>>> {
        self.invocation_live = false;
        self.invocation_return = None;
        self.out = Out::default();
        self.receipt_values.clear();
        let locals = self.body.locals.len();
        self.state = State {
            slots: PVec::default(),
            loans: PVec::default(),
            views: PVec::filled(locals, Tri::No),
        };
        self.views = PVec::filled(locals, None);
        self.versions = PVec::default();
        // Value ids restart each pass, so an earlier pass's facts would name
        // this pass's versions and prove tags no test here established.
        self.facts = Vec::new();
        self.versioned.clear();
        self.constant_version = PVec::default();
        self.chain_memo.clear();
        self.scratch.clear();
        self.outer_scope = false;
        self.unreachable = None;
        self.propagate = None;
        self.prologue_failure = None;
        self.failure = FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        };
        let n = self.body.blocks.len();
        self.incoming = vec![None; n];
        self.entry_versions = vec![None; n];
        self.entry_views = vec![None; n];
        self.entry_facts = vec![None; n];
        self.span = self.body.blocks[self.body.entry.0 as usize].span;
        self.statement = (self.body.entry.0, 0);
        self.setup()?;
        let mut used = vec![None; n];
        let rpo = self.rpo.clone();
        for t in rpo {
            let mut entry = self.incoming[t.0 as usize].clone();
            if let Some(p) = &prev[t.0 as usize] {
                match &mut entry {
                    Some(e) => e.join(p),
                    None => entry = Some(p.clone()),
                }
            }
            let Some(entry) = entry else {
                // Match lowering can prove an arm and omit its remaining
                // edges. The TIR RPO still contains those targets. Leave them
                // un-emitted unless a surviving predecessor reaches them;
                // finish removes their placeholders from the LIR graph.
                continue;
            };
            used[t.0 as usize] = Some(entry.clone());
            self.emit_block(t, entry)?;
        }
        // Back edges reach their targets after their turn in RPO, including
        // blocks deferred above. Seed those blocks for the next pass too.
        for (u, incoming) in used.iter_mut().zip(&self.incoming) {
            if let Some(i) = incoming {
                match u {
                    Some(u) => u.join(i),
                    None => *u = Some(i.clone()),
                }
            }
        }
        Ok(used)
    }

    /// Slots, parameters, loans and the prologue.
    pub(super) fn setup(&mut self) -> R<()> {
        let body = self.body;
        let view = self.instances.view(self.id);
        let homes: Vec<Home> = match self.plan {
            Some(plan) => plan.homes.clone(),
            None => vec![Home::Parameter; body.parameters.len()],
        };
        let parameter_of: HashMap<u32, u32> = body
            .parameters
            .iter()
            .enumerate()
            .map(|(k, l)| (l.0, k as u32))
            .collect();
        let mut capture = 0u32;
        // Locals receiving displaced content of a Replace.
        let displaced: BTreeSet<u32> = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| match &i.kind {
                tir::InstructionKind::Replace { displaced, .. } => {
                    Some(body.places[displaced.0 as usize].root.0)
                }
                _ => None,
            })
            .collect();
        self.local_slot.clear();
        self.loop_slots = vec![None; body.parameters.len()];
        let mut parameter_slots: Vec<Option<SlotId>> = vec![
                None;
                self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize]
                    .parameters
                    .len()
            ];
        let direct_parameters: HashMap<_, _> = self
            .instances
            .direct_captures
            .get(&self.id)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ordinal, capture)| (capture.local, (ordinal, capture.layout)))
            .collect();
        for (i, local) in body.locals.iter().enumerate() {
            if self
                .instances
                .elided
                .get(&self.id)
                .is_some_and(|locals| locals.contains(&tir::LocalId(i as u32)))
                || self.builders.elements.contains(&tir::LocalId(i as u32))
                || self.fields.locals.contains(&tir::LocalId(i as u32))
            {
                self.local_slot.push(None);
                continue;
            }
            let direct = direct_parameters.get(&tir::LocalId(i as u32)).copied();
            let ty = if let Some((_, layout)) = direct {
                self.intern(layout)
            } else if let Some(&layout) = self.builders.locals.get(&tir::LocalId(i as u32)) {
                self.intern(layout)
            } else if let Some(&layout) = self
                .instances
                .data_locals
                .get(&(self.id, tir::LocalId(i as u32)))
            {
                self.intern(layout)
            } else {
                self.lir_type(local.ty)?
            };
            let scope = ScopeId(local.scope.0);
            let slot = match local.storage {
                tir::Storage::Parameter => {
                    let Some(&k) = parameter_of.get(&(i as u32)) else {
                        return malformed(format!("parameter local {i} is not a parameter"));
                    };
                    match homes[k as usize] {
                        Home::Parameter => {
                            let s = self.new_slot(ty, SlotKind::Parameter(k), scope);
                            parameter_slots[k as usize] = Some(s);
                            s
                        }
                        Home::Loop => {
                            let s = self.new_slot(ty, SlotKind::LoopCarried(k), scope);
                            self.loop_slots[k as usize] = Some(s);
                            s
                        }
                    }
                }
                tir::Storage::Local => self.new_slot(ty, SlotKind::Local, scope),
                tir::Storage::Temporary if displaced.contains(&(i as u32)) => {
                    self.new_slot(ty, SlotKind::Displaced, scope)
                }
                tir::Storage::Temporary => self.new_slot(ty, SlotKind::Temporary, scope),
                tir::Storage::Capture => {
                    if let Some((ordinal, _)) = direct {
                        let k = body.parameters.len() + ordinal;
                        let slot = self.new_slot(ty, SlotKind::Parameter(k as u32), scope);
                        parameter_slots[k] = Some(slot);
                        slot
                    } else {
                        capture += 1;
                        self.new_slot(ty, SlotKind::Capture(capture - 1), scope)
                    }
                }
                tir::Storage::ReturnSlot => self.new_slot(ty, SlotKind::ReturnSlot, scope),
            };
            self.local_slot.push(Some(slot));
        }
        if let Some(environment) = self.instances.environments[self.id.index()]
            && let LayoutKind::Environment { .. } = self.layouts.record(environment).kind
        {
            let word = self
                .layouts
                .record(environment)
                .fields
                .last()
                .expect("live word")
                .layout;
            let ty = self.intern(word);
            self.new_slot(ty, SlotKind::Capture(capture), ScopeId(0));
        }
        self.flat_copies()?;
        self.encoding_copies()?;
        // A loop-carried borrowed parameter still arrives in its caller's storage.
        let abi =
            self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize].clone();
        for (k, home) in homes.iter().enumerate() {
            if *home == Home::Loop && abi.parameters[k].convention == Passing::Borrow {
                let local = body.parameters[k];
                let ty = self.out.slots[self.slot_for(local.0 as usize)?.index()].ty;
                let scope = ScopeId(body.locals[local.0 as usize].scope.0);
                parameter_slots[k] = Some(self.new_slot(ty, SlotKind::Parameter(k as u32), scope));
            }
        }
        let _ = view;
        // Declared parameters, in instance ordinal order.
        self.out.parameters.clear();
        for (k, p) in abi.parameters.iter().enumerate() {
            let ty = self.intern(p.layout);
            let value = match parameter_slots[k] {
                Some(slot) => {
                    let v = self.value(ty, Holding::Plain);
                    self.constant_version[slot.index()] = Some(v);
                    self.versions[slot.index()] = Some(v);
                    v
                }
                None => self.fresh(ty),
            };
            self.out.parameters.push(value);
        }
        let mut captures = Vec::new();
        for s in 0..self.out.slots.len() {
            let slot = &self.out.slots[s];
            if let SlotKind::Capture(i) = slot.kind {
                let ty = slot.ty;
                let v = self.value(ty, Holding::Plain);
                self.constant_version[s] = Some(v);
                self.versions[s] = Some(v);
                captures.push((i, v));
            }
        }
        captures.sort();
        self.capture_versions = captures.into_iter().map(|(_, v)| v).collect();
        if let Some(environment) = self.instances.environments[self.id.index()]
            && matches!(
                self.layouts.record(environment).kind,
                LayoutKind::Environment { .. }
            )
        {
            let ordinal = self
                .layouts
                .record(environment)
                .fields
                .last()
                .expect("live word")
                .ordinal;
            let slot = SlotId(
                self.out
                    .slots
                    .iter()
                    .position(|s| s.kind == SlotKind::Capture(ordinal))
                    .expect("live word slot") as u32,
            );
            self.out.tables.init_flags.push(hirsel_lir::ir::InitFlag {
                path: PoolRange::EMPTY,
                value: self.capture_versions[ordinal as usize],
                slot,
                environment: Some(environment),
            });
        }
        if let Some(source) = self.instances.receipt_source(self.id) {
            let source = source.clone();
            let contract = &source.contract;
            if !contract.captures.is_empty() || !contract.inventory.is_empty() {
                return unsupported(
                    "receipt entry requires data parameters and results without captures or inventory",
                );
            }
            let mut parameters = Vec::new();
            for parameter in &contract.parameters {
                let profile = source
                    .profile(hirsel_lir::sealed_payload_source::ReceiptPort::Parameter(
                        parameter.source,
                    ))
                    .ok_or_else(|| {
                        PassError::Malformed("receipt entry lacks full original profile".into())
                    })?;
                if !profile.functions.is_empty() {
                    return unsupported(
                        "receipt boundary callable profiles require environment custody",
                    );
                }
                let layout = self.layouts.payload_receipt(&profile.root.ty);
                let ty = self.intern(layout);
                let slot =
                    self.new_slot(ty, SlotKind::ReceiptParameter(parameter.source), ScopeId(0));
                let version = self.value(ty, Holding::Plain);
                self.constant_version[slot.index()] = Some(version);
                self.versions[slot.index()] = Some(version);
                parameters.push(hirsel_lir::ir::ReceiptEntrySlot {
                    source: parameter.source,
                    slot,
                    version,
                });
            }
            let result = if contract.result.is_some() {
                let profile = source
                    .profile(hirsel_lir::sealed_payload_source::ReceiptPort::Result)
                    .ok_or_else(|| {
                        PassError::Malformed("receipt result original profile".into())
                    })?;
                if !profile.functions.is_empty() {
                    return unsupported(
                        "receipt result callable profiles require environment custody",
                    );
                }
                let layout = self.layouts.payload_receipt(&profile.root.ty);
                let ty = self.intern(layout);
                Some(self.new_slot(ty, SlotKind::ReceiptResult, ScopeId(0)))
            } else {
                None
            };
            self.out.tables.receipt_entry = Some(hirsel_lir::ir::ReceiptEntry {
                contract: contract
                    .content()
                    .map_err(|e| PassError::Malformed(e.into()))?,
                parameters,
                captures: vec![],
                result,
                environment: None,
            });
        }
        self.hole_slots()?;
        for (s, slot) in self.out.slots.iter().enumerate() {
            let entered = matches!(
                slot.kind,
                SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReceiptParameter(_)
            );
            self.state.slots[s] = SlotState {
                live: if entered
                    || matches!(slot.kind, SlotKind::ReturnSlot | SlotKind::ReceiptResult)
                {
                    Tri::Yes
                } else {
                    Tri::No
                },
                init: if entered { Tri::Yes } else { Tri::No },
                moved: BTreeMap::new(),
            };
        }
        self.mark_versioned();
        self.version_holes();
        if self.out.tables.environment().is_some() {
            let flag = self
                .out
                .tables
                .init_flags
                .iter()
                .find(|flag| flag.environment.is_some())
                .expect("live word");
            self.versioned[flag.slot.index()] = true;
        }
        // The prologue: fixed-address preparations, then loop-slot entry.
        self.prologue = self.block(BlockKind::Body);
        self.current = self.prologue;
        self.loans()?;
        self.tir_block = vec![None; body.blocks.len()];
        for &t in &self.rpo.clone() {
            let b = self.block(BlockKind::Body);
            self.tir_block[t.0 as usize] = Some(b);
        }
        self.split(BlockKind::Body);
        if self.plan.is_some() {
            self.enter_loop(&parameter_slots)?;
            self.enter_holes()?;
        }
        let entry = self.body.entry;
        let e = self.enter(entry);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Slots that some operation after entry writes, so reads at a merge may
    /// see different versions.
    pub(super) fn mark_versioned(&mut self) {
        use tir::InstructionKind as K;
        let body = self.body;
        let root = |p: &tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
        let mut written = BTreeSet::new();
        for block in &body.blocks {
            for i in &block.instructions {
                match &i.kind {
                    K::Assign { destination, .. } | K::Snapshot { destination, .. } => {
                        written.insert(root(destination));
                    }
                    K::Replace {
                        destination,
                        displaced,
                        ..
                    } => {
                        written.insert(root(destination));
                        written.insert(root(displaced));
                    }
                    _ => {}
                }
            }
            if let tir::Terminator::Call {
                returns, arguments, ..
            } = &block.terminator
            {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    written.insert(root(destination));
                }
                for a in arguments {
                    if a.value.usage == UseKind::Mut {
                        written.insert(root(&a.value.place));
                    }
                }
            }
        }
        for local in written {
            if let Some(slot) = self.local_slot[local] {
                self.versioned[slot.index()] = true;
            }
        }
        if let Some(entry) = &self.out.tables.receipt_entry {
            for binding in &entry.parameters {
                if self.own_abi().receipts.as_ref().is_some_and(|contract| {
                    contract.parameters.iter().any(|p| {
                        p.source == binding.source
                            && p.mode == hirsel_lir::sealed_payload_source::ReceiptMode::MutSlot
                    })
                }) {
                    self.versioned[binding.slot.index()] = true;
                }
            }
        }
        for slot in self.loop_slots.clone().into_iter().flatten() {
            self.versioned[slot.index()] = true;
        }
    }

    /// One LIR loan per TIR loan. A loan of a fixed place names its prologue
    /// address; a loan of a computed place (an element, a variant field)
    /// names the address prepared at its one `BeginLoan`.
    pub(super) fn loans(&mut self) -> R<()> {
        let body = self.body;
        let mut begins: BTreeMap<u32, (tir::PlaceId, bool, u32)> = BTreeMap::new();
        let mut max = None::<u32>;
        for block in &body.blocks {
            for i in &block.instructions {
                if let tir::InstructionKind::BeginLoan {
                    id, place, mutable, ..
                } = &i.kind
                {
                    begins.entry(id.0).or_insert((*place, *mutable, 0)).2 += 1;
                }
            }
        }
        for place in &body.places {
            if let Some(l) = place.loan {
                max = Some(max.map_or(l.0, |m: u32| m.max(l.0)));
            }
        }
        if let Some((&last, _)) = begins.iter().next_back() {
            max = Some(max.map_or(last, |m| m.max(last)));
        }
        self.loan_map.clear();
        let Some(max) = max else {
            return Ok(());
        };
        for l in 0..=max {
            let Some(&(place, mutable, count)) = begins.get(&l) else {
                // A loan named only by places, never begun here: no LIR loan.
                self.loan_map.push(LoanId(u32::MAX));
                continue;
            };
            // A transient slice forwards the consumer's read loan onto its
            // stable String owner instead of borrowing an owning Bytes slot.
            let place = self
                .slices
                .sources
                .values()
                .find(|slice| slice.loan.0 == l)
                .map_or(place, |slice| slice.source);
            if self.elided_place(place) || self.elided_loan(tir::LoanId(l)) {
                self.loan_map.push(LoanId(u32::MAX));
                continue;
            }
            let address = if self.joined_root(place).is_some() {
                // Its backing SSA value exists only when BeginLoan executes.
                UNPREPARED
            } else {
                let p = self.place(place)?;
                if p.fixed {
                    self.fixed_address(&p)?
                } else if count == 1 {
                    // Prepared at the BeginLoan.
                    UNPREPARED
                } else {
                    return unsupported(format!(
                        "loan {l} of a computed place begun at {count} sites"
                    ));
                }
            };
            let id = self.new_loan(LoanInfo { address, mutable }, Tri::No);
            self.loan_map.push(id);
        }
        Ok(())
    }

    pub(super) fn loan(&self, l: tir::LoanId) -> R<LoanId> {
        match self.loan_map.get(l.0 as usize) {
            Some(&id) if id.0 != u32::MAX => Ok(id),
            _ => malformed(format!("loan {} has no BeginLoan", l.0)),
        }
    }

    /// Jump into TIR block `t`, recording the state and versions it receives.
    pub(super) fn enter(&mut self, t: tir::BlockId) -> EdgeId {
        if self.invocation_live && self.invocation_return == Some(t) {
            self.push(Inst::FrameLeave);
            self.invocation_live = false;
        }
        let target = self.tir_block[t.0 as usize].expect("reachable TIR block");
        let e = self.edge(target, Vec::new());
        let slot = &mut self.incoming[t.0 as usize];
        match slot {
            Some(s) => s.join(&self.state),
            None => *slot = Some(self.state.clone()),
        }
        self.entry_versions[t.0 as usize] = Some(self.versions.clone());
        self.entry_views[t.0 as usize] = Some(self.views.clone());
        self.entry_facts[t.0 as usize] = Some(self.facts.clone());
        e
    }

    pub(super) fn merge(&self, t: tir::BlockId) -> bool {
        let extra = u32::from(t == self.body.entry);
        self.tir_preds[t.0 as usize] + extra >= 2
    }

    pub(super) fn emit_block(&mut self, t: tir::BlockId, entry: State) -> R<()> {
        self.invocation_live = false;
        self.invocation_return = None;
        let b = self.tir_block[t.0 as usize].expect("reachable TIR block");
        let block = &self.body.blocks[t.0 as usize];
        self.current = b;
        self.span = block.span;
        self.statement = (t.0, 0);
        let anchor = self.anchor_here();
        self.out.blocks[b.index()].anchor = anchor;
        self.state = entry;
        if self.merge(t) {
            let mut phis = Vec::new();
            let mut versions = self.constant_version.clone();
            // Empty storage joins no version: only the slots holding something.
            let held: Vec<usize> = self
                .state
                .slots
                .lacking(EMPTY)
                .into_iter()
                .filter(|&(s, st)| {
                    s < versions.len()
                        && self.versioned[s]
                        && st.live == Tri::Yes
                        && st.init == Tri::Yes
                })
                .map(|(s, _)| s)
                .collect();
            for s in held {
                let ty = self.out.slots[s].ty;
                let p = self.value(ty, Holding::Plain);
                phis.push((SlotId(s as u32), p));
                versions[s] = Some(p);
            }
            self.out.blocks[b.index()].phis = phis;
            self.versions = versions;
            // A joined view held on every edge joins into a parameter whose
            // loan `finish` reads off its incoming views.
            let mut view_phis = Vec::new();
            let mut views = PVec::filled(self.body.locals.len(), None);
            let held: Vec<usize> = self
                .state
                .views
                .lacking(DEAD)
                .into_iter()
                .filter(|&(l, view)| self.joined[l] && *view == Tri::Yes)
                .map(|(l, _)| l)
                .collect();
            for l in held {
                let ty = self.out.slots[self.slot_for(l)?.index()].ty;
                let holding = self.joined_holding(l, ty)?;
                let p = self.value(ty, holding);
                view_phis.push((l, p));
                views[l] = Some(p);
            }
            self.out.blocks[b.index()].view_phis = view_phis;
            self.views = views;
            self.facts = Vec::new();
        } else {
            self.versions = self.entry_versions[t.0 as usize]
                .clone()
                .unwrap_or_else(|| self.constant_version.clone());
            // Blocks emitted since the edge's snapshot may have made slots; theirs
            // hold no version here, and `new_slot` indexes by id.
            let slots = self.out.slots.len();
            if self.versions.len() < slots {
                self.versions.resize(slots, None);
            }
            self.views = self.entry_views[t.0 as usize]
                .clone()
                .unwrap_or_else(|| PVec::filled(self.body.locals.len(), None));
            self.facts = self.entry_facts[t.0 as usize].clone().unwrap_or_default();
        }
        // An earlier block may have created cached disposal slots after this
        // block's incoming state was recorded. Reusing one here needs the
        // same dead storage and absent versions as restoring an emission point.
        self.pad();
        for (j, instruction) in block.instructions.iter().enumerate() {
            self.statement = (t.0, j as u32);
            self.span = instruction.span;
            self.failure = FailureContext {
                cleanup: Some(instruction.on_failure),
                before: Vec::new(),
                pending: Vec::new(),
            };
            self.instruction(&instruction.kind)?;
        }
        self.statement = (t.0, block.instructions.len() as u32);
        self.span = block.span;
        self.failure = FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        };
        self.terminator(t, &block.terminator)
    }
}
