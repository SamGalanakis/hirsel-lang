//! Independent executable provenance for whole-input creation envelopes.
use super::*;
use hirsel_types::{
    AuthoritySummary, Brand, DropAllocation, ErasedCaptures, Multiplicity, RelationPlace,
    SendabilityRequirement,
};

pub(super) fn join(
    inputs: impl IntoIterator<Item = Option<BTreeSet<usize>>>,
) -> Option<BTreeSet<usize>> {
    let mut retained = BTreeSet::new();
    for input in inputs {
        retained.extend(input?);
    }
    Some(retained)
}

/// Each selected incoming callback's mutable slot paired with every other
/// selected incoming state: relaying the exact pair keeps the caller's receipt.
fn relays(
    types: &TypeInterner,
    function: &hirsel_types::FunctionType,
) -> Vec<(usize, usize, usize)> {
    let Some(ErasedCaptures::FromArguments(selectors)) =
        &types.get(function.result).unwrap().erased_captures
    else {
        return vec![];
    };
    let whole: Vec<usize> = selectors
        .iter()
        .filter(|selector| selector.fields.is_empty())
        .filter_map(|selector| match selector.parameter {
            ParameterRef::Bound {
                depth: 0,
                parameter,
            } => Some(parameter.0 as usize),
            _ => None,
        })
        .collect();
    let mut pairs = vec![];
    for &callback in &whole {
        let Some(TypeKind::Function(header)) = function
            .parameters
            .get(callback)
            .and_then(|p| types.get(p.ty))
            .map(|row| &row.kind)
        else {
            continue;
        };
        for (slot, parameter) in header.parameters.iter().enumerate() {
            if parameter.passing == PassingMode::Mut {
                pairs.extend(
                    whole
                        .iter()
                        .filter(|state| **state != callback)
                        .map(|state| (*state, callback, slot)),
                );
            }
        }
    }
    pairs
}

/// The relayed value has the incoming parameter's type: the same abstract
/// parameter, or an equal type. A written `sendable` on either is a receiving
/// requirement, not part of the value: a move carries the caller's receipt as
/// evidence (MODE-4), so the root requirements are set aside.
fn relayed(types: &TypeInterner, relayed: TypeId, incoming: TypeId) -> bool {
    let (mut a, mut b) = (
        types.get(relayed).unwrap().clone(),
        types.get(incoming).unwrap().clone(),
    );
    if let (TypeKind::Parameter(a), TypeKind::Parameter(b)) = (&a.kind, &b.kind) {
        return a == b;
    }
    a.sendability_requirement = SendabilityRequirement::None;
    b.sendability_requirement = SendabilityRequirement::None;
    let (a, b) = (types.intern(a), types.intern(b));
    types.semantic_eq(a, b)
}

impl Checker<'_, '_> {
    /// The callable type the validated body implements, with its row store:
    /// the published header for the entry, the creating row for a closure.
    pub(super) fn body_callable(&self) -> Option<(&TypeInterner, &hirsel_types::FunctionType)> {
        if self.site.body == self.definition.entry {
            let signature = self.signatures.get(self.definition.path)?;
            match &signature.kind {
                SignatureKind::Function(f) => Some((self.t, f)),
                _ => None,
            }
        } else {
            let ty = self
                .scratch
                .definition_inputs(self.t, self.definition)
                .producers[self.site.body.0 as usize]
                .first_function?;
            match &self.t.get(ty)?.kind {
                TypeKind::Function(f) => Some((self.t, &f)),
                _ => None,
            }
        }
    }
    fn creation_callback_contracts(
        &self,
        state: &State,
        types: &TypeInterner,
        callee: &hirsel_types::FunctionType,
        arguments: &[CallArgument],
    ) -> bool {
        let Some((owner_types, owner)) = self.body_callable() else {
            return false;
        };
        let incoming = relays(owner_types, owner);
        for (selected, callback, slot) in hirsel_types::mutable_capture_states(types, callee) {
            let argument = |ordinal| {
                arguments
                    .iter()
                    .find(|a| a.parameter.0 as usize == ordinal)
                    .map(|a| &a.value)
            };
            let (Some(selected), Some(callback)) = (argument(selected), argument(callback)) else {
                return false;
            };
            let selected_ty = self.place_type(selected.place);
            let receipt = hirsel_types::mutable_capture_state_sendability(
                self.t,
                self.place_type(callback.place),
                slot,
                self.generic_declarations(),
                &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
            )
            .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
            if receipt == Sendability::Sendable && self.inert_creation(selected_ty) {
                continue;
            }
            // Every invocation of a callback promising its whole state keeps
            // the snapshot; a closed slot fixes what any callback hands back.
            let TypeKind::Function(header) =
                &self.t.get(self.place_type(callback.place)).unwrap().kind
            else {
                return false;
            };
            let whole =
                RelationPlace::from(ParameterRef::bound(hirsel_types::ParamId(slot as u32)));
            if header
                .state_promises
                .iter()
                .any(|p| p.selector == whole && p.preserve_captures)
                || receipt == Sendability::Sendable
                    && header
                        .parameters
                        .get(slot)
                        .is_some_and(|p| self.closed(self.t, p.ty, true))
            {
                continue;
            }
            let selected_inputs = self.creation_operand(state, selected);
            let callback_inputs = self.creation_operand(state, callback);
            if !incoming
                .iter()
                .any(|&(selected, incoming_callback, incoming_slot)| {
                    let (Some(selected), Some(incoming_callback)) = (
                        self.body.parameters.get(selected),
                        self.body.parameters.get(incoming_callback),
                    ) else {
                        return false;
                    };
                    let store = self.t;
                    incoming_slot == slot
                        && relayed(store, selected_ty, self.body.locals[selected.0 as usize].ty)
                        && selected_inputs
                            .as_ref()
                            .is_some_and(|inputs| inputs.contains(&(selected.0 as usize)))
                        && callback_inputs
                            .as_ref()
                            .is_some_and(|inputs| inputs.contains(&(incoming_callback.0 as usize)))
                        && relayed(
                            store,
                            self.place_type(callback.place),
                            self.body.locals[incoming_callback.0 as usize].ty,
                        )
                })
            {
                return false;
            }
        }
        true
    }
    fn transparent_creation(&self, constructor: &Constructor) -> bool {
        let path = match constructor {
            Constructor::Record(path)
            | Constructor::Variant {
                definition: path, ..
            } => path,
            _ => return true,
        };
        let Some(signature) = self.signatures.get(path) else {
            return false;
        };
        let SignatureKind::Type(declaration) = &signature.kind else {
            return false;
        };
        // Only a seal, finalizer or fact adds an effect beyond the parts.
        !declaration.resource && declaration.capability.is_none() && declaration.fact.is_none()
    }

    fn inert_creation(&self, ty: TypeId) -> bool {
        let types = self.t;
        let row = types.get(ty).unwrap();
        if row.drop_allocation != DropAllocation::NoAlloc
            || row.modes.multiplicity != Multiplicity::Many
            || self
                .proof(
                    types,
                    ty,
                    super::proofs::Proof::Content,
                    self.generic_declarations(),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
                != Sendability::Sendable
            || self.authority_with_generics(types, ty, self.generic_declarations())
                != AuthoritySummary::inert()
        {
            return false;
        }
        let mut pending = vec![ty];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = types.get(ty).unwrap();
            if row.brand != Brand::default()
                || !row.modes.relation.is_global()
                || row.modes.multiplicity != Multiplicity::Many
                || row.drop_allocation != DropAllocation::NoAlloc
            {
                return false;
            }
            pending.extend(row.kind.value_parts());
        }
        true
    }

    pub(super) fn initial_creation(&self, model: &Model, state: &mut State) {
        if !model.creation {
            return;
        }
        for local in &self.body.parameters {
            let root = local.0 as usize;
            state.envelopes[root] = Some(BTreeSet::from([root]));
        }
    }

    fn creation_operand(&self, state: &State, operand: &Operand) -> Option<BTreeSet<usize>> {
        if operand.usage == UseKind::Copy && !self.copy_creation(operand.place) {
            return None;
        }
        state.envelopes[self.body.places[operand.place.0 as usize].root.0 as usize].clone()
    }

    pub(super) fn copy_creation(&self, place: PlaceId) -> bool {
        self.inert_creation(self.place_type(place))
            && self
                .proof(
                    self.t,
                    self.place_type(place),
                    super::proofs::Proof::AfterSharing,
                    &self.generic_declarations(),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
                == Sendability::Sendable
            || self.bound(
                self.t,
                self.place_type(place),
                hirsel_syntax::Bound::Plain,
                &self.generic_declarations(),
            )
    }

    pub(super) fn share_creation(&self, state: &mut State, operand: &Operand) {
        if !self.copy_creation(operand.place) {
            self.invalidate_creation(state, operand.place);
        }
    }

    fn invalidate_creation(&self, state: &mut State, place: PlaceId) {
        let root = self.body.places[place.0 as usize].root.0 as usize;
        let inputs = state.envelopes[root].clone();
        state.envelopes[root] = None;
        let Some(inputs) = inputs.filter(|inputs| !inputs.is_empty()) else {
            return;
        };
        for index in 0..state.envelopes.len() {
            if state.envelopes[index]
                .as_ref()
                .is_some_and(|other| !other.is_disjoint(&inputs))
            {
                state.envelopes[index] = None;
            }
        }
    }

    fn store_creation(&self, state: &mut State, place: PlaceId, inputs: Option<BTreeSet<usize>>) {
        let place = &self.body.places[place.0 as usize];
        let root = place.root.0 as usize;
        state.envelopes[root] = if place.projection.is_empty() {
            inputs
        } else {
            // Until selected-field snapshots land, retain the whole owner's
            // envelope. A projected write cannot erase a sibling dependency.
            join([state.envelopes[root].clone(), inputs])
        };
    }

    pub(super) fn assign_creation(
        &self,
        model: &Model,
        state: &mut State,
        destination: PlaceId,
        value: &Rvalue,
    ) {
        if !model.creation {
            return;
        }
        let mut inputs = match value {
            Rvalue::Use(operand) => self.creation_operand(state, operand),
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } if self.transparent_creation(constructor) => join(
                fields
                    .iter()
                    .map(|field| self.creation_operand(state, &field.value)),
            ),
            Rvalue::Closure { captures, .. } => join(
                captures
                    .iter()
                    .map(|operand| self.creation_operand(state, operand)),
            ),
            Rvalue::Constant(_)
            | Rvalue::ReadConstant { .. }
            | Rvalue::FunctionReference { .. }
                if self.inert_creation(self.place_type(destination)) =>
            {
                Some(BTreeSet::new())
            }
            _ => None,
        };
        if matches!(value, Rvalue::Construct { .. })
            && inputs.as_ref().is_some_and(BTreeSet::is_empty)
            && !self.copy_creation(destination)
        {
            inputs = None;
        }
        self.store_creation(state, destination, inputs);
    }

    pub(super) fn replace_creation(
        &self,
        model: &Model,
        state: &mut State,
        destination: PlaceId,
        value: &Operand,
        displaced: PlaceId,
    ) {
        if !model.creation {
            return;
        }
        let old = state.envelopes[self.body.places[destination.0 as usize].root.0 as usize].clone();
        let incoming = self.creation_operand(state, value);
        self.store_creation(state, displaced, old);
        self.store_creation(state, destination, incoming);
    }

    pub(super) fn call_creation(
        &self,
        model: &Model,
        state: &mut State,
        callee: &Callee,
        arguments: &[CallArgument],
        destination: PlaceId,
    ) {
        if !model.creation {
            return;
        }
        let declaration = match callee {
            Callee::Definition { path, .. } => {
                self.signatures.get(path).and_then(|s| match &s.kind {
                    SignatureKind::Function(f) => Some((self.t, f)),
                    _ => None,
                })
            }
            Callee::Indirect { value, .. } => {
                self.t
                    .get(self.place_type(value.place))
                    .and_then(|row| match &row.kind {
                        TypeKind::Function(f) => Some((self.t, f)),
                        _ => None,
                    })
            }
        };
        let inputs = declaration.and_then(|(types, function)| {
            if !self.creation_callback_contracts(state, types, function, arguments) {
                return None;
            }
            let Some(ErasedCaptures::FromArguments(selectors)) =
                &types.get(function.result)?.erased_captures
            else {
                return None;
            };
            join(selectors.iter().map(|selector| {
                let ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } = selector.parameter
                else {
                    return None;
                };
                if !selector.fields.is_empty() {
                    return None;
                }
                let argument = arguments
                    .iter()
                    .find(|argument| argument.parameter == parameter)?;
                self.creation_operand(state, &argument.value)
            }))
        });
        for argument in arguments {
            if argument.value.usage == UseKind::Mut
                && !self.callee_preserves_state(callee, argument.parameter, false)
                || argument.value.usage == UseKind::Borrow
                    && !self.copy_creation(argument.value.place)
            {
                self.invalidate_creation(state, argument.value.place);
            }
        }
        if let Callee::Indirect { value, .. } = callee {
            self.invalidate_creation(state, value.place);
        }
        self.store_creation(state, destination, inputs);
    }

    /// The creation inputs a returned environment may retain: the selected
    /// whole parameters and the independently inert ones. None outside a
    /// creation owner.
    fn creation_allowed(&self) -> Option<BTreeSet<usize>> {
        let selectors = self.body_callable().and_then(|(types, function)| {
            match &types.get(function.result)?.erased_captures {
                Some(ErasedCaptures::FromArguments(selectors)) => Some(selectors.to_vec()),
                _ => None,
            }
        })?;
        let mut allowed: BTreeSet<_> = selectors
            .iter()
            .filter_map(|selector: &RelationPlace| {
                let ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } = selector.parameter
                else {
                    return None;
                };
                if !selector.fields.is_empty() {
                    return None;
                }
                self.body
                    .parameters
                    .get(parameter.0 as usize)
                    .map(|local| local.0 as usize)
            })
            .collect();
        allowed.extend(
            self.body
                .parameters
                .iter()
                .filter(|local| self.inert_creation(self.body.locals[local.0 as usize].ty))
                .map(|local| local.0 as usize),
        );
        Some(allowed)
    }

    /// The returned value retains only the declared creation inputs, so its
    /// environment's crossing evidence is theirs (CLO-4): the proof comes from
    /// creation operands, not from a producer's result row.
    pub(super) fn returns_within_creation(&self, state: &State, value: &Operand) -> bool {
        self.creation_allowed().is_some_and(|allowed| {
            self.creation_operand(state, value)
                .is_some_and(|inputs| inputs.is_subset(&allowed))
        })
    }

    pub(super) fn return_creation(
        &mut self,
        model: &Model,
        state: &State,
        value: &Operand,
        report: bool,
    ) {
        if !model.creation || self.creation_allowed().is_none() {
            return;
        }
        if !self.returns_within_creation(state, value) {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "creation capture envelope",
                },
            );
        }
    }
}
