use super::*;
use crate::PassError;

impl Importer<'_> {
    // --- places and addresses ---

    pub(super) fn slot_for(&self, local: usize) -> R<SlotId> {
        self.local_slot
            .get(local)
            .copied()
            .flatten()
            .ok_or_else(|| {
                PassError::Malformed(format!(
                    "a runtime use of elided generic function local {local}"
                ))
            })
    }

    pub(super) fn elided_place(&self, place: tir::PlaceId) -> bool {
        let root = self.body.places[place.0 as usize].root;
        self.instances
            .elided
            .get(&self.id)
            .is_some_and(|locals| locals.contains(&root))
            || self.builders.elements.contains(&root)
            || self.fields.locals.contains(&root)
    }

    pub(super) fn elided_loan(&self, loan: tir::LoanId) -> bool {
        self.elided_loans.contains(&loan)
    }

    /// Resolve a TIR place to its slot, steps and type.
    pub(super) fn place(&mut self, id: tir::PlaceId) -> R<PlaceRef> {
        let Some(place) = self.body.places.get(id.0 as usize) else {
            return malformed(format!("place {} outside the body", id.0));
        };
        let source_place = Some(self.source_places[id.0 as usize]);
        // A field access can extend a computed origin without binding a new
        // origin of its own. Preserve that prefix's site for its bounds check.
        let site = self.origin_sites.get(&id).copied().or_else(|| {
            prefix_origin_site(
                self.origin_sites.iter().filter_map(|(id, site)| {
                    let prefix = &self.body.places[id.0 as usize];
                    (prefix.root == place.root
                        && prefix.projection.iter().any(|step| {
                            matches!(
                                step,
                                tir::Projection::Index(_) | tir::Projection::VariantField { .. }
                            )
                        })
                        && place.projection.starts_with(&prefix.projection))
                    .then_some((prefix.projection.len(), *site))
                }),
                |span| self.site(span).source,
            )
        });
        let temporary = self.out.slots[self.slot_for(place.root.0 as usize)?.index()].kind
            == SlotKind::Temporary;
        // A view temporary reads through the place it views.
        let mut root = place.root;
        let mut projection: Vec<tir::Projection> = place.projection.iter().cloned().collect();
        let mut loan = place.loan;
        while let Some(base) = self.aliases[root.0 as usize] {
            let b = &self.body.places[base.0 as usize];
            let mut full: Vec<tir::Projection> = b.projection.iter().cloned().collect();
            full.extend(projection);
            projection = full;
            loan = loan.or(b.loan);
            root = b.root;
        }
        let value = if self.joined[root.0 as usize] {
            match (
                self.state.views[root.0 as usize],
                self.views[root.0 as usize],
            ) {
                (Tri::Yes, Some(value)) => Some(value),
                _ => {
                    return unsupported(format!(
                        "joined view local {} not held at {:?} (place {})",
                        root.0, self.statement, id.0
                    ));
                }
            }
        } else {
            None
        };
        let slot = self.slot_for(root.0 as usize)?;
        let mut steps = Vec::new();
        let mut fixed = value.is_none();
        let mut crosses = false;
        let mut flat = false;
        let mut layout = self.layout_of(self.out.slots[slot.index()].ty);
        let mut lowered = Vec::new();
        // AddressBase::Value starts at a block's payload. Preserve the logical
        // payload path for slot analysis, but do not dereference it twice.
        let mut projections = projection.as_slice();
        if value.is_some() {
            match self.layouts.record(layout).kind {
                LayoutKind::Box { payload } => {
                    if projections.first() != Some(&tir::Projection::Field(0)) {
                        return unsupported("a whole joined box as an address");
                    }
                    layout = payload;
                    projections = &projections[1..];
                    steps.push(Step::Payload);
                }
                LayoutKind::Block { payload } => layout = payload,
                _ => {}
            }
        }
        for p in projections {
            if let LayoutKind::Box { payload } = self.layouts.record(layout).kind {
                if *p != tir::Projection::Field(0) {
                    return malformed("a Box projection without its payload step");
                }
                fixed = false;
                crosses = temporary;
                layout = payload;
                steps.push(Step::Payload);
                lowered.push(ProjectionStep::Payload);
                continue;
            }
            if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                self.layouts.record(layout).kind
            {
                fixed = false;
                crosses = temporary;
                layout = payload;
            }
            // A variable flat value inside a flat value is behind its length
            // prefix: its parts are parts of the prefixed encoding's content.
            if let Some((8, content)) = flat_prefixed(self.layouts, layout)
                && !flat_fixed(self.layouts, content)
            {
                steps.push(Step::Field(0));
                lowered.push(ProjectionStep::Field(0));
                layout = content;
            }
            let map_index = matches!(p, tir::Projection::Index(_))
                && matches!(&self.layouts.record(layout).kind, LayoutKind::Opaque { owner } if *owner == hirsel_syntax::DefPath::core("Map"));
            // A container's public element place is entries[i], in its private record.
            if matches!(p, tir::Projection::Index(_))
                && matches!(&self.layouts.record(layout).kind, LayoutKind::Opaque { owner } if ["Map", "Set", "SortedMap"].iter().any(|name| *owner == hirsel_syntax::DefPath::core(name)))
            {
                steps.push(Step::Field(0));
                lowered.push(ProjectionStep::Field(0));
                layout = self.layouts.record(layout).fields[0].layout;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.layouts.record(layout).kind
                {
                    layout = payload;
                }
            }
            lowered.push(match *p {
                tir::Projection::Field(field) => ProjectionStep::Field(field),
                tir::Projection::VariantField { variant, field } => {
                    ProjectionStep::VariantField { variant, field }
                }
                tir::Projection::Index(index) => ProjectionStep::Index(index),
            });
            let record = self.layouts.record(layout);
            if matches!(record.kind, LayoutKind::Flat) {
                // A part of a flat value is its FlatFormatV0 encoding at its
                // cursor, which follows earlier variable fields' prefixes: its
                // address is prepared where it is used.
                fixed = false;
                flat = true;
            }
            let next = match p {
                tir::Projection::Field(f) => {
                    steps.push(Step::Field(*f));
                    record
                        .fields
                        .iter()
                        .find(|x| x.ordinal == *f)
                        .map(|x| x.layout)
                }
                tir::Projection::VariantField { variant, field } => {
                    fixed = false;
                    steps.push(Step::Variant(*variant, *field));
                    record
                        .tag
                        .iter()
                        .flat_map(|t| &t.variants)
                        .find(|v| v.ordinal == *variant)
                        .and_then(|v| v.fields.iter().find(|x| x.ordinal == *field))
                        .map(|x| x.layout)
                }
                tir::Projection::Index(_) => {
                    fixed = false;
                    steps.push(Step::Index);
                    if matches!(record.kind, LayoutKind::Inline { .. }) {
                        // A computed position in inline storage: a drop of
                        // the rest cannot skip it.
                        crosses = temporary;
                    }
                    match record.kind {
                        // Element i of a flat sequence, at its cursor.
                        LayoutKind::Flat => flat_sequence(self.layouts, layout),
                        LayoutKind::Inline { element, .. }
                        | LayoutKind::Simd { element, .. }
                        | LayoutKind::Slice { element }
                        | LayoutKind::SoaColumns { element }
                        | LayoutKind::Sequence { element }
                        // Element i of a sequential Packed sequence, walked to (MEM-13).
                        | LayoutKind::Sequential { element } => Some(element),
                        _ => None,
                    }
                }
            };
            let Some(next) = next else {
                return unsupported(format!(
                    "projection {p:?} of a {:?} layout",
                    self.layouts.record(layout).kind
                ));
            };
            layout = next;
            if map_index {
                steps.push(Step::Variant(1, 0));
                lowered.push(ProjectionStep::VariantField {
                    variant: 1,
                    field: 0,
                });
                fixed = false;
                layout = self
                    .layouts
                    .record(layout)
                    .tag
                    .as_ref()
                    .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
                    .and_then(|v| v.fields.first())
                    .map(|f| f.layout)
                    .ok_or_else(|| {
                        PassError::Malformed("a map entry without a live pair".into())
                    })?;
            }
        }
        let ty = if place.projection.is_empty() {
            if let Some(&layout) = self.instances.data_locals.get(&(self.id, root)) {
                self.intern(layout)
            } else if let Some(&layout) = self.builders.locals.get(&root) {
                self.intern(layout)
            } else {
                self.lir_type(place.ty)?
            }
        } else {
            self.lir_type(place.ty)?
        };
        if flat && self.layout_of(ty) != layout {
            // Only a fixed-size part has its own type's layout; a variable one
            // is a length-prefixed encoding, read by flat-aware operations.
            if flat_fixed(self.layouts, layout) {
                return unsupported("a fixed flat record part at its source type");
            }
            return Ok(PlaceRef {
                value,
                source_place,
                slot,
                steps,
                projection: lowered,
                ty: self.intern(layout),
                fixed,
                crosses,
                loan,
                source: Some((ty, self.ty(place.ty)?)),
                site,
            });
        }
        if self.layout_of(ty) != layout {
            return malformed(format!(
                "place {} has type layout {:?} but its projection reaches {:?}",
                id.0,
                self.layouts.record(self.layout_of(ty)).kind,
                self.layouts.record(layout).kind
            ));
        }
        Ok(PlaceRef {
            value,
            source_place,
            slot,
            steps,
            projection: lowered,
            ty,
            fixed,
            crosses,
            loan,
            source: None,
            site,
        })
    }

    /// The prologue-prepared address of a fixed place.
    pub(super) fn fixed_address(&mut self, p: &PlaceRef) -> R<AddressId> {
        let steps: Vec<ProjectionStep> = p
            .projection
            .iter()
            .map(|x| match x {
                ProjectionStep::Field(f) => ProjectionStep::Field(*f),
                _ => unreachable!("a fixed place has only field steps"),
            })
            .collect();
        let key = (p.slot, steps.clone());
        if let Some(&a) = self.out.fixed_addresses.get(&key) {
            return Ok(a);
        }
        let projection = PoolRange::push(&mut self.out.pools.projections, steps);
        let a = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(p.slot),
            projection,
            ty: p.ty,
        });
        self.out.fixed_addresses.insert(key, a);
        let failure = self.prologue_failure_edge()?;
        self.out.blocks[self.prologue.index()]
            .insts
            .push(Inst::Address {
                address: a,
                failure,
            });
        Ok(a)
    }

    /// A failure edge from the prologue: nothing has run yet, so its cleanup
    /// releases what the caller handed over (owned parameters, as slots or as
    /// values). Field-path preparations never fire it, but each names one.
    pub(super) fn prologue_failure_edge(&mut self) -> R<FailureId> {
        let pending: Vec<ValueId> = self
            .out
            .parameters
            .iter()
            .copied()
            .filter(|&v| self.holds(v))
            .collect();
        let target = match self.prologue_failure {
            Some(b) => b,
            None => {
                let b = self.block(BlockKind::FailureCleanup);
                self.prologue_failure = Some(b);
                let saved = self.save();
                self.state = self.initial_state();
                let actions = self.state_actions();
                self.current = b;
                self.fill_dispose(&pending, &actions, false)?;
                self.restore(saved);
                b
            }
        };
        let anchor = self.anchor_here();
        let edge = self.edge(target, pending);
        Ok(self.failure_edge(edge, None, anchor))
    }

    pub(super) fn save(
        &self,
    ) -> (
        BlockId,
        State,
        PVec<Option<ValueId>>,
        bool,
        Option<tir::BlockId>,
    ) {
        (
            self.current,
            self.state.clone(),
            self.versions.clone(),
            self.invocation_live,
            self.invocation_return,
        )
    }

    /// Restore an emission point; slots created meanwhile stay, dead.
    pub(super) fn restore(
        &mut self,
        saved: (
            BlockId,
            State,
            PVec<Option<ValueId>>,
            bool,
            Option<tir::BlockId>,
        ),
    ) {
        (
            self.current,
            self.state,
            self.versions,
            self.invocation_live,
            self.invocation_return,
        ) = saved;
        self.pad();
    }

    pub(super) fn pad(&mut self) {
        let n = self.out.slots.len();
        self.state.slots.resize(n, SlotState::DEAD);
        self.versions.resize(n, None);
        let l = self.out.tables.loans.len();
        self.state.loans.resize(l, Tri::No);
    }

    /// The state at function entry.
    pub(super) fn initial_state(&self) -> State {
        State {
            slots: self
                .out
                .slots
                .iter()
                .map(|slot| {
                    let entered = matches!(
                        slot.kind,
                        SlotKind::Parameter(_)
                            | SlotKind::Capture(_)
                            | SlotKind::ReceiptParameter(_)
                    );
                    SlotState {
                        live: if entered || slot.kind == SlotKind::ReturnSlot {
                            Tri::Yes
                        } else {
                            Tri::No
                        },
                        init: if entered { Tri::Yes } else { Tri::No },
                        moved: BTreeMap::new(),
                    }
                })
                .collect(),
            loans: PVec::filled(self.out.tables.loans.len(), Tri::No),
            views: PVec::filled(self.body.locals.len(), Tri::No),
        }
    }

    pub(super) fn failure_edge(
        &mut self,
        cleanup: EdgeId,
        rule: Option<String>,
        anchor: SourceAnchorId,
    ) -> FailureId {
        let id = FailureId(self.out.tables.failures.len() as u32);
        self.out.tables.failures.push(FailureEdge {
            cleanup,
            rule,
            anchor,
            dispose: PoolRange::EMPTY,
        });
        id
    }

    /// The address of a place, prepared here when it is computed. Index
    /// operands are read first; a bounds or tag check fails along the current
    /// failure successor with its rule, at the site of the origin binding
    /// that named the place when there was one.
    pub(super) fn address(&mut self, p: &PlaceRef) -> R<AddressId> {
        if p.fixed {
            return self.fixed_address(p);
        }
        let (a, rule) = self.named_address(p)?;
        let span = self.span;
        self.span = p.site.unwrap_or(span);
        let failure = self.fail(rule.map(String::from));
        self.span = span;
        self.push(Inst::Address {
            address: a,
            failure: failure?,
        });
        Ok(a)
    }

    /// The address an origin event names. The event binds a name and never
    /// accesses the place (the interpreter's `BindOrigin` computes it without
    /// failing), so a dynamic address is not prepared: an index or tag check
    /// runs where the place is accessed, after the operands evaluated before
    /// that access, such as an assignment's right-hand value.
    pub(super) fn origin_address(&mut self, p: &PlaceRef) -> R<AddressId> {
        if p.fixed {
            return self.fixed_address(p);
        }
        Ok(self.named_address(p)?.0)
    }

    /// A dynamic place's address and the rule its preparation checks, with
    /// its index operands read here; nothing prepares it.
    pub(super) fn named_address(&mut self, p: &PlaceRef) -> R<(AddressId, Option<&'static str>)> {
        let mut steps = Vec::new();
        let mut rule = None;
        let mut unchecked = Vec::new();
        for (ordinal, x) in p.projection.iter().enumerate() {
            let check = match x {
                ProjectionStep::Index(_) => Some("MEM-14"),
                ProjectionStep::VariantField { .. } => Some("MEM-13"),
                _ => None,
            };
            let proved = p.source_place.zip(check).is_some_and(|(place, rule)| {
                self.discharged(Form::Projection {
                    place,
                    rule: rule.into(),
                })
            });
            if proved {
                unchecked.push(ordinal as u32);
            }
            steps.push(match x {
                ProjectionStep::Payload => ProjectionStep::Payload,
                ProjectionStep::Field(f) => ProjectionStep::Field(*f),
                ProjectionStep::VariantField { variant, field } => {
                    if !proved {
                        rule.get_or_insert("MEM-13");
                    }
                    ProjectionStep::VariantField {
                        variant: *variant,
                        field: *field,
                    }
                }
                ProjectionStep::Index(local) => {
                    if !proved {
                        rule = Some("MEM-14");
                    }
                    let index = self.read_local_bits(*local)?;
                    ProjectionStep::Index(index)
                }
                ProjectionStep::Slot(_) | ProjectionStep::FlatAt { .. } => {
                    return malformed("library addressing in a TIR place");
                }
            });
        }
        let projection = PoolRange::push(&mut self.out.pools.projections, steps);
        let a = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            source_place: p.source_place,
            unchecked,
            base: p
                .value
                .map_or(AddressBase::Slot(p.slot), AddressBase::Value),
            projection,
            ty: p.ty,
        });
        Ok((a, rule))
    }

    /// Read a whole plain local (an index operand).
    pub(super) fn read_local_bits(&mut self, local: tir::LocalId) -> R<ValueId> {
        let slot = self.slot_for(local.0 as usize)?;
        let ty = self.out.slots[slot.index()].ty;
        let p = PlaceRef {
            value: None,
            source_place: None,
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let address = self.fixed_address(&p)?;
        let reaching = self.reaching(slot)?;
        let result = self.value(ty, Holding::Plain);
        let mode = if self.bits(ty) {
            ReadMode::Bits
        } else {
            return unsupported("a non-plain index operand");
        };
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode,
        });
        Ok(result)
    }

    /// The reaching version of a slot.
    pub(super) fn reaching(&self, slot: SlotId) -> R<ValueId> {
        match self.versions[slot.index()] {
            Some(v) => Ok(v),
            None => malformed(format!(
                "read of slot {} ({:?}) with no reaching version",
                slot.0,
                self.out.slots[slot.index()].kind
            )),
        }
    }

    // --- operands ---

    /// Read an operand at its use. `Copy` makes an owned copy (a plain read
    /// for bits), `Take` moves the content out, `Borrow` reads a view inside
    /// the operand's loan. A copy may fail; owned values read earlier in the
    /// same operation are pending on that failure.
    pub(super) fn operand(&mut self, op: &tir::Operand) -> R<ValueId> {
        if let Some(view) = self.joined_view(op) {
            let value = view?;
            if op.usage == UseKind::Take {
                let local = self.joined_local(op.place).expect("whole joined operand");
                self.state.views[local] = Tri::No;
                self.views[local] = None;
            }
            return Ok(value);
        }
        if let Some(view) = self.whole_joined(op.place) {
            let reaching = view?;
            let ty = self.value_type(reaching);
            if matches!(
                self.out.types[ty.index()].repr,
                ValueRepr::Block | ValueRepr::Uncounted
            ) {
                let result = self.fresh(ty);
                self.push(Inst::CopyValue {
                    result,
                    value: reaching,
                });
                self.push(Inst::Share { value: result });
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                return Ok(result);
            }
            let address = self.view_address(reaching)?;
            let result = self.fresh(ty);
            if self.bits(ty) {
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                });
            } else {
                let failure = self.fail(None)?;
                self.push(Inst::Copy {
                    result,
                    address,
                    reaching,
                    failure,
                });
                if !self.record(ty).drop.is_empty() {
                    self.push(Inst::Share { value: result });
                }
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
            }
            return Ok(result);
        }
        let place = self.place(op.place)?;
        let address = match self.prepared(&place) {
            Some(address) => address,
            None => self.address(&place)?,
        };
        self.operand_at(op.usage, &place, address, place.loan)
    }

    /// The reaching version of a place's slot when the place can carry facts:
    /// payload, field and variant steps only.
    pub(super) fn fact_version(&self, p: &PlaceRef) -> Option<ValueId> {
        let steps_ok = p.projection.iter().all(|step| {
            matches!(
                step,
                ProjectionStep::Payload
                    | ProjectionStep::Field(_)
                    | ProjectionStep::VariantField { .. }
            )
        });
        if p.source.is_some() || !steps_ok {
            return None;
        }
        p.value.or(self.versions[p.slot.index()])
    }

    /// The variant match tag tests proved `p` holds at its slot's current version.
    pub(super) fn proven_variant(&self, p: &PlaceRef) -> Option<u32> {
        let current = self.fact_version(p)?;
        self.facts.iter().find_map(|fact| match fact {
            Fact::Variant {
                slot,
                projection,
                version,
                variants,
            } if *slot == p.slot
                && *version == current
                && *projection == p.projection
                && variants.len() == 1 =>
            {
                Some(variants[0])
            }
            _ => None,
        })
    }

    /// The address prepared at its arm's `BindFields` for an operand read of
    /// `p`, while its slot keeps that version (FACT-3: the arm proved the tag, so
    /// the read needs no second MEM-13 check). A read, even a take, writes nothing.
    pub(super) fn prepared(&self, p: &PlaceRef) -> Option<AddressId> {
        let current = self.fact_version(p)?;
        self.facts.iter().find_map(|fact| match fact {
            Fact::Prepared {
                slot,
                projection,
                version,
                address,
            } if *slot == p.slot
                && *version == current
                && *projection == p.projection
                && self.out.addresses[address.index()].ty == p.ty =>
            {
                Some(*address)
            }
            _ => None,
        })
    }

    /// Follow storage aliases, including projected ones, to a joined root.
    pub(super) fn joined_root(&self, id: tir::PlaceId) -> Option<usize> {
        let mut root = self.body.places[id.0 as usize].root.0 as usize;
        let mut seen = BTreeSet::new();
        while seen.insert(root) {
            if self.joined[root] {
                return Some(root);
            }
            root = self.body.places[self.aliases[root]?.0 as usize].root.0 as usize;
        }
        None
    }

    /// A declaration label for a view phi, obtained from its source loans.
    /// This is not a coverage proof: LIR independently checks every incoming
    /// value, and finish recomputes the actual union from the emitted edges.
    pub(super) fn joined_holding(&self, local: usize, ty: LirTypeId) -> R<Holding> {
        if !self.owns(ty) {
            return Ok(Holding::Plain);
        }
        let (assignments, _, _) = view_assignments(self.body);
        let mut pending = vec![local];
        let mut seen = BTreeSet::new();
        let mut loans = BTreeSet::new();
        while let Some(root) = pending.pop() {
            if !seen.insert(root) {
                continue;
            }
            // A transient slice carries its append consumer's loan on the
            // stable owner, begun at the slice rather than at append entry.
            for (&block, slice) in &self.slices.sources {
                if let tir::Terminator::Call {
                    returns: Some(ret), ..
                } = &self.body.blocks[block.0 as usize].terminator
                    && self.body.places[ret.destination.0 as usize].root.0 as usize == root
                {
                    loans.insert(self.loan(slice.loan)?);
                }
            }
            for source in &assignments[root] {
                let place = match source {
                    Some(
                        ViewSource::Loaned(p)
                        | ViewSource::Nested(p)
                        | ViewSource::Forwarded(p)
                        | ViewSource::Indexed(p, _)
                        | ViewSource::Moved(p),
                    ) => *p,
                    None => return unsupported("owning input to a joined borrow"),
                };
                let source = &self.body.places[place.0 as usize];
                if let Some(loan) = source.loan {
                    loans.insert(self.loan(loan)?);
                } else {
                    // This walk follows view transport, not the construction
                    // of an owning value protected by an established loan.
                    pending.push(source.root.0 as usize);
                }
            }
        }
        loans
            .first()
            .copied()
            .map(Holding::Borrowed)
            .ok_or_else(|| PassError::Unsupported("a joined view without a source loan".into()))
    }

    /// Follow whole-place aliases to the joined SSA view they name.
    pub(super) fn joined_local(&self, id: tir::PlaceId) -> Option<usize> {
        let mut place = &self.body.places[id.0 as usize];
        loop {
            if !place.projection.is_empty() {
                return None;
            }
            let local = place.root.0 as usize;
            if self.joined[local] {
                return Some(local);
            }
            place = &self.body.places[self.aliases[local]?.0 as usize];
        }
    }

    pub(super) fn whole_joined(&self, id: tir::PlaceId) -> Option<R<ValueId>> {
        let l = self.joined_local(id)?;
        Some(match (self.state.views[l], self.views[l]) {
            (Tri::Yes, Some(v)) => Ok(v),
            _ => unsupported(format!(
                "joined local {l} not held on every path at {:?}: {:?}",
                self.statement, self.state.views[l]
            )),
        })
    }

    /// Borrowing a whole joined view keeps its existing SSA representation.
    pub(super) fn joined_view(&self, op: &tir::Operand) -> Option<R<ValueId>> {
        matches!(op.usage, UseKind::Borrow | UseKind::Take)
            .then(|| self.whole_joined(op.place))
            .flatten()
    }

    /// Address an inline SSA view without inventing an owning slot or credit.
    pub(super) fn view_address(&mut self, value: ValueId) -> R<AddressId> {
        let ty = self.value_type(value);
        if matches!(
            self.out.types[ty.index()].repr,
            ValueRepr::Block | ValueRepr::Uncounted
        ) {
            return unsupported("a whole block joined-view copy");
        }
        let address = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Value(value),
            projection: PoolRange::EMPTY,
            ty,
        });
        let failure = self.fail(None)?;
        self.push(Inst::Address { address, failure });
        Ok(address)
    }

    /// Whether the environment moves capture `ordinal` out under a live bit.
    fn movable_capture(&self, ordinal: u32) -> bool {
        self.out.tables.environment().is_some_and(|environment| {
            matches!(&self.layouts.record(environment).kind,
                LayoutKind::Environment { movable } if movable.contains(&ordinal))
        })
    }

    pub(super) fn clear_capture(&mut self, ordinal: u32) -> R<()> {
        let environment = self
            .out
            .tables
            .environment()
            .ok_or_else(|| PassError::Malformed("capture take without live word".into()))?;
        let record = self.layouts.record(environment);
        let LayoutKind::Environment { movable } = &record.kind else {
            return malformed("capture mask layout");
        };
        let bit = movable
            .iter()
            .position(|&i| i == ordinal)
            .ok_or_else(|| PassError::Malformed("capture lacks live bit".into()))?;
        let flag_ordinal = record.fields.last().expect("live word").ordinal;
        let slot = SlotId(
            self.out
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Capture(flag_ordinal))
                .expect("live word slot") as u32,
        );
        let ty = self.out.slots[slot.index()].ty;
        let place = PlaceRef {
            value: None,
            source_place: None,
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let address = self.fixed_address(&place)?;
        let reaching = self.reaching(slot)?;
        let old = self.value(ty, Holding::Plain);
        self.push(Inst::Read {
            result: old,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        let value = self.value(ty, Holding::Plain);
        self.push(Inst::UpdateMask {
            result: value,
            mask: old,
            field: bit as u32,
            initialized: false,
        });
        let version = self.value(ty, Holding::Plain);
        self.push(Inst::Store {
            address,
            value,
            version,
        });
        self.state.write(slot, &[]);
        self.versions[slot.index()] = Some(version);
        Ok(())
    }

    pub(super) fn operand_at(
        &mut self,
        usage: UseKind,
        place: &PlaceRef,
        address: AddressId,
        loan: Option<tir::LoanId>,
    ) -> R<ValueId> {
        if let Some((source, source_type)) = place.source {
            if matches!(usage, UseKind::Copy | UseKind::Take) {
                if let LayoutKind::Block { payload } = self.record(source).kind
                    && matches!(
                        self.layouts.record(payload).kind,
                        LayoutKind::Sequence { .. }
                    )
                {
                    return self.materialize_sequence(address, place.ty, source, source_type);
                }
                if !matches!(self.record(source).kind, LayoutKind::Slice { .. })
                    && !matches!(self.record(source).kind, LayoutKind::Block { payload }
                        if self.layouts.record(payload).kind == LayoutKind::Flat)
                {
                    return unsupported("materializing this ordinary value from a flat encoding");
                }
                let result = self.fresh(source);
                let failure = self.fail(None)?;
                self.push(Inst::FlatCopy {
                    result,
                    address,
                    layout: self.layout_of(place.ty),
                    failure,
                });
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                return Ok(result);
            }
            let loan = match (usage, loan) {
                (UseKind::Borrow, Some(l)) => self.loan(l)?,
                _ => {
                    return unsupported(format!(
                        "a variable-size part of a flat value as a {usage:?} operand"
                    ));
                }
            };
            return self.flat_view(address, loan, source);
        }
        let ty = place.ty;
        let reaching = match place.value {
            Some(value) => value,
            None => self.reaching(place.slot)?,
        };
        // A source copy shares the arena Lists the value holds (MEM-6); a
        // take this lowers as a copy is no source copy.
        let semantic_copy = usage == UseKind::Copy;
        // The caller keeps what it lends: taking from a borrowed parameter
        // (a copyable capability, `take s` of `s: Dir`) copies it.
        let usage = if usage == UseKind::Take && self.borrowed_storage(place.slot) {
            UseKind::Copy
        } else if usage == UseKind::Take && self.bits(ty) && self.mut_storage(place.slot) {
            // The caller's storage keeps its bits: a plain take is a read.
            UseKind::Copy
        } else if usage == UseKind::Take
            && matches!(
                self.out.slots[place.slot.index()].kind,
                SlotKind::Capture(ordinal) if !self.movable_capture(ordinal)
            )
            && self.record(ty).copy != CopyRecipe::NotCopyable
        {
            // A closure body's captures are its caller's environment, lent
            // for the call as a borrowed parameter is: a capture without a
            // live bit holds only bits, so taking it (a `once` closure's
            // `take x`) copies it.
            UseKind::Copy
        } else if usage == UseKind::Take
            && place.crosses
            && self.record(ty).copy != CopyRecipe::NotCopyable
        {
            // A copyable part taken out of a temporary's block (an element of
            // a call's list result, say) or out of its inline storage at a
            // computed position (an element of a const) is copied, and the
            // temporary is dropped whole: a take would detach a shared block
            // (OWN-4), or leave a hole at a position the drop cannot name,
            // that its drop would have to skip. Nothing tells the two apart,
            // as copyable content has no finalizer.
            UseKind::Copy
        } else {
            usage
        };
        match usage {
            UseKind::Copy if self.bits(ty) => {
                let result = self.value(ty, Holding::Plain);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                });
                Ok(result)
            }
            UseKind::Copy => {
                let result = self.fresh(ty);
                let failure = self.fail(None)?;
                self.push(Inst::Copy {
                    result,
                    address,
                    reaching,
                    failure,
                });
                let row = &self.out.types[ty.index()];
                if semantic_copy
                    && (row.repr == ValueRepr::Block
                        || !self.layouts.record(row.layout).drop.is_empty())
                {
                    self.push(Inst::Share { value: result });
                }
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                Ok(result)
            }
            UseKind::Take => {
                let result = self.fresh(ty);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Take,
                });
                if let SlotKind::Capture(ordinal) = self.out.slots[place.slot.index()].kind {
                    if !place.steps.is_empty() {
                        return unsupported("partial capture take");
                    }
                    self.clear_capture(ordinal)?;
                }
                self.state.take(place.slot, &place.steps);
                if place.steps.is_empty() {
                    self.versions[place.slot.index()] = None;
                }
                if self.holds(result) {
                    self.failure.pending.push(result);
                }
                Ok(result)
            }
            // A value read through a mutable loan (`x += y` reads `mut x`):
            // a view inside that loan, like a shared one.
            UseKind::Borrow | UseKind::Mut => {
                let holding = if self.owns(ty) {
                    match loan {
                        Some(l) => Holding::Borrowed(self.loan(l)?),
                        None => match self.out.values[reaching.index()].holding {
                            Holding::Borrowed(loan) if place.value.is_some() => {
                                Holding::Borrowed(loan)
                            }
                            _ => {
                                return unsupported(format!(
                                    "a borrowed read of counted storage outside any loan: slot {:?}, steps {:?}, reaching {:?} ({:?}), at {:?}",
                                    place.slot,
                                    place.steps,
                                    reaching,
                                    self.out.values[reaching.index()].holding,
                                    self.statement,
                                ));
                            }
                        },
                    }
                } else {
                    Holding::Plain
                };
                let result = self.value(ty, holding);
                self.push(Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                });
                Ok(result)
            }
        }
    }

    /// A String or Bytes part of a flat value viewed at its source type under
    /// `loan` (LIR schema revision 4); other variable parts have no view.
    pub(super) fn flat_view(
        &mut self,
        address: AddressId,
        loan: LoanId,
        source: LirTypeId,
    ) -> R<ValueId> {
        if !matches!(self.record(source).kind, LayoutKind::Slice { .. }) {
            return unsupported(
                "a variable-size part of a flat value other than String or Bytes at its source type",
            );
        }
        let result = self.value(source, Holding::Borrowed(loan));
        self.push(Inst::FlatView { result, address });
        Ok(result)
    }

    /// A new loan of this pass, `active` in the current state. A later pass
    /// starts from states joined over the previous pass's edges, which
    /// already name the loans this pass allocates after a loop head, so the
    /// state is set at the loan's id, never appended.
    pub(super) fn new_loan(&mut self, info: LoanInfo, active: Tri) -> LoanId {
        let loan = LoanId(self.out.tables.loans.len() as u32);
        self.out.tables.loans.push(info);
        if self.state.loans.len() <= loan.index() {
            self.state.loans.resize(loan.index() + 1, Tri::No);
        }
        self.state.loans[loan.index()] = active;
        loan
    }

    /// A loan the TIR does not name, begun here on a prepared address.
    pub(super) fn synthetic_loan(&mut self, address: AddressId) -> LoanId {
        let info = LoanInfo {
            address,
            mutable: false,
        };
        let loan = self.new_loan(info, Tri::Yes);
        self.push(Inst::BeginLoan {
            loan,
            address,
            mutable: false,
            if_inactive: false,
        });
        loan
    }

    /// A slot the caller lends: a borrowed parameter in its own storage.
    pub(super) fn borrowed_storage(&self, slot: SlotId) -> bool {
        let SlotKind::Parameter(k) = self.out.slots[slot.index()].kind else {
            return false;
        };
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Borrow)
    }

    /// A `mut` parameter's storage, which the caller reads back.
    pub(super) fn mut_storage(&self, slot: SlotId) -> bool {
        let SlotKind::Parameter(k) = self.out.slots[slot.index()].kind else {
            return false;
        };
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Mut)
    }

    /// A value consumed by an operation leaves the pending set.
    pub(super) fn consumed(&mut self, v: ValueId) {
        self.failure.pending.retain(|&p| p != v);
    }
}

// Keep the selection at the place-resolution seam: the iterator may be a
// HashMap's order, which is not a source program property. For equal prefix
// and start, the smallest complete projection is the bounds operation, rather
// than its enclosing compound assignment (diagnostics.md, Primary sites).
// Finish a full-coordinate tie with original source order; session Anchor
// numbers must never choose between source owners. Return the original Span.
fn prefix_origin_site(
    candidates: impl Iterator<Item = (usize, hirsel_span::Span)>,
    source: impl Fn(hirsel_span::Span) -> u32,
) -> Option<hirsel_span::Span> {
    candidates
        .max_by_key(|(len, site)| {
            (
                *len,
                site.start,
                std::cmp::Reverse(site.end),
                std::cmp::Reverse(source(*site)),
            )
        })
        .map(|(_, site)| site)
}

#[cfg(test)]
mod tests {
    use super::prefix_origin_site;
    use hirsel_span::{Anchor, Span};

    fn span(anchor: u32, start: u32, end: u32) -> Span {
        Span {
            anchor: Anchor(anchor),
            start,
            end,
        }
    }

    #[test]
    fn equal_prefix_and_start_preserve_the_smallest_complete_origin_in_any_order() {
        let narrow = span(7, 1762, 1770);
        let wide = span(7, 1762, 1775);
        for candidates in [vec![(1, narrow), (1, wide)], vec![(1, wide), (1, narrow)]] {
            assert_eq!(
                prefix_origin_site(candidates.into_iter(), |_| 0),
                Some(narrow)
            );
        }
    }

    #[test]
    fn longest_projection_then_latest_origin_remain_the_primary_order() {
        let expected = span(7, 18, 23);
        let candidates = [(1, span(7, 40, 42)), (2, span(7, 10, 12)), (2, expected)];
        for reverse in [false, true] {
            let mut candidates = candidates.to_vec();
            if reverse {
                candidates.reverse();
            }
            assert_eq!(
                prefix_origin_site(candidates.into_iter(), |_| 0),
                Some(expected)
            );
        }
    }

    #[test]
    fn source_owner_ties_use_original_source_order_after_session_relocation() {
        // Source owner0 stays owner0 after session-local Anchor numbers move.
        // Returned spans retain that owner's Anchor rather than being rebuilt
        // with the current use's or another source owner's Anchor.
        for anchors in [[2, 91], [701, 3]] {
            let original = span(anchors[0], 18, 23);
            let other = span(anchors[1], 18, 23);
            for candidates in [
                vec![(2, original), (2, other)],
                vec![(2, other), (2, original)],
            ] {
                assert_eq!(
                    prefix_origin_site(candidates.into_iter(), |span| {
                        if span.anchor == original.anchor { 0 } else { 1 }
                    }),
                    Some(original)
                );
            }
        }
    }
}
