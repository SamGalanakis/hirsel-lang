//! Selected mutable state keeps its symbolic incoming snapshot: an effect
//! discipline over binder-exact, field-sensitive selections.
use super::*;
use hirsel_syntax::StmtKind;

/// A promise of the callable this graph proves, on one of its own bindings.
#[derive(Clone, Debug)]
pub(super) struct Selection {
    binding: usize,
    fields: Vec<u32>,
    advance: bool,
}

/// Where an operated place lies relative to a selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Meet {
    /// The place is the selection, or contains it (or reaches it through an
    /// alias or a non-field step).
    Covering,
    /// The place lies strictly inside the selection.
    Inside,
}

/// The relative position of place fields `q` and selection fields `p`.
fn meet(q: &[Projection], p: &[u32]) -> Option<Meet> {
    for (i, step) in q.iter().enumerate() {
        let Some(&selected) = p.get(i) else {
            return Some(Meet::Inside);
        };
        match step {
            Projection::Field(field) if *field == selected => {}
            Projection::Field(_) => return None,
            _ => return Some(Meet::Covering),
        }
    }
    Some(Meet::Covering)
}

impl Checker<'_, '_> {
    /// The callable type this graph's body implements.
    fn graph_function(&self) -> Option<&FunctionType> {
        self.graph_callable().map(|(function, _)| function)
    }
    /// The callable type this graph's body implements, and whether its rows
    /// live in the item's own signature store rather than the body's.
    pub(super) fn graph_callable(&self) -> Option<&(FunctionType, bool)> {
        self.callable
            .get_or_init(|| self.implemented_callable())
            .as_ref()
    }
    fn implemented_callable(&self) -> Option<(FunctionType, bool)> {
        let ty = match self.graph.owner {
            NameSite::Item if self.graph.clause.is_none() => {
                return match &self.own.kind {
                    SignatureKind::Function(f) => Some((f.clone(), true)),
                    _ => None,
                };
            }
            NameSite::Expression(e) => self.ty(e)?,
            NameSite::Statement(s) => {
                if !matches!(
                    self.tree.statements[s.0 as usize].kind,
                    StmtKind::Function { .. }
                ) {
                    return None;
                }
                self.body
                    .bindings
                    .iter()
                    .find(|b| b.site == NameSite::Statement(s))?
                    .ty
            }
            _ => return None,
        };
        match &self.t.get(ty)?.kind {
            TypeKind::Function(f) => Some((f.clone(), false)),
            _ => None,
        }
    }

    pub(super) fn selections(&self) -> Vec<Selection> {
        let Some(function) = self.graph_function() else {
            return vec![];
        };
        function
            .state_promises
            .iter()
            .filter(|p| p.preserve_captures)
            .filter_map(|promise| {
                let ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } = promise.selector.parameter
                else {
                    return None;
                };
                let binding = self.source_rows.binding(
                    self.body,
                    Some(self.graph.owner),
                    &ResolvedName::Parameter(parameter),
                )?;
                Some(Selection {
                    binding,
                    fields: promise.selector.fields.iter().map(|f| f.0).collect(),
                    advance: promise.advance_only,
                })
            })
            .collect()
    }

    /// Each selection the place meets, directly or through an alias of its
    /// binding. An alias reaches the whole selected binding.
    fn met(&self, state: &State, place: &Place) -> Vec<(Selection, Meet, Vec<u32>)> {
        let selections = self.selections();
        if selections.is_empty() {
            return vec![];
        }
        let mut met = vec![];
        for selection in &selections {
            if selection.binding == place.binding {
                if let Some(relation) = meet(&place.projections, &selection.fields) {
                    let fields = place
                        .projections
                        .iter()
                        .map_while(|p| match p {
                            Projection::Field(f) => Some(*f),
                            _ => None,
                        })
                        .collect();
                    met.push((selection.clone(), relation, fields));
                }
                continue;
            }
            // Joined views may name several sources and therefore have no
            // single alias. Any alternative reaching the selected state is
            // still subject to its obligation.
            let mut pending = state.slots[place.binding].value.roots();
            let mut seen = BTreeSet::from([place.binding]);
            while let Some(index) = pending.pop() {
                if !seen.insert(index) {
                    continue;
                }
                if index == selection.binding {
                    met.push((selection.clone(), Meet::Covering, vec![]));
                    break;
                }
                pending.extend(state.slots[index].value.roots());
            }
        }
        met
    }

    /// Every value of a closed type carries the same type-determined source
    /// bounds, brands, authority, multiplicity and cleanup.
    pub(super) fn closed_type(&self, ty: TypeId) -> bool {
        self.closed_content(ty, false)
    }
    /// A `mut` parameter's root row is the call-bounded view of its content.
    pub(super) fn closed_content(&self, root: TypeId, view: bool) -> bool {
        let mut pending = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.get(ty).unwrap();
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
                    kind: BuiltinType::Iter,
                    ..
                } => return false,
                TypeKind::Named { .. } => {
                    if !self.properties[ty].plain {
                        return false;
                    }
                }
                kind => pending.extend(kind.value_parts()),
            }
        }
        structural_authority(
            self.t,
            root,
            &self.own.metadata.roles,
            &self.body.resolution.generic_declarations,
            &mut |_, d| Ok::<_, QueryError>(d.and_then(|d| self.signatures.cached(d))),
        )
        .map(|authority| authority == AuthoritySummary::inert())
        .unwrap_or_else(|error| {
            *self.proof_error.borrow_mut() = Some(error);
            false
        })
    }

    /// Closed content strictly inside a selection, or inside a preservation
    /// selection, keeps the snapshot when overwritten (bookkeeping).
    fn closed_effect(&self, selection: &Selection, relation: Meet, ty: Option<TypeId>) -> bool {
        (relation == Meet::Inside || !selection.advance)
            && ty.is_some_and(|ty| self.closed_type(ty))
    }

    fn fixed_inert_content(&self, root: TypeId) -> bool {
        let mut pending = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.get(ty).unwrap();
            if row.erased_captures.is_some()
                || row.brand != Brand::default()
                || ty != root && !row.modes.relation.is_global()
                || row.modes.multiplicity != Multiplicity::Many
                || row.drop_allocation != DropAllocation::NoAlloc
            {
                return false;
            }
            match &row.kind {
                TypeKind::Parameter(_)
                | TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                } => return false,
                TypeKind::Named { .. } if !self.properties[ty].copyable => return false,
                kind => pending.extend(kind.value_parts()),
            }
        }
        structural_authority(
            self.t,
            root,
            &self.own.metadata.roles,
            &self.body.resolution.generic_declarations,
            &mut |_, d| Ok::<_, QueryError>(d.and_then(|d| self.signatures.cached(d))),
        )
        .map(|authority| authority == AuthoritySummary::inert())
        .unwrap_or_else(|error| {
            *self.proof_error.borrow_mut() = Some(error);
            false
        })
    }

    // A counted field is read through its owner's local view. That view is
    // not the field's stored locality: an independently global, owned inert
    // replacement introduces no new environment input. Keep this separate from
    // closed_effect: an arbitrary mutable callback can still share or replace
    // the field and therefore needs its own preservation promise.
    fn owned_data_replacement(
        &self,
        state: &State,
        selection: &Selection,
        relation: Meet,
        target: Option<TypeId>,
        incoming: ExprId,
    ) -> bool {
        if selection.advance || relation != Meet::Inside {
            return false;
        }
        let Some(target) = target else { return false };
        if !self.fixed_inert_content(target) {
            return false;
        }
        let value = self.value(state, incoming);
        self.arenas.global(&value.modes.relation)
            && (value.modes.uniqueness == Uniqueness::Unique
                || self.captured_sendability(incoming, &value) == Sendability::Sendable)
    }

    fn callee_covers(
        &self,
        callee: ExprId,
        ordinal: usize,
        selection: &Selection,
        fields: &[u32],
    ) -> bool {
        self.ty(callee)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| {
                let TypeKind::Function(function) = &row.kind else {
                    return false;
                };
                function.state_promises.iter().any(|p| {
                    let promised: Vec<u32> = fields
                        .iter()
                        .copied()
                        .chain(p.selector.fields.iter().map(|f| f.0))
                        .collect();
                    p.selector.parameter == ParameterRef::bound(ParamId(ordinal as u32))
                        && p.preserve_captures
                        && (!selection.advance || p.advance_only)
                        && (p.selector.fields.is_empty() || selection.fields.starts_with(&promised))
                })
            })
    }

    fn state_violation(&mut self, binding: usize, advance: bool, site: Span) {
        self.report(if advance { const { RuleId::of("MEM-14") } } else { const { RuleId::of("CLO-4") } }, site,
            "the selected incoming state must survive unchanged except for declared preserving advances", |checker| {
                let (name, declaration) = checker.binding_site(binding);
                Explanation::default()
                    .provided(Fact::new(Cause::MutParameter, declaration, &[&name]))
                    .required(Fact::new(Requirement::StatePreservation, declaration, &[&name]))
                    .into()
            });
    }

    pub(super) fn check_state_operation(&mut self, op: &Op, state: &State) {
        if !self.emit {
            return;
        }
        match op {
            Op::Write {
                place: expression,
                operation,
                value,
                ..
            } => {
                let Some(place) = place(self.body, *expression, self.source_rows) else {
                    return;
                };
                let ty = self.ty(*expression);
                for (selection, relation, _) in self.met(state, &place) {
                    if !self.closed_effect(&selection, relation, ty)
                        && !self.owned_data_replacement(state, &selection, relation, ty, *value)
                    {
                        self.state_violation(selection.binding, selection.advance, *operation);
                    }
                }
            }
            Op::Use {
                expression,
                context,
                operation,
            } => {
                if !matches!(
                    context,
                    Context::Own
                        | Context::Store
                        | Context::Copy
                        | Context::Take
                        | Context::Unique
                        | Context::ResultShare
                        | Context::Invoke
                ) {
                    return;
                }
                let Some(place) = place(self.body, *expression, self.source_rows) else {
                    return;
                };
                let ty = self.ty(*expression);
                for (selection, relation, _) in self.met(state, &place) {
                    let admitted = match context {
                        // A stable copy shares no ownership or crossing evidence.
                        Context::Copy => ty.is_some_and(|ty| self.stable_creation_copy(ty)),
                        // Invoking a many-use callable borrows it.
                        Context::Invoke => ty.is_some_and(|ty| {
                            self.t.get(ty).unwrap().modes.multiplicity == Multiplicity::Many
                        }),
                        _ => self.closed_effect(&selection, relation, ty),
                    };
                    if !admitted {
                        self.state_violation(
                            selection.binding,
                            selection.advance,
                            self.span_of(*operation),
                        );
                    }
                }
            }
            Op::Call {
                callee,
                arguments,
                requirements,
                constructor: false,
                expression,
                ..
            } => {
                for (argument, requirement) in arguments.iter().zip(requirements) {
                    let Some(requirement) = requirement else {
                        continue;
                    };
                    if requirement.passing != PassingMode::Mut {
                        continue;
                    }
                    let Some(place) = place(self.body, argument.value, self.source_rows) else {
                        continue;
                    };
                    let ty = self.ty(argument.value);
                    for (selection, relation, fields) in self.met(state, &place) {
                        let inert_advance = !selection.advance
                            && relation == Meet::Inside
                            && ty.is_some_and(|ty| self.fixed_inert_content(ty))
                            && self.callee_preserves_unique(*callee, requirement.parameter)
                            && arguments.iter().zip(requirements).all(|(arg, req)| {
                                req.as_ref().is_none_or(|_| {
                                    arg.value == argument.value
                                        || self.property(arg.value).is_some_and(|p| p.plain)
                                        || {
                                            let value = self.value(state, arg.value);
                                            self.arenas.global(&value.modes.relation)
                                                && (value.modes.uniqueness == Uniqueness::Unique
                                                    || self.captured_sendability(arg.value, &value)
                                                        == Sendability::Sendable)
                                        }
                                })
                            });
                        if !self.closed_effect(&selection, relation, ty)
                            && !inert_advance
                            && !self.callee_covers(
                                *callee,
                                requirement.parameter,
                                &selection,
                                &fields,
                            )
                        {
                            self.state_violation(
                                selection.binding,
                                selection.advance,
                                self.span_of(*expression),
                            );
                        }
                    }
                }
            }
            Op::Capture { expression, .. } => self.check_state_capture(state, *expression),
            Op::LocalFunction(statement) => {
                if let StmtKind::Function { body, .. } =
                    self.tree.statements[statement.0 as usize].kind
                {
                    self.check_state_capture(state, body);
                }
            }
            _ => {}
        }
    }

    fn check_state_capture(&mut self, state: &State, expression: ExprId) {
        let Some(index) = self.source_rows.closure(expression) else {
            return;
        };
        let captures = self.body.closures[index].captures.clone();
        for (ordinal, capture) in captures.iter().enumerate() {
            let Some(mode) = self.graph.capture_modes.get(expression, ordinal as u32) else {
                continue;
            };
            // A scoped readonly view owns and shares no state. Its nested
            // body is independently prohibited from mutating the view.
            if mode.usage == UseKind::Borrow {
                continue;
            }
            let Some(binding) =
                self.source_rows
                    .binding(self.body, Some(mode.owner), &capture.binding)
            else {
                continue;
            };
            let whole = Place {
                binding,
                projections: vec![],
            };
            for (selection, _, _) in self.met(state, &whole) {
                self.state_violation(
                    selection.binding,
                    selection.advance,
                    self.span_of(expression),
                );
            }
        }
    }
}
