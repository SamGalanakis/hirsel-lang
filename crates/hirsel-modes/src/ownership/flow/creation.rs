//! Creation promises concern executable operands, not a receiving type's summary.
use super::*;
use hirsel_syntax::Argument;

pub(super) fn join(
    inputs: impl IntoIterator<Item = Option<BTreeSet<usize>>>,
) -> Option<BTreeSet<usize>> {
    let mut result = BTreeSet::new();
    for input in inputs {
        result.extend(input?);
    }
    Some(result)
}

/// Every selected incoming callback slot that may advance a selected incoming
/// state. Relaying the exact pair retains the caller's own future receipt.
fn incoming_relays(types: &TypeStore, function: &FunctionType) -> Vec<(usize, usize, usize)> {
    let Some(ErasedCaptures::FromArguments(retained)) =
        &types.get(function.result).unwrap().erased_captures
    else {
        return vec![];
    };
    let selected: Vec<usize> = retained
        .iter()
        .filter_map(|selector| match selector.parameter {
            ParameterRef::Bound {
                depth: 0,
                parameter,
            } if selector.fields.is_empty() => Some(parameter.0 as usize),
            _ => None,
        })
        .collect();
    let mut relays = vec![];
    for &callback in &selected {
        let TypeKind::Function(header) = &types.get(function.parameters[callback].ty).unwrap().kind
        else {
            continue;
        };
        for (slot, parameter) in header.parameters.iter().enumerate() {
            if parameter.passing != PassingMode::Mut {
                continue;
            }
            for &state in &selected {
                if state != callback {
                    relays.push((state, callback, slot));
                }
            }
        }
    }
    relays
}

pub(super) struct Effect {
    value: Option<(ExprId, Option<BTreeSet<usize>>)>,
    writes: Vec<(usize, Option<BTreeSet<usize>>)>,
}

impl Checker<'_> {
    // Mutable callback receipts constrain the future state, not just its
    // creation-time value. An abstract relay must retain that same incoming
    // callback/state pair; a freshly supplied callback needs a nested proof.
    fn creation_callback_contracts(
        &self,
        state: &State,
        callee: ExprId,
        arguments: &[Argument],
        requirements: &[Option<super::super::cfg::Requirement>],
    ) -> bool {
        let Some(callee_ty) = self.ty(callee) else {
            return false;
        };
        let TypeKind::Function(callee) = &self.body.types.get(callee_ty).unwrap().kind else {
            return false;
        };
        let Some((owner, own)) = self.graph_callable() else {
            return false;
        };
        let incoming = incoming_relays(
            if *own {
                &self.own.types
            } else {
                &self.body.types
            },
            owner,
        );
        let argument = |ordinal| {
            requirements
                .iter()
                .position(|r| r.as_ref().is_some_and(|r| r.parameter == ordinal))
                .map(|i| arguments[i].value)
        };
        let binding = |ordinal| {
            self.source_rows.binding(
                self.body,
                Some(self.graph.owner),
                &ResolvedName::Parameter(ParamId(ordinal as u32)),
            )
        };
        for (selected, callback, slot) in mutable_capture_states(&self.body.types, callee) {
            let (Some(selected), Some(callback)) = (argument(selected), argument(callback)) else {
                return false;
            };
            let (Some(selected_ty), Some(callback_ty)) = (self.ty(selected), self.ty(callback))
            else {
                return false;
            };
            let receipt = mutable_capture_state_sendability(
                &self.body.types,
                callback_ty,
                slot,
                &self.body.resolution.generic_declarations,
                &mut |path| Ok(self.signatures.cached(path)),
            )
            .unwrap_or_else(|error| {
                *self.proof_error.borrow_mut() = Some(error);
                Sendability::NotSendable
            });
            if receipt == Sendability::Sendable && self.inert_creation_input(selected_ty) {
                continue;
            }
            // A callback promising to preserve its whole state keeps the
            // snapshot on every future invocation; a closed state type
            // determines everything a callback can hand back.
            let TypeKind::Function(header) = &self.body.types.get(callback_ty).unwrap().kind else {
                return false;
            };
            if header.state_promises.iter().any(|p| {
                p.selector == RelationPlace::from(ParameterRef::bound(ParamId(slot as u32)))
                    && p.preserve_captures
            }) || receipt == Sendability::Sendable
                && header
                    .parameters
                    .get(slot)
                    .is_some_and(|p| self.closed_content(p.ty, true))
            {
                continue;
            }
            let selected_inputs = self.value(state, selected).envelope;
            let callback_inputs = self.value(state, callback).envelope;
            if !incoming.iter().any(|&(selected, callback, incoming_slot)| {
                let (Some(selected), Some(callback)) = (binding(selected), binding(callback))
                else {
                    return false;
                };
                incoming_slot == slot
                    && match (
                        &self.body.types.get(selected_ty).unwrap().kind,
                        &self
                            .body
                            .types
                            .get(self.body.bindings[selected].ty)
                            .unwrap()
                            .kind,
                    ) {
                        (TypeKind::Parameter(relayed), TypeKind::Parameter(incoming)) => {
                            relayed == incoming
                        }
                        _ => semantic_equal(
                            &self.body.types,
                            selected_ty,
                            &self.body.types,
                            self.body.bindings[selected].ty,
                        )
                        .unwrap_or(false),
                    }
                    && selected_inputs
                        .as_ref()
                        .is_some_and(|inputs| inputs.contains(&selected))
                    && callback_inputs
                        .as_ref()
                        .is_some_and(|inputs| inputs.contains(&callback))
                    && semantic_equal(
                        &self.body.types,
                        callback_ty,
                        &self.body.types,
                        self.body.bindings[callback].ty,
                    )
                    .unwrap_or(false)
            }) {
                return false;
            }
        }
        true
    }
    pub(super) fn stable_creation_copy(&self, ty: TypeId) -> bool {
        self.properties[ty.index()].plain
            || self.inert_creation_input(ty) && self.properties[ty.index()].sendable_aliased
    }

    fn invalidate_creation(&self, state: &mut State, binding: usize) {
        let inputs = state.slots[binding].value.envelope.clone();
        state.slots[binding].value.envelope = None;
        let Some(inputs) = inputs.filter(|inputs| !inputs.is_empty()) else {
            return;
        };
        for index in 0..state.slots.len() {
            if state.slots[index]
                .value
                .envelope
                .as_ref()
                .is_some_and(|other| !other.is_disjoint(&inputs))
            {
                state.slots[index].value.envelope = None;
            }
        }
        for value in state.values.values_mut() {
            if value
                .envelope
                .as_ref()
                .is_some_and(|other| !other.is_disjoint(&inputs))
            {
                value.envelope = None;
            }
        }
    }
    fn transparent_creation(&self, expression: ExprId) -> bool {
        let Some(call) = self
            .source_rows
            .call(expression)
            .map(|i| &self.body.calls[i])
        else {
            return false;
        };
        let path = match &call.target {
            CallTarget::Constructor(ContextualName::Constructor(path)) => path,
            CallTarget::Constructor(ContextualName::Variant { owner, .. }) => owner,
            _ => return false,
        };
        let Some(signature) = self.signatures.cached(path) else {
            return false;
        };
        let SignatureKind::Type(declaration) = &signature.kind else {
            return false;
        };
        // A plain nominal's whole-value authority, cleanup and brands are its
        // parts'; only a seal, finalizer or fact adds an independent effect.
        !declaration.resource && declaration.capability.is_none() && declaration.fact.is_none()
    }
    /// The `@captures` selection of the callable this graph implements.
    pub(super) fn creation_selectors(&self) -> Option<Vec<RelationPlace>> {
        let (function, own) = self.graph_callable()?;
        let types = if *own {
            &self.own.types
        } else {
            &self.body.types
        };
        match &types.get(function.result)?.erased_captures {
            Some(ErasedCaptures::FromArguments(selectors)) => Some(selectors.to_vec()),
            _ => None,
        }
    }

    pub(super) fn inert_creation_input(&self, ty: TypeId) -> bool {
        let row = self.body.types.get(ty).unwrap();
        if row.drop_allocation != DropAllocation::NoAlloc
            || row.modes.multiplicity != Multiplicity::Many
            || row.modes.sendability != Sendability::Sendable
                && !self.properties[ty.index()].sendable_aliased
        {
            return false;
        }
        let mut pending = vec![ty];
        let mut seen = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.body.types.get(ty).unwrap();
            if row.brand != Brand::default()
                || !row.modes.relation.is_global()
                || row.modes.multiplicity != Multiplicity::Many
                || row.drop_allocation != DropAllocation::NoAlloc
            {
                return false;
            }
            pending.extend(row.kind.value_parts());
        }
        structural_authority(
            &self.body.types,
            ty,
            &self.own.metadata.roles,
            &self.body.resolution.generic_declarations,
            &mut |_, path| {
                Ok::<_, QueryError>(path.and_then(|path| {
                    if path == &self.own.definition {
                        Some(Arc::new(self.own.clone()))
                    } else {
                        self.signatures.cached(path)
                    }
                }))
            },
        )
        .map(|authority| authority == AuthoritySummary::inert())
        .unwrap_or_else(|error| {
            *self.proof_error.borrow_mut() = Some(error);
            false
        })
    }

    pub(super) fn creation_effect(&self, op: &Op, state: &State) -> Option<Effect> {
        self.creation_selectors()?;
        let mut effect = Effect {
            value: None,
            writes: vec![],
        };
        let expression_value = |expression| self.value(state, expression).envelope;
        let fresh = |expression| {
            self.ty(expression)
                .filter(|ty| self.inert_creation_input(*ty))
                .map(|_| BTreeSet::new())
        };
        effect.value = match op {
            Op::Finish {
                expression,
                children,
                ..
            } => Some((
                *expression,
                if children.is_empty() {
                    fresh(*expression)
                } else {
                    join(children.iter().map(|e| expression_value(*e)))
                },
            )),
            Op::Call {
                expression,
                callee,
                arguments,
                captures,
                constructor,
                requirements,
                ..
            } => {
                let envelope = if *constructor && self.transparent_creation(*expression) {
                    join(arguments.iter().map(|a| expression_value(a.value)))
                } else if let Some(captures) = captures.as_ref().filter(|_| {
                    self.creation_callback_contracts(state, *callee, arguments, requirements)
                }) {
                    join(
                        captures
                            .iter()
                            .map(|i| expression_value(arguments[*i].value)),
                    )
                } else {
                    None
                };
                // A mutable call can install fresh content. An unchanged receiving
                // row cannot keep the old symbolic provenance alive.
                for (argument, requirement) in arguments.iter().zip(requirements) {
                    let Some(requirement) = requirement else {
                        continue;
                    };
                    if requirement.passing == PassingMode::Take
                        || requirement.passing == PassingMode::Mut
                            && self.callee_preserves_state(*callee, requirement.parameter, false)
                        || requirement.passing == PassingMode::Borrow
                            && self
                                .ty(argument.value)
                                .is_some_and(|ty| self.stable_creation_copy(ty))
                    {
                        continue;
                    }
                    if let Some(place) = place(self.body, argument.value, self.source_rows) {
                        let mut pending = vec![place.binding];
                        let mut seen = BTreeSet::new();
                        while let Some(binding) = pending.pop() {
                            if seen.insert(binding) {
                                pending.extend(state.slots[binding].value.roots());
                                effect.writes.push((binding, None));
                            }
                        }
                    }
                }
                let callable = self.value(state, *callee);
                for binding in callable.captures().chain(callable.roots()) {
                    effect.writes.push((binding, None));
                }
                Some((*expression, envelope))
            }
            Op::Capture { expression, .. } => {
                let captures = self
                    .source_rows
                    .closure(*expression)
                    .map(|i| &self.body.closures[i]);
                let envelope = captures.and_then(|captures| {
                    join(
                        captures
                            .captures
                            .iter()
                            .enumerate()
                            .map(|(ordinal, capture)| {
                                let owner = self
                                    .graph
                                    .capture_modes
                                    .get(*expression, ordinal as u32)
                                    .map_or(self.graph.owner, |mode| mode.owner);
                                let owner = (!matches!(capture.binding, ResolvedName::Local(_)))
                                    .then_some(owner);
                                self.source_rows
                                    .binding(self.body, owner, &capture.binding)
                                    .and_then(|binding| {
                                        let usage = self
                                            .graph
                                            .capture_modes
                                            .get(*expression, ordinal as u32)
                                            .map_or(capture.usage, |mode| mode.usage);
                                        let ty = self.body.bindings[binding].ty;
                                        if usage == UseKind::Copy && !self.stable_creation_copy(ty)
                                        {
                                            None
                                        } else {
                                            state.slots[binding].value.envelope.clone()
                                        }
                                    })
                            }),
                    )
                });
                Some((*expression, envelope))
            }
            Op::Write {
                place: expression,
                value,
                ..
            } => {
                if let Some(place) = place(self.body, *expression, self.source_rows) {
                    let incoming = expression_value(*value);
                    let envelope = if place.projections.is_empty() {
                        incoming
                    } else {
                        join([state.slots[place.binding].value.envelope.clone(), incoming])
                    };
                    effect.writes.push((place.binding, envelope));
                }
                None
            }
            _ => None,
        };
        if matches!(
            op,
            Op::Finish { .. }
                | Op::Call {
                    constructor: true,
                    ..
                }
        ) && let Some((expression, envelope)) = &mut effect.value
            && envelope.as_ref().is_some_and(BTreeSet::is_empty)
            && !self
                .ty(*expression)
                .is_some_and(|ty| self.stable_creation_copy(ty))
        {
            *envelope = None;
        }
        Some(effect)
    }

    pub(super) fn install_creation_effect(&self, effect: Option<Effect>, state: &mut State) {
        let Some(effect) = effect else { return };
        for (binding, envelope) in effect.writes {
            if envelope.is_none() {
                self.invalidate_creation(state, binding);
            } else {
                state.slots[binding].value.envelope = envelope;
            }
        }
        if let Some((expression, envelope)) = effect.value
            && let Some(value) = state.values.get_mut(&expression)
        {
            value.envelope = envelope;
        }
    }

    pub(super) fn creation_shared(&self, expression: ExprId, state: &mut State) {
        if self.creation_selectors().is_none() {
            return;
        }
        let value = self.value(state, expression);
        if value.usage != UseKind::Copy
            || self
                .ty(expression)
                .is_some_and(|ty| self.stable_creation_copy(ty))
            || self
                .property(expression)
                .is_some_and(|property| property.plain)
        {
            return;
        }
        // An exact move transports the symbolic envelope. A general copy can
        // lose crossing evidence; until that relative proof exists, neither
        // the copy nor aliases of the original may claim the entry envelope.
        if let Some(value) = state.values.get_mut(&expression) {
            value.envelope = None;
        }
        let mut pending: Vec<_> = place(self.body, expression, self.source_rows)
            .map(|p| p.binding)
            .into_iter()
            .collect();
        let mut seen = BTreeSet::new();
        while let Some(binding) = pending.pop() {
            if seen.insert(binding) {
                pending.extend(state.slots[binding].value.roots());
                self.invalidate_creation(state, binding);
            }
        }
    }

    pub(super) fn creation_return(&mut self, state: &State, expression: ExprId) {
        let Some(selectors) = self.creation_selectors() else {
            return;
        };
        let allowed: BTreeSet<_> = self
            .body
            .bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                let ResolvedName::Parameter(parameter) = binding.binding else {
                    return None;
                };
                (binding.owner == self.graph.owner
                    && (self.inert_creation_input(binding.ty)
                        || selectors.iter().any(|selector| {
                            selector.fields.is_empty()
                                && selector.parameter == ParameterRef::bound(parameter)
                        })))
                .then_some(index)
            })
            .collect();
        let value = self.value(state, expression);
        if value.invalid {
            return;
        }
        if !value
            .envelope
            .is_some_and(|inputs| inputs.is_subset(&allowed))
        {
            let site = self.span_of(value.site.unwrap_or(expression));
            self.report(
                const { RuleId::of("CLO-4") },
                site,
                "the created environment retains an unselected or unproved input envelope",
                |_| {
                    Explanation::default()
                        .provided(Fact::new(Cause::CreationInput, site, &[]))
                        .required(Fact::new(
                            Requirement::CreationEnvelope,
                            site,
                            &["the declared capture inputs"],
                        ))
                        .into()
                },
            );
        }
    }
}
