//! Independent selected-state proofs: binder-exact, field-sensitive effect
//! discipline over the invocation's symbolic incoming snapshot.
use super::*;
use hirsel_types::{AuthoritySummary, Brand, Multiplicity};

/// One promise of the body being validated, on its own parameter local.
#[derive(Clone, Debug)]
pub(super) struct Selection {
    root: usize,
    fields: Vec<u32>,
    advance: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Relation {
    /// The access is the selection, an ancestor of it, an alias of its root
    /// or reaches it through a non-field step.
    Whole,
    /// The access lies strictly inside the selection.
    Part,
}

pub(super) fn selections(
    function: Option<&hirsel_types::FunctionType>,
    body: &ReadBody<'_>,
) -> Vec<Selection> {
    let Some(function) = function else {
        return vec![];
    };
    function
        .state_promises
        .iter()
        .filter(|promise| promise.preserve_captures)
        .filter_map(|promise| {
            let ParameterRef::Bound {
                depth: 0,
                parameter,
            } = promise.selector.parameter
            else {
                return None;
            };
            Some(Selection {
                root: body.parameters.get(parameter.0 as usize)?.0 as usize,
                fields: promise.selector.fields.iter().map(|f| f.0).collect(),
                advance: promise.advance_only,
            })
        })
        .collect()
}

fn relation(projection: &[Projection], fields: &[u32]) -> Option<Relation> {
    for (depth, step) in projection.iter().enumerate() {
        let Some(selected) = fields.get(depth) else {
            return Some(Relation::Part);
        };
        match step {
            Projection::Field(field) if field == selected => {}
            Projection::Field(_) => return None,
            _ => return Some(Relation::Whole),
        }
    }
    Some(Relation::Whole)
}

impl Checker<'_, '_> {
    /// Every selection an access meets, with the access's leading field path.
    pub(super) fn selected(
        &self,
        model: &Model,
        state: &State,
        place: PlaceId,
    ) -> Vec<(Selection, Relation, Vec<u32>)> {
        if model.state_promises.is_empty() {
            return vec![];
        }
        let access = &self.body.places[place.0 as usize];
        let root = access.root.0 as usize;
        let mut sources = BTreeSet::new();
        if !state.must[root].has(MustFacts::OWNED) {
            for loan in self.dependencies(
                model,
                state,
                &Operand {
                    place,
                    usage: UseKind::Borrow,
                },
            ) {
                sources.insert(self.body.places[model.loans[loan].1.0 as usize].root.0 as usize);
            }
        }
        let leading: Vec<u32> = access
            .projection
            .iter()
            .map_while(|step| match step {
                Projection::Field(field) => Some(*field),
                _ => None,
            })
            .collect();
        let mut met = vec![];
        for selection in &model.state_promises {
            if selection.root == root {
                if let Some(relation) = relation(&access.projection.to_vec(), &selection.fields) {
                    met.push((selection.clone(), relation, leading.clone()));
                }
            } else if sources.contains(&selection.root) {
                met.push((selection.clone(), Relation::Whole, vec![]));
            }
        }
        met
    }

    /// A closed type fixes every envelope axis of each of its values. A
    /// callable slot's root row is only its call-bounded view.
    pub(super) fn closed(&self, types: &TypeInterner, root: TypeId, view: bool) -> bool {
        let mut pending = vec![root];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = types.get(ty).unwrap();
            if row.erased_captures.is_some()
                || row.brand != Brand::default()
                || !(view && ty == root || row.modes.relation.is_global())
                || row.modes.multiplicity != Multiplicity::Many
            {
                return false;
            }
            match &row.kind {
                TypeKind::Parameter(_)
                | TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Iter,
                    ..
                } => return false,
                TypeKind::Named { .. } => {
                    if !self.bound(
                        types,
                        ty,
                        hirsel_types::Bound::Plain,
                        &self.generic_declarations(),
                    ) {
                        return false;
                    }
                }
                kind => pending.extend(kind.value_parts()),
            }
        }
        self.authority_with_generics(types, root, self.generic_declarations())
            == AuthoritySummary::inert()
    }

    /// Closed content keeps the snapshot when it is replaced; advance-only
    /// admits that only strictly inside the selection (bookkeeping).
    fn closed_state_effect(
        &self,
        selection: &Selection,
        relation: Relation,
        place: PlaceId,
    ) -> bool {
        (relation == Relation::Part || !selection.advance)
            && self.closed(self.t, self.place_type(place), false)
    }

    /// Pure counted data has no hidden environment; only its ownership receipt
    /// can vary. The root row is the owner's mutable view, not stored locality.
    fn fixed_inert_data(&self, root: TypeId) -> bool {
        let types = self.t;
        let mut pending = vec![root];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = types.get(ty).unwrap();
            if row.erased_captures.is_some()
                || row.brand != Brand::default()
                || ty != root && !row.modes.relation.is_global()
                || row.modes.multiplicity != Multiplicity::Many
                || row.drop_allocation != hirsel_types::DropAllocation::NoAlloc
            {
                return false;
            }
            match &row.kind {
                TypeKind::Parameter(_)
                | TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Iter,
                    ..
                } => return false,
                TypeKind::Named { .. }
                    if !self.bound(
                        types,
                        ty,
                        hirsel_types::Bound::Copyable,
                        &self.generic_declarations(),
                    ) =>
                {
                    return false;
                }
                kind => pending.extend(kind.value_parts()),
            }
        }
        self.authority_with_generics(types, root, self.generic_declarations())
            == AuthoritySummary::inert()
    }

    pub(super) fn independent_global_data(&self, state: &State, value: &Operand) -> bool {
        if !self.global_operand(state, value) {
            return false;
        }
        if self.operand_unique(state, value) {
            return true;
        }
        let store = self.t;
        let ty = self.transferred_type(state, value, store);
        self.proof(
            &store,
            ty,
            super::proofs::Proof::Content,
            &self.generic_declarations(),
        )
        .is_ok_and(|proof| proof == hirsel_types::Sendability::Sendable)
    }

    fn callee_function(&self, callee: &Callee) -> Option<&hirsel_types::FunctionType> {
        match callee {
            Callee::Definition { path, .. } => {
                self.signatures.get(path).and_then(|s| match &s.kind {
                    SignatureKind::Function(f) => Some(f),
                    _ => None,
                })
            }
            Callee::Indirect { value, .. } => {
                self.t
                    .get(self.place_type(value.place))
                    .and_then(|row| match &row.kind {
                        TypeKind::Function(f) => Some(f),
                        _ => None,
                    })
            }
        }
    }

    fn state_violation(&mut self, report: bool) {
        self.flow_error(
            report,
            ViolationKind::TypeMismatch {
                entity: "iterator state preservation",
            },
        );
    }

    /// A write, move or mutable capture of the access.
    pub(super) fn state_write(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        report: bool,
    ) {
        self.state_install(model, state, place, false, report);
    }

    pub(super) fn state_install(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        independent_data: bool,
        report: bool,
    ) {
        for (selection, relation, _) in self.selected(model, state, place) {
            let inert = independent_data
                && !selection.advance
                && relation == Relation::Part
                && self.fixed_inert_data(self.place_type(place));
            if !self.closed_state_effect(&selection, relation, place) && !inert {
                self.state_violation(report);
            }
        }
    }

    /// A copy shares the selected content unless it is stable, or is the
    /// callee temporary of an invocation of a many-use callable.
    pub(super) fn state_copy(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        report: bool,
    ) {
        if !self.selected(model, state, place).is_empty()
            && !self.copy_creation(place)
            && !self.invocation_copy(place)
        {
            self.state_violation(report);
        }
    }

    /// The current instruction copies `place` into a temporary many-use
    /// callable whose only consumer is an indirect call: it is invoked, never
    /// moved, copied, lent or returned.
    fn invocation_copy(&self, place: PlaceId) -> bool {
        let (Some(block), Some(index)) = (self.site.block, self.site.instruction) else {
            return false;
        };
        let Some(InstructionKind::Assign { destination, value }) = self.body.blocks
            [block.0 as usize]
            .instructions
            .get(index as usize)
            .map(|i| &i.kind)
        else {
            return false;
        };
        if !matches!(self.body.rvalues.get(value.0 as usize), Some(Rvalue::Use(o)) if o.place == place)
        {
            return false;
        }
        let local = self.body.places[destination.0 as usize].root;
        if self.body.locals[local.0 as usize].storage != Storage::Temporary
            || !matches!(
                self.t.get(self.place_type(*destination)),
                Some(row) if matches!(row.kind, TypeKind::Function(_))
                    && row.modes.multiplicity == Multiplicity::Many
            )
        {
            return false;
        }
        let names = |p: &PlaceId| self.body.places[p.0 as usize].root == local;
        let escapes = |o: &Operand| names(&o.place);
        !self.body.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|instruction| match &instruction.kind {
                    InstructionKind::Assign { value, .. } => self
                        .body
                        .rvalues
                        .get(value.0 as usize)
                        .is_some_and(|r| typing::operands(r).into_iter().any(escapes)),
                    InstructionKind::Replace { value, .. }
                    | InstructionKind::Snapshot { value, .. }
                    | InstructionKind::Check {
                        condition: value, ..
                    } => escapes(value),
                    InstructionKind::BeginLoan { place, .. } => names(place),
                    InstructionKind::BindOrigin { source, place, .. } => {
                        names(source) || names(place)
                    }
                    InstructionKind::BindFields { source, fields } => {
                        names(source) || fields.iter().any(names)
                    }
                    _ => false,
                })
                || match &block.terminator {
                    Terminator::Call { arguments, .. } => {
                        arguments.iter().any(|a| escapes(&a.value))
                    }
                    Terminator::Return { value, .. }
                    | Terminator::Branch {
                        condition: value, ..
                    }
                    | Terminator::Switch { value, .. } => escapes(value),
                    _ => false,
                }
        })
    }

    /// A mutable capture can install anything later.
    pub(super) fn state_capture(
        &mut self,
        model: &Model,
        state: &State,
        place: PlaceId,
        report: bool,
    ) {
        if !self.selected(model, state, place).is_empty() {
            self.state_violation(report);
        }
    }

    pub(super) fn state_argument(
        &mut self,
        model: &Model,
        state: &State,
        callee: &Callee,
        argument: &CallArgument,
        arguments: &[CallArgument],
        report: bool,
    ) {
        for (selection, relation, fields) in self.selected(model, state, argument.value.place) {
            if self.closed_state_effect(&selection, relation, argument.value.place) {
                continue;
            }
            let inert = !selection.advance
                && relation == Relation::Part
                && self.fixed_inert_data(self.place_type(argument.value.place))
                && self.callee_preserves_unique(callee, argument.parameter)
                && arguments.iter().all(|other| {
                    other.parameter == argument.parameter
                        || self.bound(
                            self.t,
                            self.place_type(other.value.place),
                            hirsel_types::Bound::Plain,
                            &self.generic_declarations(),
                        )
                        || self.independent_global_data(state, &other.value)
                });
            let covered = inert
                || self.callee_function(callee).is_some_and(|f| {
                    f.state_promises.iter().any(|p| {
                        let promised: Vec<u32> = fields
                            .iter()
                            .copied()
                            .chain(p.selector.fields.iter().map(|f| f.0))
                            .collect();
                        p.selector.parameter == ParameterRef::bound(argument.parameter)
                            && p.preserve_captures
                            && (!selection.advance || p.advance_only)
                            && (p.selector.fields.is_empty()
                                || selection.fields.starts_with(&promised))
                    })
                });
            if !covered {
                self.state_violation(report);
            }
        }
    }
}
