use super::*;
use crate::PassError;

impl Importer<'_> {
    fn static_layout_key(&mut self, layout: LayoutId) -> hirsel_lir::layout_types::LayoutKey {
        *self
            .static_layout_keys
            .entry(layout)
            .or_insert_with(|| hirsel_lir::LayoutKeys::rooted(self.layouts.table(), layout))
    }

    pub(super) fn destination_class(&self, d: &hirsel_types::Destination) -> R<DestinationClass> {
        use hirsel_types::Destination as D;
        Ok(match d {
            D::Heap => DestinationClass::Heap,
            D::Inferred => DestinationClass::Inferred,
            D::Region(_) => DestinationClass::Region,
            D::Caller(parameters) => {
                DestinationClass::Caller(parameters.iter().map(|p| p.parameter().0).collect())
            }
            D::Arena(origin) => DestinationClass::Arena(self.arena_slot(origin)?),
        })
    }

    /// The slot holding the arena token a relation names: a whole parameter
    /// or binding of this body.
    pub(super) fn arena_slot(&self, origin: &hirsel_types::Origin) -> R<SlotId> {
        use hirsel_types::{OriginRoot, ParameterRef};
        if !origin.fields.is_empty() {
            return unsupported("an arena destination naming a field path");
        }
        let body = self.body;
        let parameter = match &origin.root {
            OriginRoot::Parameter(ParameterRef::Body { scope, parameter })
                if *scope == body.parameter_scope =>
            {
                body.parameters.get(parameter.0 as usize).copied()
            }
            _ => None,
        };
        let root = parameter.or_else(|| {
            body.places
                .iter()
                .find(|p| {
                    p.projection.is_empty()
                        && p.binding
                            .as_ref()
                            .is_some_and(|b| b.root == origin.root && b.fields.is_empty())
                })
                .map(|p| p.root)
        });
        match root {
            Some(root) => self.slot_for(root.0 as usize),
            None => unsupported("an arena destination naming storage outside this body"),
        }
    }

    pub(super) fn whole(&self, slot: SlotId) -> PlaceRef {
        PlaceRef {
            value: None,
            source_place: None,
            slot,
            steps: Vec::new(),
            projection: Vec::new(),
            ty: self.out.slots[slot.index()].ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        }
    }

    /// Store `value` at a prepared address; the root slot gets a new version.
    /// Plain content still present (a plain `take` of `mut` storage kept its
    /// bits) is overwritten through a scratch displaced slot.
    pub(super) fn store(&mut self, place: &PlaceRef, address: AddressId, value: ValueId) -> R<()> {
        let ty = self.out.slots[place.slot.index()].ty;
        let st = &self.state.slots[place.slot.index()];
        let present = st.init != Tri::No
            && !st.moved.iter().any(|(q, t)| {
                *t == Tri::Yes && q.len() <= place.steps.len() && overlap(q, &place.steps)
            });
        if present && self.bits(place.ty) && self.mut_storage(place.slot) {
            let scratch = self.new_slot(
                place.ty,
                SlotKind::Displaced,
                self.out.slots[place.slot.index()].scope,
            );
            self.push(Inst::StorageLive { slot: scratch });
            self.state.live(scratch);
            let version = self.value(ty, Holding::Plain);
            let scratch_ty = self.out.slots[scratch.index()].ty;
            let displaced_version = self.value(scratch_ty, Holding::Plain);
            let failure = self.fail(None)?;
            self.push(Inst::Replace {
                address,
                value,
                displaced: scratch,
                version,
                displaced_version,
                if_live: st_maybe(&self.state.slots[place.slot.index()]),
                failure,
            });
            self.consumed(value);
            self.state.write(place.slot, &place.steps);
            self.versions[place.slot.index()] = Some(version);
            self.push(Inst::StorageDead {
                slot: scratch,
                if_live: false,
            });
            self.state.dead(scratch);
            return Ok(());
        }
        let version = self.value(ty, Holding::Plain);
        self.push(Inst::Store {
            address,
            value,
            version,
        });
        self.consumed(value);
        self.state.write(place.slot, &place.steps);
        self.versions[place.slot.index()] = Some(version);
        Ok(())
    }

    /// A per-statement anchor for drops TIR states as instructions.
    pub(super) fn statement_action(&self) -> u32 {
        (self.statement.0 << 16) | (self.statement.1 & 0xffff)
    }

    /// Whether an instruction only maintains a view temporary's own storage,
    /// which does not exist: its reads go to the viewed place.
    /// Maintain a joined view temporary, which has no storage: an assignment
    /// holds the assigned view as its SSA value, a storage end or drop lets it
    /// go, and an origin binding of it names nothing. Whether `kind` was one.
    pub(super) fn on_joined(&mut self, kind: &tir::InstructionKind) -> R<bool> {
        use tir::InstructionKind as K;
        let joined = |p: &tir::PlaceId| {
            let place = &self.body.places[p.0 as usize];
            let l = place.root.0 as usize;
            (self.joined[l] && place.projection.is_empty()).then_some(l)
        };
        match kind {
            K::StorageLive(local) => Ok(self.joined[local.0 as usize]),
            K::StorageDead { place, .. } | K::Drop { place, .. } => match joined(place) {
                Some(l) => {
                    self.state.views[l] = Tri::No;
                    self.views[l] = None;
                    Ok(true)
                }
                None => Ok(false),
            },
            K::Assign { destination, value } => {
                let Some(l) = joined(destination) else {
                    return Ok(false);
                };
                let tir::Rvalue::Use(o) = &self.body.rvalues[value.0 as usize] else {
                    return malformed("a joined view assigned other than a view");
                };
                let v = self.operand(o)?;
                if self.value_type(v) != self.out.slots[self.slot_for(l)?.index()].ty {
                    return unsupported("a joined view of a place of another type");
                }
                self.state.views[l] = Tri::Yes;
                self.views[l] = Some(v);
                Ok(true)
            }
            K::BindOrigin { source, place, .. } => match (joined(source), joined(place)) {
                (_, Some(_)) => Ok(true),
                (Some(_), None) if self.joined_local(*place).is_some() => Ok(true),
                (Some(_), None) => unsupported("an origin bound from a joined view"),
                (None, None) => Ok(false),
            },
            _ => Ok(false),
        }
    }

    pub(super) fn on_view(&mut self, kind: &tir::InstructionKind) -> R<bool> {
        use tir::InstructionKind as K;
        let view = |p: &tir::PlaceId| {
            let place = &self.body.places[p.0 as usize];
            place.projection.is_empty() && self.aliases[place.root.0 as usize].is_some()
        };
        Ok(match kind {
            K::StorageLive(local) => self.aliases[local.0 as usize].is_some(),
            K::StorageDead { place, .. } | K::Drop { place, .. } => view(place),
            K::Assign { destination, value } if view(destination) => {
                let tir::Rvalue::Use(o) = &self.body.rvalues[value.0 as usize] else {
                    return malformed("a view assigned other than a borrowed use");
                };
                // Keep computed-parent checks at the binding, even if the
                // view is unused or a later child projection fails first.
                if self.body.places[o.place.0 as usize]
                    .projection
                    .iter()
                    .any(|p| {
                        matches!(
                            p,
                            tir::Projection::Index(_) | tir::Projection::VariantField { .. }
                        )
                    })
                {
                    let p = self.place(o.place)?;
                    self.address(&p)?;
                }
                if self
                    .encoding_views
                    .contains(&(self.body.places[destination.0 as usize].root.0 as usize))
                {
                    // Eliding the physical credit must still preserve MEM-6's
                    // permanent arena-sharing event at the original Copy site.
                    let p = self.place(o.place)?;
                    let address = self.address(&p)?;
                    let loan = self.synthetic_loan(address);
                    let value = self.value(p.ty, Holding::Borrowed(loan));
                    let reaching = self.reaching(p.slot)?;
                    self.push(Inst::Read {
                        result: value,
                        address,
                        reaching,
                        mode: ReadMode::Borrow,
                    });
                    self.push(Inst::Share { value });
                    self.push(Inst::EndLoan {
                        loan,
                        if_active: false,
                    });
                    self.state.loans[loan.index()] = Tri::No;
                }
                true
            }
            _ => false,
        })
    }

    pub(super) fn instruction(&mut self, kind: &tir::InstructionKind) -> R<()> {
        use tir::InstructionKind as K;
        if let K::Assign { destination, value } = kind {
            let root = self.body.places[destination.0 as usize].root;
            if let Some(&(list, field)) = self.fields.opens.get(&root) {
                return self.field_open(list, field);
            }
            if let Some(&list) = self.builders.heads.get(&root) {
                let open = self.fields.closes.get(&root).copied();
                return self.flat_append(list, *value, open);
            }
        }
        let elided = match kind {
            K::StorageLive(local) => self.local_slot[local.0 as usize].is_none(),
            K::StorageDead { place, .. } | K::Drop { place, .. } | K::EndOrigin(place) => {
                self.elided_place(*place)
            }
            K::BeginLoan { id, place, .. } => self.elided_place(*place) || self.elided_loan(*id),
            K::Assign { destination, .. } | K::Snapshot { destination, .. } => {
                self.elided_place(*destination)
            }
            K::BindOrigin { source, place, .. } => {
                self.elided_place(*source) || self.elided_place(*place)
            }
            K::EndLoan { loan, .. } => self.elided_loan(*loan),
            _ => false,
        };
        if elided {
            return Ok(());
        }
        match kind {
            K::StorageLive(local) if self.observations[local.0 as usize] => return Ok(()),
            K::Assign { destination, value }
                if self.observations[self.body.places[destination.0 as usize].root.0 as usize] =>
            {
                let tir::Rvalue::Use(o) = &self.body.rvalues[value.0 as usize] else {
                    return malformed("discarded callable read is not a borrowed use");
                };
                self.operand(o)?;
                return Ok(());
            }
            _ => {}
        }
        if self.on_view(kind)? {
            return Ok(());
        }
        if self.on_joined(kind)? {
            return Ok(());
        }
        match kind {
            K::StorageLive(local) => {
                let slot = self.slot_for(local.0 as usize)?;
                self.push(Inst::StorageLive { slot });
                self.state.live(slot);
                self.versions[slot.index()] = None;
            }
            K::StorageDead { place, .. } => {
                let p = self.place(*place)?;
                if !p.steps.is_empty() {
                    return malformed("storage end of a part");
                }
                let live = self.state.slots[p.slot.index()].live;
                if live != Tri::No {
                    self.push(Inst::StorageDead {
                        slot: p.slot,
                        if_live: live == Tri::Maybe,
                    });
                    self.state.dead(p.slot);
                    self.versions[p.slot.index()] = None;
                }
            }
            K::EndScope(scope) => {
                for (local, storage) in self.body.locals.iter().enumerate() {
                    if !self.joined[local] {
                        continue;
                    }
                    let mut current = Some(storage.scope);
                    while let Some(at) = current {
                        if at == *scope {
                            self.state.views[local] = Tri::No;
                            self.views[local] = None;
                            break;
                        }
                        current = self.body.scopes[at.0 as usize].parent;
                    }
                }
                self.push(Inst::EndScope {
                    scope: ScopeId(scope.0),
                });
            }
            K::Assign { destination, value } => {
                let rvalue = &self.body.rvalues[value.0 as usize];
                let p = self.place(*destination)?;
                let v = self.rvalue(rvalue, p.ty)?;
                let v = self.weaken(v, p.ty);
                let address = self.address(&p)?;
                self.store(&p, address, v)?;
            }
            K::Snapshot { destination, value } => {
                let p = self.place(*destination)?;
                let v = self.operand(value)?;
                let v = self.weaken(v, p.ty);
                let address = self.address(&p)?;
                self.store(&p, address, v)?;
            }
            K::Replace {
                destination,
                value,
                displaced,
                ..
            } => self.replace(*destination, value, *displaced)?,
            K::BeginLoan {
                id, place, mutable, ..
            } => {
                let loan = self.loan(*id)?;
                let st = self.state.loans[loan.index()];
                if st != Tri::Yes {
                    let mut address = self.out.tables.loans[loan.index()].address;
                    if address == UNPREPARED {
                        let p = self.place(*place)?;
                        address = self.address(&p)?;
                        self.out.tables.loans[loan.index()].address = address;
                    }
                    self.push(Inst::BeginLoan {
                        loan,
                        address,
                        mutable: *mutable,
                        if_inactive: st == Tri::Maybe,
                    });
                    self.state.loans[loan.index()] = Tri::Yes;
                }
            }
            K::EndLoan { loan, .. } => {
                let loan = self.loan(*loan)?;
                if let Step2::Inline(inst) = self.action(&Action::EndLoan { loan })? {
                    self.push(inst);
                }
            }
            K::Drop { place, .. } => self.drop_place(*place)?,
            K::RunCleanup(cleanup) => {
                let actions = self.chain_actions(*cleanup)?;
                let tail = self.failure_actions()?;
                let before = self.current;
                self.run_normal(&actions, &tail)?;
                if self.current != before {
                    self.split(BlockKind::Body);
                }
            }
            K::Check { condition, rule } => {
                let c = self.operand(condition)?;
                // PS-13: a licensed check is left out; its condition is still read.
                if !self.discharged(Form::Check(rule.to_string())) {
                    let failure = self.fail(Some(rule.to_string()))?;
                    self.push(Inst::Check {
                        condition: c,
                        failure,
                    });
                }
            }
            K::BindOrigin {
                source,
                place,
                kind,
            } => {
                for id in [*source, *place] {
                    self.origin_sites.entry(id).or_insert(self.span);
                }
                let s = self.place(*source)?;
                let source = self.origin_address(&s)?;
                let p = self.place(*place)?;
                let place = self.origin_address(&p)?;
                self.push(Inst::BindOrigin {
                    source,
                    place,
                    kind: match kind {
                        tir::OriginBindingKind::Alias => OriginKind::Alias,
                        tir::OriginBindingKind::Fresh => OriginKind::Fresh,
                    },
                });
            }
            K::BindFields { source, fields } => {
                let s = self.place(*source)?;
                let proven = self.proven_variant(&s);
                let source = self.origin_address(&s)?;
                let mut addresses = Vec::new();
                for f in fields {
                    let p = self.place(*f)?;
                    let field_of_proven = p.slot == s.slot
                        && p.projection.len() == s.projection.len() + 1
                        && p.projection.starts_with(&s.projection)
                        && matches!(
                            p.projection.last(),
                            Some(ProjectionStep::VariantField { variant, .. })
                                if Some(*variant) == proven
                        );
                    let version = self.fact_version(&p);
                    match version {
                        // The tag tests proved the variant: this check cannot
                        // fail, and the arm's reads of the field reuse it.
                        Some(version) if field_of_proven => {
                            let address = self.address(&p)?;
                            self.facts.push(Fact::Prepared {
                                slot: p.slot,
                                projection: p.projection.clone(),
                                version,
                                address,
                            });
                            addresses.push(address);
                        }
                        _ => addresses.push(self.origin_address(&p)?),
                    }
                }
                let fields = PoolRange::push(&mut self.out.pools.addresses, addresses);
                self.push(Inst::BindFields { source, fields });
            }
            K::Defuse { place } => {
                let p = self.place(*place)?;
                let address = self.address(&p)?;
                self.push(Inst::Defuse { address });
            }
            K::EndOrigin(place) => {
                let p = self.place(*place)?;
                let place = self.origin_address(&p)?;
                self.push(Inst::EndOrigin { place });
            }
            // Native code runs no compile-time evaluation (CT-4).
            K::Charge(_) => {}
        }
        Ok(())
    }

    /// A drop of a fixed part: the same state rule as a cleanup drop.
    pub(super) fn part_drop(&mut self, p: &PlaceRef) -> R<Step2> {
        let span = self.span;
        let anchor = (INSTRUCTION_DROP, self.statement_action());
        // Reuse the cleanup path through a synthetic TIR-free action.
        let st = &self.state.slots[p.slot.index()];
        let covered = st
            .moved
            .iter()
            .any(|(q, t)| q.len() <= p.steps.len() && overlap(q, &p.steps) && *t == Tri::Yes);
        if st.live == Tri::No || st.init == Tri::No || covered {
            return Ok(Step2::Skip);
        }
        let partial = st
            .moved
            .iter()
            .any(|(q, t)| overlap(q, &p.steps) && t.may());
        let sure = st.live == Tri::Yes && st.init == Tri::Yes && !partial;
        let address = self.fixed_address(p)?;
        let anchor = self.cleanup_anchor(anchor.0, anchor.1, span);
        Ok(Step2::Drop(DropStep {
            address,
            if_live: !sure,
            anchor,
            slot: p.slot,
            steps: p.steps.clone(),
        }))
    }

    /// `Replace`: the new value is evaluated, the destination prepared, then
    /// old content moves to the displaced slot and new content is installed
    /// in one step; the displaced value is dropped by the TIR's
    /// following `Drop`.
    pub(super) fn replace(
        &mut self,
        destination: tir::PlaceId,
        value: &tir::Operand,
        displaced: tir::PlaceId,
    ) -> R<()> {
        let v = self.operand(value)?;
        let p = self.place(destination)?;
        let v = self.weaken(v, p.ty);
        let address = self.address(&p)?;
        let d = self.place(displaced)?;
        if !d.steps.is_empty() {
            return malformed("a displaced part");
        }
        let st = &self.state.slots[p.slot.index()];
        let moved = st
            .moved
            .iter()
            .filter(|(q, _)| overlap(q, &p.steps))
            .map(|(_, t)| *t)
            .fold(Tri::No, Tri::join);
        let present = match (st.init, moved) {
            (Tri::No, _) | (_, Tri::Yes) => Tri::No,
            (Tri::Yes, Tri::No) => Tri::Yes,
            _ => Tri::Maybe,
        };
        if present == Tri::No {
            // Nothing to displace: a plain store; the displaced slot stays empty.
            self.store(&p, address, v)?;
            return Ok(());
        }
        self.consumed(v);
        let failure = self.fail(None)?;
        let ty = self.out.slots[p.slot.index()].ty;
        let version = self.value(ty, Holding::Plain);
        let displaced_ty = self.out.slots[d.slot.index()].ty;
        let displaced_version = self.value(displaced_ty, Holding::Plain);
        self.push(Inst::Replace {
            address,
            value: v,
            displaced: d.slot,
            version,
            displaced_version,
            if_live: present == Tri::Maybe,
            failure,
        });
        self.state.write(p.slot, &p.steps);
        self.versions[p.slot.index()] = Some(version);
        let ds = &mut self.state.slots[d.slot.index()];
        ds.init = if p.steps.is_empty() {
            present
        } else {
            Tri::Yes
        };
        ds.moved.clear();
        self.versions[d.slot.index()] = Some(displaced_version);
        Ok(())
    }

    pub(super) fn rvalue(&mut self, rvalue: &tir::Rvalue, ty: LirTypeId) -> R<ValueId> {
        use tir::Rvalue as V;
        Ok(match rvalue {
            V::Constant(c) => {
                let value = match c {
                    tir::Constant::Unit => ConstantValue::Unit,
                    tir::Constant::Bool(b) => ConstantValue::Bool(*b),
                    tir::Constant::Integer(n) => ConstantValue::Integer(*n),
                    tir::Constant::F32(x) => ConstantValue::F32(*x),
                    tir::Constant::F64(x) => ConstantValue::F64(*x),
                    tir::Constant::String(s) => ConstantValue::String(s.clone()),
                    tir::Constant::Bytes(b) => ConstantValue::Bytes(b.clone()),
                };
                // A text literal is a static slice: no credit (LIR schema rev 2).
                let literal = matches!(value, ConstantValue::String(_) | ConstantValue::Bytes(_));
                let constant = self.constant(ty, value);
                let result = if literal {
                    self.value(ty, Holding::Plain)
                } else {
                    self.fresh(ty)
                };
                self.push(Inst::Const { result, constant });
                result
            }
            V::ReadConstant {
                definition,
                arguments,
            } => {
                let values = arguments
                    .iter()
                    .map(|size| match size {
                        hirsel_types::Size::Constant(n) => Some(*n),
                        hirsel_types::Size::Parameter(p) => self
                            .instances
                            .view(self.id)
                            .substitution
                            .sizes
                            .iter()
                            .find(|(parameter, _)| parameter == p)
                            .map(|(_, n)| *n),
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(values) = values else {
                    return malformed("constant size without an actual");
                };
                let instance = hirsel_tir::constant_instance_path(definition, &values);
                let definition = &instance;
                let symbol = self.symbol(SymbolKind::Constant, definition, None);
                let result = self.fresh(ty);
                self.push(Inst::ReadStatic { result, symbol });
                result
            }
            V::FunctionReference {
                definition,
                arguments,
            } => {
                let actuals = crate::instance::callee_actuals(
                    self.types,
                    &self.program.memo,
                    self.program.binders.get(definition).copied(),
                    arguments,
                    self.actuals,
                    false,
                    definition,
                )?;
                let (symbol, _) = if let Some(request) =
                    crate::library::keyed::request(self.program, definition, actuals.arguments)?
                {
                    let direct = self.require(request)?;
                    (
                        self.symbol(
                            SymbolKind::Function,
                            &direct.symbol.definition,
                            direct.symbol.instance,
                        ),
                        direct.abi,
                    )
                } else {
                    self.direct(definition, arguments)?
                };
                let result = self.fresh(ty);
                self.push(Inst::FunctionRef { result, symbol });
                result
            }
            V::Size(parameter) => {
                let n = self
                    .instances
                    .view(self.id)
                    .substitution
                    .sizes
                    .iter()
                    .find(|(p, _)| p == parameter)
                    .map(|(_, n)| *n);
                let Some(n) = n else {
                    return malformed("a size parameter without an actual");
                };
                let constant = self.constant(ty, ConstantValue::Integer(u128::from(n)));
                let result = self.fresh(ty);
                self.push(Inst::Const { result, constant });
                result
            }
            V::Use(op) => self.operand(op)?,
            V::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } => self.unique(value, ty)?,
            V::Unary { op, value } => {
                let op = match op {
                    hirsel_syntax::UnaryOp::Neg => UnaryOp::Neg,
                    hirsel_syntax::UnaryOp::Not => UnaryOp::Not,
                    hirsel_syntax::UnaryOp::BitNot => UnaryOp::BitNot,
                    other => return malformed(format!("ownership operator {other:?} as a value")),
                };
                let operand = self.operand(value)?;
                let result = self.fresh(ty);
                if op == UnaryOp::Neg && (self.unsigned(ty) || self.discharged(Form::Negation)) {
                    // Negating an unsigned value is `0 - x`: SEM-1 unless x is 0.
                    let constant = self.constant(ty, ConstantValue::Integer(0));
                    let zero = self.fresh(ty);
                    self.push(Inst::Const {
                        result: zero,
                        constant,
                    });
                    if self.discharged(Form::Integer) || self.discharged(Form::Negation) {
                        self.push(Inst::IntArith {
                            result,
                            flag: None,
                            op: BinaryOp::Sub,
                            mode: hirsel_lir::ir::ArithMode::Wrapping,
                            left: zero,
                            right: operand,
                        });
                    } else {
                        let failure = self.fail(Some("SEM-1".to_string()))?;
                        self.push(Inst::Binary {
                            result,
                            op: BinaryOp::Sub,
                            left: zero,
                            right: operand,
                            failure,
                        });
                    }
                    return Ok(result);
                }
                let failure = self.fail((op == UnaryOp::Neg).then(|| "SEM-1".to_string()))?;
                self.push(Inst::Unary {
                    result,
                    op,
                    operand,
                    failure,
                });
                result
            }
            V::Binary { op, left, right } => {
                let Some(lop) = lir_binary(*op) else {
                    return unsupported(format!("binary {op:?} as one operation"));
                };
                let side = match self.joined_view(left) {
                    Some(view) => self.value_type(view?),
                    None => self.place(left.place)?.ty,
                };
                if !matches!(self.out.types[side.index()].repr, ValueRepr::Scalar(_)) {
                    return self.compare(lop, left, right, ty);
                }
                let l = self.operand(left)?;
                let r = self.operand(right)?;
                let result = self.fresh(ty);
                let arithmetic = matches!(
                    lop,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Rem
                        | BinaryOp::Shl
                        | BinaryOp::Shr
                );
                if arithmetic && self.discharged(Form::Integer) {
                    // Every failure condition of this integer operation is proved absent.
                    self.push(Inst::IntArith {
                        result,
                        flag: None,
                        op: lop,
                        mode: hirsel_lir::ir::ArithMode::Wrapping,
                        left: l,
                        right: r,
                    });
                    return Ok(result);
                }
                let failure = self.fail(arithmetic.then(|| "SEM-1".to_string()))?;
                self.push(Inst::Binary {
                    result,
                    op: lop,
                    left: l,
                    right: r,
                    failure,
                });
                result
            }
            V::Construct {
                constructor,
                fields,
                destination,
            } => {
                if let tir::Constructor::SortedMap { by } = constructor {
                    let by = self.operand(by)?;
                    self.consumed(by);
                    return self.keyed_construct(
                        hirsel_types::BuiltinType::SortedMap,
                        &[],
                        self.destination_class(destination)?,
                        ty,
                    );
                }
                if matches!(constructor, tir::Constructor::Map | tir::Constructor::Set) {
                    return self.keyed_construct(
                        if matches!(constructor, tir::Constructor::Map) {
                            hirsel_types::BuiltinType::Map
                        } else {
                            hirsel_types::BuiltinType::Set
                        },
                        fields,
                        self.destination_class(destination)?,
                        ty,
                    );
                }
                if let Some(value) = self.nullary_box(constructor, fields, destination, ty)? {
                    return Ok(value);
                }
                let layout = match self.record(ty).kind {
                    LayoutKind::Block { payload } | LayoutKind::Box { payload }
                        if !matches!(self.out.types[ty.index()].repr, ValueRepr::Aggregate) =>
                    {
                        payload
                    }
                    _ => self.layout_of(ty),
                };
                // A flat record encodes each variable field's source value,
                // reading it only (LIR schema revision 4).
                let record = self.layouts.record(layout);
                let encoded: Vec<u32> = if record.kind == LayoutKind::Flat {
                    record
                        .fields
                        .iter()
                        .filter(|d| !flat_fixed(self.layouts, d.layout))
                        .map(|d| d.ordinal)
                        .collect()
                } else {
                    Vec::new()
                };
                let (values, loans, taken) = self.construct_fields(&encoded, fields)?;
                if !encoded.is_empty() {
                    let constructor = match constructor {
                        tir::Constructor::Record(path) => {
                            ConstructorKind::Record(self.symbol(SymbolKind::Type, path, None))
                        }
                        tir::Constructor::Tuple => ConstructorKind::Tuple,
                        other => return malformed(format!("a flat {other:?} construction")),
                    };
                    let destination = self.destination_class(destination)?;
                    // The construction is pending while the encoded sources
                    // are released after it.
                    let mut result = None;
                    self.reading(loans, taken, |this| {
                        let v = this.construct(constructor, layout, values, destination, ty)?;
                        if this.holds(v) {
                            this.failure.pending.push(v);
                        }
                        result = Some(v);
                        Ok(())
                    })?;
                    let v = result.expect("constructed");
                    self.failure.pending.retain(|&p| p != v);
                    return Ok(v);
                }
                let destination = self.destination_class(destination)?;
                // A distinct type over another nominal (`type R = A`) is laid
                // out as a one-field wrapper of it, while TIR builds it from the
                // inner type's own fields: build the inner value, then wrap it.
                if let tir::Constructor::Record(path) = constructor
                    && let Some(inner) = self.wrapped_nominal(path)
                    && let [field] = self.layouts.record(layout).fields.as_slice()
                    && !(values.len() == 1
                        && values[0].field == field.ordinal
                        && self.layout_of(self.value_type(values[0].value)) == field.layout)
                {
                    let inner_layout = field.layout;
                    let inner_ty = self.intern(inner_layout);
                    let symbol = self.symbol(SymbolKind::Type, &inner, None);
                    let v = self.construct(
                        ConstructorKind::Record(symbol),
                        inner_layout,
                        values,
                        destination.clone(),
                        inner_ty,
                    )?;
                    if self.holds(v) {
                        self.failure.pending.push(v);
                    }
                    let symbol = self.symbol(SymbolKind::Type, path, None);
                    return self.construct(
                        ConstructorKind::Record(symbol),
                        layout,
                        vec![FieldValue { field: 0, value: v }],
                        destination,
                        ty,
                    );
                }
                let constructor = match constructor {
                    tir::Constructor::Tuple => ConstructorKind::Tuple,
                    tir::Constructor::Record(path) => {
                        ConstructorKind::Record(self.symbol(SymbolKind::Type, path, None))
                    }
                    tir::Constructor::Variant {
                        definition,
                        variant,
                    } => ConstructorKind::Variant {
                        definition: self.symbol(SymbolKind::Type, definition, None),
                        variant: *variant,
                    },
                    tir::Constructor::Sequence => ConstructorKind::Sequence,
                    tir::Constructor::Map
                    | tir::Constructor::Set
                    | tir::Constructor::SortedMap { .. } => {
                        unreachable!("handled above")
                    }
                    tir::Constructor::Box => ConstructorKind::Box,
                };
                let site = self.out.tables.constructs.len();
                let sequence = constructor == ConstructorKind::Sequence;
                let value = self.construct(constructor, layout, values, destination, ty)?;
                // A List or Array literal's block, of rows, of `@soa` columns or
                // of `Packed` values, is tracked in an arena (MEM-6); a static
                // empty sequence is no construction.
                if sequence
                    && self.out.tables.constructs.len() > site
                    && matches!(
                        self.layouts.record(layout).kind,
                        LayoutKind::Sequence { .. }
                            | LayoutKind::SoaColumns { .. }
                            | LayoutKind::Sequential { .. }
                    )
                {
                    self.out.tables.constructs[site].tracked = true;
                }
                value
            }
            V::Closure {
                context,
                body,
                captures,
                destination,
            } => {
                let data_environment = self
                    .instances
                    .data_environments
                    .get(&(self.id, *body))
                    .copied();
                let mut values = Vec::new();
                for c in captures {
                    if self.elided_place(c.place) {
                        continue;
                    }
                    let usage = match c.usage {
                        UseKind::Borrow => hirsel_lir::ir::CaptureUse::Borrow,
                        UseKind::Take => hirsel_lir::ir::CaptureUse::Take,
                        UseKind::Copy => hirsel_lir::ir::CaptureUse::Copy,
                        UseKind::Mut => return unsupported("a mut capture"),
                    };
                    let v = self.operand(c)?;
                    // A copied capture was copied at its read: the
                    // environment takes that owned copy.
                    let usage = if usage == hirsel_lir::ir::CaptureUse::Copy && self.holds(v) {
                        hirsel_lir::ir::CaptureUse::Take
                    } else {
                        usage
                    };
                    values.push(CaptureValue { value: v, usage });
                }
                if let Some(environment) = data_environment {
                    let fields = values
                        .iter()
                        .enumerate()
                        .map(|(field, c)| FieldValue {
                            field: field as u32,
                            value: c.value,
                        })
                        .collect();
                    return self.construct(
                        ConstructorKind::Tuple,
                        environment,
                        fields,
                        self.destination_class(destination)?,
                        ty,
                    );
                }
                let instance = self
                    .instances
                    .closure(self.id, *body)
                    .ok_or_else(|| InstanceError::MissingInstance(self.def.path.path().clone()))?;
                let (code, environment) = {
                    let digest = self.instances.view(instance).key.digest();
                    let symbol = self.symbol(
                        SymbolKind::Function,
                        &self.def.path.path().clone(),
                        Some(digest),
                    );
                    let code = ClosureCode {
                        symbol,
                        abi: self.instances.abis[instance.index()],
                    };
                    let environment = self.instances.environments[instance.index()]
                        .expect("closure instances have an environment");
                    (Some(code), environment)
                };
                let anchor = self.anchor_here();
                let captures =
                    PoolRange::push(&mut self.out.pools.captures, values.iter().copied());
                let closure = hirsel_lir::ir::ClosureId(self.out.tables.closures.len() as u32);
                self.out.tables.closures.push(ClosureSite {
                    receipts: None,
                    scoped: *context == hirsel_types::CaptureContext::Scoped,
                    code,
                    environment,
                    captures,
                    destination: self.destination_class(destination)?,
                    placement: None,
                    anchor,
                });
                for c in &values {
                    self.consumed(c.value);
                }
                let result = self.fresh(ty);
                let failure = self.fail(None)?;
                self.push(Inst::Closure {
                    result,
                    closure,
                    failure,
                });
                result
            }
            V::Shape(_) => {
                let anchor = self.anchor_here();
                self.push(Inst::Unavailable {
                    feature: hirsel_lir::ir::UnavailableFeature::Shape,
                    anchor,
                });
                return unsupported("Shape static data is not materialized");
            }
        })
    }

    /// `unique x`: a deep copy of the source's owned graph (plain content is
    /// a bit copy). The source is read as a view; one without a loan of its
    /// own is viewed through a synthetic loan around the copy.
    pub(super) fn unique(&mut self, value: &tir::Operand, ty: LirTypeId) -> R<ValueId> {
        let p = self.place(value.place)?;
        let address = self.address(&p)?;
        // A variable flat field is an encoding, not an ordinary value with
        // a holder to borrow. Decoding it already creates an independent
        // owned graph, just as copying or taking that field does.
        if p.source.is_some() || self.bits(p.ty) {
            return self.operand_at(UseKind::Copy, &p, address, None);
        }
        // A joined loop/pattern view has SSA backing and no initialized slot.
        // Read that transported view, as ordinary operands and tag tests do.
        let reaching = match p.value {
            Some(value) => value,
            None => self.reaching(p.slot)?,
        };
        let tir_loan = p.loan;
        let (holding, synthetic) = match tir_loan {
            Some(l) if self.state.loans[self.loan(l)?.index()] == Tri::Yes => {
                (Holding::Borrowed(self.loan(l)?), None)
            }
            _ => {
                let loan = self.synthetic_loan(address);
                (Holding::Borrowed(loan), Some(loan))
            }
        };
        let view = self.value(p.ty, holding);
        self.push(Inst::Read {
            result: view,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        let layout = match self.record(ty).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => self.layout_of(ty),
        };
        let anchor = self.anchor_here();
        // A copy of a List or Array of rows, a Map or a Set in an arena is
        // tracked there (MEM-6).
        let tracked = self.tracked_block(self.body.places[value.place.0 as usize].ty, layout)?;
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked,
            constructor: ConstructorKind::Tuple,
            layout,
            fields: PoolRange::EMPTY,
            destination: DestinationClass::Inferred,
            placement: None,
            anchor,
        });
        let result = self.fresh(ty);
        if let Some(loan) = synthetic {
            self.failure.before.insert(0, Action::EndLoan { loan });
        }
        let failure = self.fail(None);
        if synthetic.is_some() {
            self.failure.before.remove(0);
        }
        let failure = failure?;
        self.push(Inst::UniqueCopy {
            result,
            value: view,
            construct,
            deep: true,
            failure,
        });
        if let Some(loan) = synthetic {
            self.push(Inst::EndLoan {
                loan,
                if_active: false,
            });
            self.state.loans[loan.index()] = Tri::No;
        }
        if self.holds(result) {
            self.failure.pending.push(result);
        }
        Ok(result)
    }

    /// A construction's field values in source order. A field in `encoded`
    /// is a flat record's variable field: its source is only read, from a
    /// construction-owned temporary or under a loan the caller ends (`loans`,
    /// `taken`, for `reading`) once the construction is built.
    pub(super) fn construct_fields(
        &mut self,
        encoded: &[u32],
        fields: &[tir::ConstructField],
    ) -> R<(Vec<FieldValue>, Vec<LoanId>, Vec<SlotId>)> {
        let mut values = Vec::new();
        let (mut loans, mut taken) = (Vec::new(), Vec::new());
        let before = (!encoded.is_empty()).then(|| self.failure.before.clone());
        for f in fields {
            let v = if encoded.contains(&f.field) {
                self.compared(&f.value, &mut loans, &mut taken)?
            } else {
                self.operand(&f.value)?
            };
            // Preparing a later operand can fail after earlier encoded
            // sources have entered construction-owned temporaries.
            if let Some(before) = &before {
                let mut prepared = self.reading_actions(&loans, &taken);
                prepared.extend(before.iter().cloned());
                self.failure.before = prepared;
            }
            values.push(FieldValue {
                field: f.field,
                value: v,
            });
        }
        if let Some(before) = before {
            self.failure.before = before;
        }
        Ok((values, loans, taken))
    }

    /// Drop the value at a fixed `place` (TIR `Drop`).
    fn drop_place(&mut self, place: tir::PlaceId) -> R<()> {
        let p = self.place(place)?;
        if !p.fixed {
            return unsupported("a drop of a computed place");
        }
        let action = Action::Release {
            slot: p.slot,
            anchor: (INSTRUCTION_DROP, self.statement_action()),
        };
        let step = if p.steps.is_empty() {
            self.action(&action)?
        } else {
            self.part_drop(&p)?
        };
        if let Step2::Drop(drop) = step {
            drop.apply(self);
            let failure = self.fail(None)?;
            self.push(Inst::Drop {
                address: drop.address,
                if_live: drop.if_live,
                anchor: drop.anchor,
                failure,
            });
        }
        Ok(())
    }

    /// `box C()` of a nullary variant `C` on the heap: the one static immortal block of
    /// that variant at the payload's layout (MEM-7), so nothing is allocated and its release
    /// is a count test. The variant the box would have moved out of its temporary is dropped
    /// there instead.
    /// A resource's variant is constructed, since its drop runs its finalizer.
    fn nullary_box(
        &mut self,
        constructor: &tir::Constructor,
        fields: &[tir::ConstructField],
        destination: &hirsel_types::Destination,
        ty: LirTypeId,
    ) -> R<Option<ValueId>> {
        let (tir::Constructor::Box, [field]) = (constructor, fields) else {
            return Ok(None);
        };
        let place = &self.body.places[field.value.place.0 as usize];
        let Some(variant) = self.nullary[place.root.0 as usize] else {
            return Ok(None);
        };
        let LayoutKind::Box { payload } = self.record(ty).kind else {
            return Ok(None);
        };
        if !place.projection.is_empty()
            || self.out.types[ty.index()].repr != ValueRepr::Block
            || !matches!(
                self.destination_class(destination)?,
                DestinationClass::Heap | DestinationClass::Inferred
            )
        {
            return Ok(None);
        }
        let key = self.static_layout_key(payload);
        let Some(instance) =
            crate::statics::nullary_variant_instance(self.layouts.record(payload), key, variant)
        else {
            return Ok(None);
        };
        self.drop_place(field.value.place)?;
        let symbol = self.symbol(SymbolKind::Constant, &nullary_variant(), Some(instance));
        let result = self.fresh(ty);
        self.push(Inst::ReadStatic { result, symbol });
        Ok(Some(result))
    }

    /// Push a construction site and build its value; the fields are consumed
    /// even when the construction fails.
    pub(super) fn construct(
        &mut self,
        constructor: ConstructorKind,
        layout: LayoutId,
        values: Vec<FieldValue>,
        destination: DestinationClass,
        ty: LirTypeId,
    ) -> R<ValueId> {
        // An empty sequence holds no element: it is the static empty
        // sequence of its layout, which the first reserve or growth replaces
        // (`hirsel_rt_v2_seq_grow`), and allocates nothing. A source-required
        // payload receipt keeps its checked construction correspondence.
        if constructor == ConstructorKind::Sequence
            && values.is_empty()
            && matches!(
                destination,
                DestinationClass::Heap | DestinationClass::Inferred
            )
            && match self.layouts.record(layout).kind {
                LayoutKind::Sequence { .. } | LayoutKind::SoaColumns { .. } => true,
                // A flat record builder's empty allocation is also its write
                // destination. Keep that path; only ordinary Packed lists use
                // static empty storage here.
                LayoutKind::Sequential { element } => {
                    self.layouts.record(element).kind == LayoutKind::Packed
                }
                _ => false,
            }
            && matches!(
                self.layouts.record(self.layout_of(ty)).kind,
                LayoutKind::Block { payload } if payload == layout
            )
            && !self.needs_construction_receipt()
        {
            let instance = self.static_layout_key(layout).0;
            let symbol = self.symbol(SymbolKind::Constant, &empty_sequence(), Some(instance));
            let result = self.fresh(ty);
            self.push(Inst::ReadStatic { result, symbol });
            return Ok(result);
        }
        let anchor = self.anchor_here();
        let fields = PoolRange::push(&mut self.out.pools.fields, values.iter().copied());
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor,
            layout,
            fields,
            destination,
            placement: None,
            anchor,
        });
        for f in &values {
            self.consumed(f.value);
        }
        let result = self.fresh(ty);
        let failure = self.fail(None)?;
        self.push(Inst::Construct {
            result,
            construct,
            failure,
        });
        self.construct_receipt(result, construct)?;
        Ok(result)
    }

    /// Container syntax is an empty private record, then ordinary insert
    /// calls. No backend container-constructor vocabulary survives import.
    pub(super) fn keyed_construct(
        &mut self,
        kind: hirsel_types::BuiltinType,
        fields: &[tir::ConstructField],
        destination: DestinationClass,
        ty: LirTypeId,
    ) -> R<ValueId> {
        let layout = self.layout_of(ty);
        let payload = match self.layouts.record(layout).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => return malformed("container block"),
        };
        let shape: Vec<_> = self
            .layouts
            .record(payload)
            .fields
            .iter()
            .map(|f| (f.ordinal, f.layout))
            .collect();
        let mut values = Vec::new();
        for (field, layout) in shape {
            if matches!(
                kind,
                hirsel_types::BuiltinType::Map | hirsel_types::BuiltinType::Set
            ) && field == 3
            {
                let ty = self.intern(layout);
                let constant = self.constant(ty, ConstantValue::Integer(0));
                let value = self.value(ty, Holding::Plain);
                self.push(Inst::Const {
                    result: value,
                    constant,
                });
                values.push(FieldValue { field, value });
                continue;
            }
            let sequence = match self.layouts.record(layout).kind {
                LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
                _ => return malformed("container sequence"),
            };
            let sequence_ty = self.intern(layout);
            let site = self.out.tables.constructs.len();
            let value = self.construct(
                ConstructorKind::Sequence,
                sequence,
                vec![],
                destination.clone(),
                sequence_ty,
            )?;
            // A Map's or Set's entries are tracked in an arena with it, its
            // index never (MEM-6).
            if field == 0 && self.out.tables.constructs.len() > site {
                self.out.tables.constructs[site].tracked = true;
            }
            if self.holds(value) {
                self.failure.pending.push(value);
            }
            values.push(FieldValue { field, value });
        }
        let symbol = self.symbol(
            SymbolKind::Type,
            &hirsel_syntax::DefPath::core(match kind {
                hirsel_types::BuiltinType::Map => "Map",
                hirsel_types::BuiltinType::Set => "Set",
                hirsel_types::BuiltinType::SortedMap => "SortedMap",
                _ => return malformed("keyed constructor kind"),
            }),
            None,
        );
        let site = self.out.tables.constructs.len();
        let empty = self.construct(
            ConstructorKind::Record(symbol),
            payload,
            values,
            destination,
            ty,
        )?;
        self.out.tables.constructs[site].tracked = true;
        if fields.is_empty() {
            return Ok(empty);
        }
        if self.holds(empty) {
            self.failure.pending.push(empty);
        }
        let scope = self.out.slots[self
            .slot_for(self.body.places[fields[0].value.place.0 as usize].root.0 as usize)?
            .index()]
        .scope;
        let slot = self.new_slot(ty, SlotKind::Temporary, scope);
        self.push(Inst::StorageLive { slot });
        self.state.live(slot);
        let place = self.whole(slot);
        let address = self.address(&place)?;
        self.store(&place, address, empty)?;
        let release = Action::Release {
            slot,
            anchor: (INSTRUCTION_DROP, self.statement_action()),
        };
        self.failure.before.insert(0, release.clone());
        let is_map = kind == hirsel_types::BuiltinType::Map;
        // The container's own type is among the definition's TIR types.
        let mut candidates = Vec::new();
        for &ty in self.def.types.iter() {
            let id = self.ty(ty)?;
            if let hirsel_types::TypeKind::Builtin { kind: k, arguments } = &self.types.row(id).kind
                && *k == kind
            {
                candidates.push((id, arguments.to_vec()));
            }
        }
        let mut actuals = None;
        for (candidate, arguments) in candidates {
            if self.layout(candidate)? == layout {
                actuals = Some(arguments);
                break;
            }
        }
        let actuals = actuals
            .ok_or_else(|| PassError::Unsupported("container constructor actuals".into()))?;
        let path = hirsel_syntax::DefPath::core(if is_map { "map.insert" } else { "set.insert" });
        let body = crate::library::body_index(&path).expect("registered insertion");
        let request = crate::library::Request::operation(self.program, body, &path, actuals)?;
        let direct = self.require(request)?;
        let symbol = self.symbol(
            direct.symbol.kind,
            &direct.symbol.definition,
            direct.symbol.instance,
        );
        for field in fields {
            let value = self.operand(&field.value)?;
            let mut args = Vec::new();
            let pair = if is_map {
                let pair_ty = self.value_type(value);
                let s = self.new_slot(pair_ty, SlotKind::Temporary, scope);
                self.push(Inst::StorageLive { slot: s });
                self.state.live(s);
                let whole = self.whole(s);
                let a = self.address(&whole)?;
                self.store(&whole, a, value)?;
                let release = Action::Release {
                    slot: s,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                };
                self.failure.before.insert(0, release.clone());
                let record = self.record(pair_ty).clone();
                let mut key = whole.clone();
                key.steps.push(Step::Field(0));
                key.projection.push(ProjectionStep::Field(0));
                key.ty = self.intern(record.fields[0].layout);
                let key_address = self.address(&key)?;
                let k = self.operand_at(UseKind::Take, &key, key_address, None)?;
                let mut item = whole;
                item.steps.push(Step::Field(1));
                item.projection.push(ProjectionStep::Field(1));
                item.ty = self.intern(record.fields[1].layout);
                let a = self.address(&item)?;
                let v = self.operand_at(UseKind::Take, &item, a, None)?;
                args.push(CallArgument {
                    parameter: 1,
                    passing: Passing::Take,
                    value: ArgumentValue::Value(k),
                });
                args.push(CallArgument {
                    parameter: 2,
                    passing: Passing::Take,
                    value: ArgumentValue::Value(v),
                });
                Some((s, release))
            } else {
                args.push(CallArgument {
                    parameter: 1,
                    passing: Passing::Take,
                    value: ArgumentValue::Value(value),
                });
                None
            };
            let version = self.value(ty, Holding::Plain);
            self.versions[slot.index()] = Some(version);
            args.insert(
                0,
                CallArgument {
                    parameter: 0,
                    passing: Passing::Mut,
                    value: ArgumentValue::Address { address, version },
                },
            );
            Inline::direct_call(self, symbol, direct.abi, args)?;
            if let Some((s, release)) = pair {
                self.failure.before.remove(0);
                self.run_normal(&[release], &[])?;
                self.push(Inst::StorageDead {
                    slot: s,
                    if_live: false,
                });
                self.state.dead(s);
            }
        }
        self.failure.before.remove(0);
        let result = self.operand_at(UseKind::Take, &place, address, None)?;
        self.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
        self.state.dead(slot);
        self.failure.pending.retain(|&v| v != result);
        Ok(result)
    }

    /// The nominal a distinct type is declared over, if any.
    pub(super) fn wrapped_nominal(&self, path: &DefPath) -> Option<DefPath> {
        let signature = self.program.signatures.get(path)?;
        let SignatureKind::Type(t) = &signature.kind else {
            return None;
        };
        let representation = t.representation?;
        match &self.types.get(representation)?.kind {
            hirsel_types::TypeKind::Named { definition, .. } => Some(definition.path().clone()),
            _ => None,
        }
    }

    /// The symbol and ABI of a direct callee instance.
    pub(super) fn direct(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<(SymbolId, FnAbiId)> {
        // CT-5: a generic generated declaration is its selected instance's
        // monomorphic definition.
        if self
            .program
            .instance_bodies
            .declarations
            .contains(&DefId::intern(path))
        {
            let callee = crate::instance::callee_actuals(
                self.types,
                &self.program.memo,
                None,
                arguments,
                self.actuals,
                false,
                path,
            )?;
            let instance = match self.program.instance_bodies.resolve(
                DefId::intern(path),
                self.types,
                &callee.arguments,
            ) {
                Ok(Some(instance)) => instance.path().clone(),
                Ok(None) => return unsupported(format!("{path:?} has no instance bodies")),
                Err(error) => return unsupported(format!("{error:?}")),
            };
            return self.direct(&instance, &[]);
        }
        if let Some(body) = crate::library::body_index(path) {
            let request = self.library_request(body, path, arguments)?;
            let direct = self.require(request)?;
            return Ok((
                self.symbol(SymbolKind::Function, path, direct.symbol.instance),
                direct.abi,
            ));
        }
        // An operation without a library body is named, as a value, by its
        // forwarding body (`library::value`).
        if self.program.operations.contains_key(path) && !self.instances.has_function(path) {
            if !crate::library::value::forwards(path) {
                return unsupported(format!(
                    "{path:?} as a function value: it is lowered inline at each call"
                ));
            }
            let callee = crate::instance::callee_actuals(
                self.types,
                &self.program.memo,
                self.program.binders.get(path).copied(),
                arguments,
                self.actuals,
                false,
                path,
            )?;
            let request =
                crate::library::Request::operation_value(self.program, path, callee.arguments)?;
            let direct = self.require(request)?;
            return Ok((
                self.symbol(SymbolKind::Function, path, direct.symbol.instance),
                direct.abi,
            ));
        }
        let callee = crate::instance::callee_actuals(
            self.types,
            &self.program.memo,
            self.program.binders.get(path).copied(),
            arguments,
            self.actuals,
            false,
            path,
        )?;
        let id = self.instances.find(self.program, path, &callee)?;
        let digest = self.instances.view(id).key.digest();
        Ok((
            self.symbol(SymbolKind::Function, path, Some(digest)),
            self.instances.abis[id.index()],
        ))
    }
}
