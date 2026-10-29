//! Independently derived cursor contracts and finite read reachability.
//! A summarized descendant is never an exact identity or branded owner.
use super::*;
use crate::origin_dependencies::Dependencies;

#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct View {
    pub(super) anchors: BTreeSet<Origin>,
    pub(super) imprecise: bool,
    pub(super) lineages: BTreeSet<usize>,
}
impl View {
    pub(super) fn join(&mut self, other: &Self) {
        self.anchors.extend(other.anchors.iter().cloned());
        self.imprecise |= other.imprecise;
        self.lineages
            .retain(|lineage| other.lineages.contains(lineage));
    }
    fn within(&self, cover: &BTreeSet<Origin>) -> bool {
        !self.anchors.is_empty()
            && self.anchors.iter().all(|anchor| {
                cover
                    .iter()
                    .any(|allowed| crate::flow_origins::contains(allowed, anchor))
            })
    }
}
#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct Contract {
    pub(super) cover: BTreeSet<Origin>,
    pub(super) expired: BTreeSet<Origin>,
    pub(super) snapshot: bool,
}
impl Dependencies for Contract {
    fn roots(&self) -> BTreeSet<OriginRoot> {
        self.cover
            .iter()
            .map(|origin| origin.root.clone())
            .collect()
    }
}

impl State {
    pub(super) fn expire_origin(&mut self, origin: &Origin) {
        self.origins.expire(origin);
        for holder in self.cursor_contracts.holders(&origin.root) {
            self.cursor_contracts.update(holder, |contract| {
                if contract
                    .cover
                    .iter()
                    .any(|anchor| crate::flow_origins::overlaps(origin, anchor))
                {
                    contract.expired.insert(origin.clone());
                }
            });
        }
    }
    pub(super) fn widen_cursors(&mut self, model: &Model, body: &ReadBody<'_>) {
        for cursor in &body.borrow_cursors {
            let root = cursor.carrier.0 as usize;
            let contract = &self.cursor_contracts[root];
            if contract.cover.is_empty() || self.places[model.roots[root]] & LIVE == 0 {
                continue;
            }
            let Some(current) = &self.cursor_views[root] else {
                continue;
            };
            let anchors = contract
                .cover
                .iter()
                .filter(|allowed| {
                    current
                        .anchors
                        .iter()
                        .any(|anchor| crate::flow_origins::contains(allowed, anchor))
                })
                .cloned()
                .collect();
            let mut lineages = current.lineages.clone();
            lineages.insert(root);
            self.cursor_views.set(
                root,
                Some(View {
                    anchors,
                    imprecise: true,
                    lineages,
                }),
            );
            // Forget exact identity rather than confusing an unknown descendant
            // with the parent at the cover boundary. Brand proofs must fail.
            for &part in &model.root_parts[root] {
                self.origins.identities[part].clear();
                self.origins.parts[part].clear();
            }
        }
    }
}

impl Checker<'_, '_> {
    fn cursor_view(&self, model: &Model, state: &State, place: PlaceId) -> View {
        let access = &self.body.places[place.0 as usize];
        if !state.cursor_views.is_empty()
            && let Some(mut view) = state.cursor_views[access.root.0 as usize].clone()
        {
            if !view.imprecise {
                for projection in &access.projection {
                    match projection {
                        Projection::Field(field) | Projection::VariantField { field, .. } => {
                            view.anchors = view
                                .anchors
                                .into_iter()
                                .map(|mut anchor| {
                                    anchor.fields.push(hirsel_types::FieldId(*field));
                                    anchor
                                })
                                .collect();
                        }
                        // All elements are descendants of the sequence anchor.
                        Projection::Index(_) => view.imprecise = true,
                    }
                }
            }
            return view;
        }
        if let Some(index) = access
            .projection
            .iter()
            .position(|p| matches!(p, Projection::Index(_)))
        {
            // Indexed values have no exact element identity. Their storage
            // anchors are descendants of the independently checked sequence
            // prefix; this observation grants no brand or ownership proof.
            let mut prefix = access.clone();
            prefix.projection.truncate(index);
            prefix.binding = None;
            return View {
                anchors: Self::storage_identity(model, state, &prefix),
                imprecise: true,
                lineages: BTreeSet::new(),
            };
        }
        View {
            anchors: self.origin_identity(model, state, place),
            imprecise: false,
            lineages: BTreeSet::new(),
        }
    }
    /// Snapshot before the administrative Take clears the source carrier.
    pub(super) fn cursor_assignment(
        &mut self,
        model: &Model,
        state: &mut State,
        destination: PlaceId,
        value: RvalueId,
        report: bool,
    ) -> Option<View> {
        if self.body.borrow_cursors.is_empty() {
            return None;
        }
        let target = &self.body.places[destination.0 as usize];
        let root = target.root.0 as usize;
        let rvalue = &self.body.rvalues[value.0 as usize];
        let mut source = match rvalue {
            Rvalue::Use(operand) if matches!(operand.usage, UseKind::Borrow | UseKind::Take) => {
                let access = &self.body.places[operand.place.0 as usize];
                let index = model.mapping[operand.place.0 as usize];
                (operand.usage == UseKind::Borrow
                    || (!state.must[index].has(MustFacts::OWNED)
                        && !state.ownership_conflicts[access.root.0 as usize]))
                    .then(|| self.cursor_view(model, state, operand.place))
            }
            _ => None,
        };
        if let Some(cursor) = self
            .body
            .borrow_cursors
            .iter()
            .find(|c| c.carrier == target.root)
            .cloned()
        {
            if !target.projection.is_empty() {
                self.flow_error(report, ViolationKind::InvalidUse(destination));
                return source;
            }
            if cursor.initializer == value {
                if !state.cursor_contracts[root].cover.is_empty() {
                    self.flow_error(report, ViolationKind::InvalidUse(destination));
                }
                let valid = matches!(rvalue, Rvalue::Use(operand) if operand.usage == UseKind::Borrow)
                    && self.body.locals[root].storage == Storage::Temporary
                    && needs_release(self, self.place_type(destination), self.t)
                    && copyable(self, self.place_type(destination), self.t)
                    && self.bound(
                        self.t,
                        self.place_type(destination),
                        hirsel_types::Bound::Data,
                        &self.generic_declarations(),
                    );
                let mut cover = BTreeSet::new();
                if let Some(places) = &cursor.cover {
                    for &place in places {
                        let access = &self.body.places[place.0 as usize];
                        let local = &self.body.locals[access.root.0 as usize];
                        let mut scope = Some(self.body.locals[root].scope);
                        let mut enclosing = false;
                        while let Some(id) = scope {
                            if id == local.scope {
                                enclosing = true;
                                break;
                            }
                            scope = self.body.scopes[id.0 as usize].parent;
                        }
                        if access.root.0 >= cursor.carrier.0
                            || !enclosing
                            || !model.binding_valid(state, access, self.body)
                        {
                            self.flow_error(report, ViolationKind::InvalidOrigin(place));
                        }
                        self.available(model, state, place, false, report);
                        self.projection_origin(model, state, place, report);
                        cover.extend(self.cursor_view(model, state, place).anchors);
                    }
                } else if let Some(view) = &source {
                    cover = view.anchors.clone();
                }
                if !valid || source.as_ref().is_none_or(|view| !view.within(&cover)) {
                    self.flow_error(report, ViolationKind::InvalidUse(destination));
                }
                let expired = state.origins.expired_by(&cover);
                let snapshot =
                    cursor.cover.is_none() && source.as_ref().is_some_and(|view| view.imprecise);
                state.cursor_contracts.set(
                    root,
                    Contract {
                        cover,
                        expired,
                        snapshot,
                    },
                );
                if let Some(source) = &mut source {
                    source.lineages.insert(root);
                }
            }
            let contract = &state.cursor_contracts[root];
            if source.as_ref().is_some_and(|view| {
                view.anchors.iter().any(|anchor| {
                    contract
                        .expired
                        .iter()
                        .any(|expired| crate::flow_origins::overlaps(expired, anchor))
                })
            }) || contract.cover.is_empty()
                || source.as_ref().is_none_or(|view| {
                    if contract.snapshot {
                        !view.lineages.contains(&root)
                    } else {
                        !view.within(&contract.cover)
                    }
                })
            {
                self.flow_error(report, ViolationKind::InvalidOrigin(destination));
            }
            return source;
        }
        // Borrowed staging/join temporaries retain their independently derived
        // referent anchors even when a later BindOrigin names the expression.
        // Without a cursor claim this function returns before touching flow.
        source
    }
}
