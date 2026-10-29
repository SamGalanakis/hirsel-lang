//! Current receiving content is independent of its original storage lifetime.
use super::*;

/// Static eligibility, independently derived from IR before any flow judgment.
#[derive(Default)]
pub(crate) struct ContentInputs {
    receiving_roots: BTreeSet<u32>,
    global_projections: BTreeSet<u32>,
}

pub(crate) struct ContentProof {
    pub(crate) acquirable: bool,
    pub(crate) deep: bool,
}

impl Checker<'_, '_> {
    fn content_inputs(&self) -> &ContentInputs {
        self.scratch.content_inputs[self.site.body.0 as usize]
            .get()
            .unwrap()
    }

    pub(crate) fn prepare_content_inputs(&mut self) {
        let inputs = &self.scratch.content_inputs[self.site.body.0 as usize];
        if inputs.get().is_some() {
            return;
        }
        let receiving_roots = self
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| {
                let destination = match instruction.kind {
                    InstructionKind::Replace { destination, .. } => destination,
                    InstructionKind::Assign { destination, value } => {
                        let Rvalue::Use(value) = &self.body.rvalues[value.0 as usize] else {
                            return None;
                        };
                        if !self
                            .lifetime(
                                &self
                                    .t
                                    .get(self.place_type(value.place))
                                    .unwrap()
                                    .modes
                                    .relation,
                            )
                            .is_global()
                        {
                            return None;
                        }
                        destination
                    }
                    _ => return None,
                };
                let place = &self.body.places[destination.0 as usize];
                (place.projection.is_empty()
                    && matches!(
                        self.body.locals[place.root.0 as usize].storage,
                        Storage::Local | Storage::Temporary
                    ))
                .then_some(place.root.0)
            })
            .collect();
        let global_projections = self
            .body
            .places
            .iter()
            .enumerate()
            .filter_map(|(index, place)| {
                if place.projection.is_empty() {
                    return None;
                }
                let store = self.t;
                self.global_root_projection(store, place)
                    .is_some_and(|part| {
                        self.lifetime(&store.get(part).unwrap().modes.relation)
                            .is_global()
                    })
                    .then_some(index as u32)
            })
            .collect();
        let _ = inputs.set(ContentInputs {
            receiving_roots,
            global_projections,
        });
    }

    /// Initialization from Global content and whole replacement can strengthen
    /// body storage's current lifetime. Eligibility is not flow evidence.
    pub(crate) fn receiving_root(&self, root: LocalId) -> bool {
        self.content_inputs().receiving_roots.contains(&root.0)
    }

    pub(crate) fn global_root_projection(
        &mut self,
        store: &TypeInterner,
        place: &Place,
    ) -> Option<hirsel_types::TypeId> {
        let root = self.body.locals[place.root.0 as usize].ty;
        if !store.get(root).unwrap().modes.relation.has_environment() {
            return None;
        }
        let mut row = store.get(root).unwrap().clone();
        row.modes.relation = self.placement(&row.modes.relation);
        let root = store.intern(row);
        let root = self.derive_sendability(store, root);
        self.projection_type_at(store, place, root, |_, _| None)
    }

    /// Static acquisition eligibility and the must-owned deep-content proof
    /// stay separate: typing can use eligibility, only flow can supply evidence.
    pub(crate) fn content_proof(&self, state: Option<&State>, place: PlaceId) -> ContentProof {
        let access = &self.body.places[place.0 as usize];
        let root = access.root.0 as usize;
        ContentProof {
            acquirable: access.projection.is_empty()
                || self
                    .lifetime(
                        &self
                            .t
                            .get(self.body.locals[root].ty)
                            .unwrap()
                            .modes
                            .relation,
                    )
                    .is_global()
                || self.content_inputs().global_projections.contains(&place.0),
            deep: state.is_some_and(|state| {
                (state.must[root].has(MustFacts::GLOBAL)
                    || !access.projection.is_empty()
                        && matches!(self.t.get(access.ty).unwrap().kind, TypeKind::Function(_))
                        && state.crossing_parts[root]
                            .get(&super::flow::crossing::path(&access.projection))
                            == Some(true))
                    && state.must[root].has(MustFacts::OWNED)
                    && !state.ownership_conflicts[root]
            }),
        }
    }

    pub(super) fn current_content_global(&self, state: &State, value: &Operand) -> bool {
        let access = &self.body.places[value.place.0 as usize];
        let receiving = &self
            .t
            .get(self.body.locals[access.root.0 as usize].ty)
            .unwrap()
            .modes
            .relation;
        let proof = self.content_proof(Some(state), value.place);
        proof.acquirable
            && (proof.deep
                || self
                    .lifetime(&self.t.get(access.ty).unwrap().modes.relation)
                    .is_global()
                    && !self.lifetime(receiving).has_environment())
    }

    pub(super) fn global_operand(&self, state: &State, value: &Operand) -> bool {
        let access = &self.body.places[value.place.0 as usize];
        let receiving = &self
            .t
            .get(self.body.locals[access.root.0 as usize].ty)
            .unwrap()
            .modes
            .relation;
        let proof = self.content_proof(Some(state), value.place);
        proof.acquirable
            && (self
                .lifetime(&self.t.get(access.ty).unwrap().modes.relation)
                .is_global()
            && (!self.lifetime(receiving).has_environment() || proof.deep)
            // A Local part (a Local receiver's, or an inferred Local join's)
            // is acquired as Global only from current deep content.
            || matches!(value.usage, UseKind::Copy | UseKind::Take) && proof.deep)
    }

    pub(super) fn rvalue_global(
        &self,
        state: &State,
        destination: PlaceId,
        value: &Rvalue,
    ) -> bool {
        match value {
            Rvalue::Use(value) => self.global_operand(state, value),
            Rvalue::Closure { captures, .. } => captures
                .iter()
                .all(|capture| self.global_operand(state, capture)),
            Rvalue::Construct { fields, .. } => {
                self.lifetime(
                    &self
                        .t
                        .get(self.place_type(destination))
                        .unwrap()
                        .modes
                        .relation,
                )
                .is_global()
                    && fields
                        .iter()
                        .all(|field| self.global_operand(state, &field.value))
            }
            _ => self
                .lifetime(
                    &self
                        .t
                        .get(self.place_type(destination))
                        .unwrap()
                        .modes
                        .relation,
                )
                .is_global(),
        }
    }

    pub(super) fn set_current_global(&self, state: &mut State, destination: PlaceId, global: bool) {
        let access = &self.body.places[destination.0 as usize];
        let root = access.root.0 as usize;
        if access.projection.is_empty() {
            state.must.set_fact(root, MustFacts::GLOBAL, global);
        } else if needs_release(self, access.ty, self.t) {
            if !global {
                state.must.set_fact(root, MustFacts::GLOBAL, false);
            }
        }
    }
}
