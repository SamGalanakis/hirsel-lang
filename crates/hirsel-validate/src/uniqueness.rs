//! Sharing ends uniqueness until storage receives a fresh whole value.
use super::*;
use hirsel_types::{Sendability, TypeId, TypeKind, TypeStore, Uniqueness};

impl Checker<'_> {
    pub(crate) fn flat_value(&self, place: PlaceId) -> bool {
        self.bound(
            &self.definition.types,
            self.place_type(place),
            hirsel_types::Bound::Flat,
            &self.generic_declarations(),
        )
    }
    pub(crate) fn copied_type(&self, store: &mut TypeStore, value: &Operand, ty: TypeId) -> TypeId {
        if value.usage != UseKind::Copy {
            return ty;
        }
        if super::handle_root(store, ty) {
            // MEM-11: a copied Shared root is an independently counted handle
            // on an immutable root that is never region-placed: it is global.
            let mut row = store.get(ty).unwrap().clone();
            row.modes.relation = self.placement(&row.modes.relation);
            let root = store.push(row.clone());
            row.modes.sendability = hirsel_types::structural_sendability(
                store,
                root,
                &self.generic_declarations(),
                &mut |p| {
                    Ok::<_, hirsel_types::QueryError>(
                        self.signatures.get(p).cloned().map(std::sync::Arc::new),
                    )
                },
            )
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
            return store.push(row);
        }
        if !self.flat_value(value.place) {
            return ty;
        }
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let root = store.push(row.clone());
        row.modes.sendability = hirsel_types::content_sendability(
            store,
            root,
            &self.generic_declarations(),
            &mut |p| {
                Ok::<_, hirsel_types::QueryError>(
                    self.signatures.get(p).cloned().map(std::sync::Arc::new),
                )
            },
        )
        .unwrap_or_else(|error| self.proof_failure(error, hirsel_types::Sendability::NotSendable));
        store.push(row)
    }
    pub(crate) fn weakened_type(
        &self,
        store: &mut TypeStore,
        ty: TypeId,
        generics: &hirsel_types::GenericDeclarations,
    ) -> TypeId {
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Aliased;
        if !erased_environment(&row.kind) {
            row.modes.sendability = Sendability::NotSendable;
        }
        let root = store.push(row.clone());
        row.modes.sendability =
            hirsel_types::sendability_after_sharing(store, root, generics, &mut |p| {
                Ok::<_, hirsel_types::QueryError>(
                    self.signatures.get(p).cloned().map(std::sync::Arc::new),
                )
            })
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
        store.push(row)
    }
    pub(super) fn transferred_type(
        &self,
        state: &State,
        value: &Operand,
        store: &mut TypeStore,
    ) -> TypeId {
        let root = self.body.places[value.place.0 as usize].root.0 as usize;
        let ty = self.place_type(value.place);
        let mut row = store.get(ty).unwrap().clone();
        if self.body.places[value.place.0 as usize]
            .projection
            .is_empty()
            && !erased_environment(&row.kind)
        {
            row.modes.sendability = if state.must[root].has(MustFacts::SENDABLE) {
                Sendability::Sendable
            } else if !state.shared[root]
                && matches!(row.modes.sendability, Sendability::DependsOn(_))
                && state.crossing_parts[root].get(&[]) != Some(false)
            {
                row.modes.sendability.clone()
            } else {
                hirsel_types::sendability_after_sharing(
                    store,
                    ty,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
            };
        }
        if !self.body.places[value.place.0 as usize]
            .projection
            .is_empty()
            && (state.must[root].has(MustFacts::SENDABLE)
                || !state.shared[root] && state.current_unique[root])
            && (matches!(value.usage, UseKind::Take | UseKind::Borrow | UseKind::Mut)
                || value.usage == UseKind::Copy && erased_environment(&row.kind))
        {
            let mut proof = row.clone();
            proof.modes.relation = self.placement(&proof.modes.relation);
            if state.must[root].has(MustFacts::SENDABLE) {
                proof.modes.sendability = Sendability::Sendable;
            } else {
                let part = store.push(proof);
                let unique = self.unique_type(store, part);
                proof = store.get(unique).unwrap().clone();
            }
            row.modes.sendability = proof.modes.sendability;
        }
        let part_path = crossing::path(&self.body.places[value.place.0 as usize].projection);
        let part_proof = state.crossing_parts[root].get(&part_path);
        let independent = (!part_path.is_empty() || part_proof.is_some())
            && hirsel_types::sendability_after_sharing(
                store,
                ty,
                &self.generic_declarations(),
                &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
            )
            .is_ok_and(|proof| proof == Sendability::Sendable);
        if independent && !part_path.is_empty() && !erased_environment(&row.kind) {
            row.modes.sendability = Sendability::Sendable;
        }
        if let Some(proof) = part_proof {
            if !independent && (!part_path.is_empty() || !proof || erased_environment(&row.kind)) {
                row.modes.sendability = if proof {
                    Sendability::Sendable
                } else {
                    Sendability::NotSendable
                };
            }
        }
        let ty = store.push(row);
        let ty = if self.global_operand(state, value)
            && store.get(ty).unwrap().modes.relation.has_environment()
        {
            let mut row = store.get(ty).unwrap().clone();
            row.modes.relation = self.placement(&row.modes.relation);
            if state.must[root].has(MustFacts::SENDABLE)
                && self.body.places[value.place.0 as usize]
                    .projection
                    .is_empty()
            {
                row.modes.sendability = Sendability::Sendable;
            }

            let root = store.push(row);
            self.derive_sendability(store, root)
        } else {
            ty
        };
        let copied = self.copied_type(store, value, ty);
        if copied != ty {
            return copied;
        }
        if self.operand_unique(state, value) {
            let unique = self.unique_type(store, ty);
            if state.must[root].has(MustFacts::SENDABLE) {
                let mut row = store.get(unique).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                return store.push(row);
            }
            return unique;
        }
        let projected = !self.body.places[value.place.0 as usize]
            .projection
            .is_empty();
        if (state.shared[root] && (!projected || part_proof != Some(true))
            || value.usage == UseKind::Copy)
            && needs_release(self, ty, &self.definition.types)
        {
            let weakened = self.weakened_type(store, ty, &self.generic_declarations());
            if !projected
                && value.usage != UseKind::Copy
                && state.must[root].has(MustFacts::SENDABLE)
            {
                let mut row = store.get(weakened).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                store.push(row)
            } else {
                weakened
            }
        } else {
            let access = &self.body.places[value.place.0 as usize];
            if access.projection.is_empty() && state.must[root].has(MustFacts::SENDABLE) {
                let mut row = store.get(ty).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                store.push(row)
            } else if projected && !self.operand_unique(state, value) {
                let mut row = store.get(ty).unwrap().clone();
                row.modes.uniqueness = Uniqueness::Aliased;
                store.push(row)
            } else {
                ty
            }
        }
    }
    pub(super) fn rvalue_sendable(
        &self,
        state: &State,
        destination: PlaceId,
        rvalue: &Rvalue,
    ) -> bool {
        let declared = self.place_type(destination);
        let mut store = self.working_store();
        if !self
            .placement(&store.get(declared).unwrap().modes.relation)
            .is_global()
        {
            return false;
        }
        match rvalue {
            Rvalue::Use(value) => {
                let ty = self.transferred_type(state, value, &mut store);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
                    || self.bound(
                        &store,
                        ty,
                        hirsel_types::Bound::Plain,
                        &self.generic_declarations(),
                    )
            }
            Rvalue::Constant(_)
            | Rvalue::Size(_)
            | Rvalue::Shape(_)
            | Rvalue::FunctionReference { .. } => true,
            Rvalue::Construct { fields, .. } => fields.iter().all(|field| {
                let ty = self.transferred_type(state, &field.value, &mut store);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }),
            Rvalue::Closure { captures, .. } => captures.iter().all(|capture| {
                let ty = self.transferred_type(state, capture, &mut store);
                let mut lookup = |path: &hirsel_syntax::DefPath| {
                    Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
                };
                let proof = if capture.usage == UseKind::Copy {
                    hirsel_types::sendability_after_sharing(
                        &store,
                        ty,
                        &self.generic_declarations(),
                        &mut lookup,
                    )
                } else {
                    hirsel_types::content_sendability(
                        &store,
                        ty,
                        &self.generic_declarations(),
                        &mut lookup,
                    )
                };
                proof.unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
                    == Sendability::Sendable
            }),
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } => {
                let incoming = self.transferred_type(state, value, &mut store);
                let mut row = store.get(incoming).unwrap().clone();
                row.modes.relation = self.placement(&row.modes.relation);
                let incoming = store.push(row);
                let ty = self.unique_type(&mut store, incoming);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Box,
                value,
            } => {
                let ty = self.transferred_type(state, value, &mut store);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
            _ => {
                let ty = self.derive_sendability(&mut store, declared);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
        }
    }
    pub(super) fn crossing_receipts(
        &self,
        store: &TypeStore,
        root: TypeId,
    ) -> Vec<(Vec<crossing::Part>, bool)> {
        hirsel_types::owned_crossing_receipts(
            store,
            root,
            &self.generic_declarations(),
            &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
        )
        .unwrap_or_else(|error| self.proof_failure(error, Vec::new()))
        .into_iter()
        .map(|(path, stable)| {
            (
                path.into_iter()
                    .map(|part| match part {
                        hirsel_types::OwnedTypePart::Field(f) => crossing::Part::Field(f),
                        hirsel_types::OwnedTypePart::VariantField { variant, field } => {
                            crossing::Part::Variant(variant, field)
                        }
                        hirsel_types::OwnedTypePart::Elements => crossing::Part::Elements,
                    })
                    .collect(),
                stable,
            )
        })
        .collect()
    }
    fn has_crossing_receipt(&self, store: &TypeStore, root: TypeId) -> bool {
        // A graph with no written owned requirement owes no part receipt.
        // Inspect declarations before expanding recursive generic instances.
        let mut pending = vec![(None, root)];
        let mut seen = BTreeSet::new();
        while let Some((definition, id)) = pending.pop() {
            if !seen.insert((definition.clone(), id)) {
                continue;
            }
            let types = definition
                .as_ref()
                .and_then(|path| self.signatures.get(path))
                .map_or(store, |s| &s.types);
            let Some(row) = types.get(id) else { continue };
            if row.sendability_requirement == hirsel_types::SendabilityRequirement::Sendable {
                return true;
            }
            pending.extend(
                row.kind
                    .value_parts()
                    .into_iter()
                    .map(|id| (definition.clone(), id)),
            );
            if let TypeKind::Named { definition, .. } = &row.kind {
                if let Some(signature) = self.signatures.get(definition) {
                    if let hirsel_types::SignatureKind::Type(declaration) = &signature.kind {
                        if let Some(representation) = declaration.representation {
                            pending.push((Some(definition.clone()), representation));
                        }
                    }
                }
            }
        }
        false
    }
    pub(super) fn nested_crossing_receipt(
        &mut self,
        state: &State,
        store: &TypeStore,
        root: TypeId,
        value: &Operand,
        report: bool,
    ) {
        if !self.has_crossing_receipt(store, root) {
            return;
        }
        let access = &self.body.places[value.place.0 as usize];
        let parts = state.crossing_parts[access.root.0 as usize]
            .project(&crossing::path(&access.projection));
        for (path, stable) in self.crossing_receipts(store, root) {
            if !path.is_empty() && !stable && parts.get(&path) == Some(false) {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: "owned crossing receipt",
                    },
                );
            }
        }
    }
    pub(super) fn call_crossing_receipts(
        &mut self,
        state: &State,
        callee: &Callee,
        values: &[CallArgument],
        report: bool,
    ) {
        let (source, function, arguments, roles) = match callee {
            Callee::Definition { path, arguments } => {
                let Some(signature) = self.signatures.get(path).cloned() else {
                    return;
                };
                let SignatureKind::Function(function) = signature.kind else {
                    return;
                };
                (
                    signature.types,
                    function,
                    arguments,
                    signature.metadata.roles,
                )
            }
            Callee::Indirect {
                forward_caller: _,
                value,
                arguments,
            } => {
                let source = self.definition.types.clone();
                let TypeKind::Function(function) = source
                    .get(self.place_type(value.place))
                    .unwrap()
                    .kind
                    .clone()
                else {
                    return;
                };
                (
                    source,
                    function,
                    arguments,
                    hirsel_types::NominalRoles::default(),
                )
            }
        };
        let mut store = self.working_store();
        for value in values {
            let root = hirsel_types::generic_crossing_parameter(
                &source,
                &function,
                value.parameter.0 as usize,
                &mut store,
                arguments,
                &roles,
            );
            if let Ok(Some(root)) = root {
                self.nested_crossing_receipt(state, &store, root, &value.value, report);
            }
        }
    }
    pub(super) fn operand_crossing_parts(&self, state: &State, value: &Operand) -> crossing::Parts {
        let access = &self.body.places[value.place.0 as usize];
        let mut parts = state.crossing_parts[access.root.0 as usize]
            .project(&crossing::path(&access.projection));
        let mut store = self.working_store();
        let transferred = self.transferred_type(state, value, &mut store);
        let evidence = &store.get(transferred).unwrap().modes.sendability;
        if *evidence == Sendability::Sendable {
            parts.set(&[], true);
        } else if value.usage == UseKind::Copy {
            let mut referent = value.clone();
            referent.usage = UseKind::Borrow;
            let current = self.transferred_type(state, &referent, &mut store);
            if store.get(current).unwrap().modes.sendability == Sendability::Sendable {
                parts.set(&[], true);
            }
            self.share_crossing_parts(&mut parts, value.place);
        } else if *evidence == Sendability::NotSendable
            && matches!(store.get(transferred).unwrap().kind, TypeKind::Parameter(_))
        {
            // A move into ordinary storage is legal, but its derived row must
            // not resurrect a conditional proof absent from the incoming value.
            parts.set(&[], false);
        }
        parts
    }
    /// Sharing data cannot invalidate a proved immutable payload contract.
    /// Keep only stable paths whose current proof already holds; a written
    /// requirement alone never supplies the proof.
    pub(super) fn share_crossing_parts(&self, parts: &mut crossing::Parts, place: PlaceId) {
        let ty = self.place_type(place);
        if !self.has_crossing_receipt(&self.definition.types, ty) {
            parts.set(&[], false);
            return;
        }
        let stable: Vec<_> = self
            .crossing_receipts(&self.definition.types, ty)
            .into_iter()
            .filter(|(path, stable)| {
                if parts.get(path) != Some(true) {
                    return false;
                }
                if *stable {
                    return true;
                }
                let mut id = ty;
                for part in path {
                    let row = self.definition.types.get(id).unwrap();
                    let next = match (part, &row.kind) {
                        (crossing::Part::Field(0), TypeKind::Box(inner)) => Some(*inner),
                        (crossing::Part::Field(n), TypeKind::Tuple(parts)) => {
                            parts.get(*n as usize).copied()
                        }
                        (crossing::Part::Field(n), TypeKind::Record(fields)) => {
                            fields.get(*n as usize).map(|f| f.ty)
                        }
                        (crossing::Part::Variant(v, n), TypeKind::Variant(variants)) => variants
                            .get(*v as usize)
                            .and_then(|v| v.fields.get(*n as usize))
                            .map(|f| f.1),
                        (
                            crossing::Part::Elements,
                            TypeKind::Builtin {
                                kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
                                arguments,
                            },
                        ) => arguments.first().and_then(|a| {
                            if let GenericArgument::Type(id) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        }),
                        _ => None,
                    };
                    let Some(next) = next else { return false };
                    id = next;
                }
                matches!(
                    self.definition.types.get(id).unwrap().kind,
                    TypeKind::Function(_)
                )
            })
            .map(|(path, _)| path)
            .collect();
        parts.set(&[], false);
        for path in stable {
            let mut proved = crossing::Parts::default();
            proved.set(&[], true);
            parts.install(&path, &proved);
        }
    }
    pub(super) fn rvalue_crossing_parts(
        &self,
        state: &State,
        destination: PlaceId,
        value: &Rvalue,
    ) -> crossing::Parts {
        match value {
            Rvalue::Use(value) => self.operand_crossing_parts(state, value),
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                let mut parts = crossing::Parts::default();
                for field in fields {
                    let path = match constructor {
                        Constructor::Variant { variant, .. } => {
                            crossing::Part::Variant(*variant, field.field)
                        }
                        Constructor::Sequence | Constructor::Map | Constructor::Set => {
                            crossing::Part::Elements
                        }
                        _ => crossing::Part::Field(field.field),
                    };
                    let incoming = self.operand_crossing_parts(state, &field.value);
                    parts.install(&[path], &incoming);
                }
                if self.rvalue_sendable(state, destination, value) {
                    parts.set(&[], true);
                }
                parts
            }
            _ => {
                let mut parts = crossing::Parts::default();
                if self.rvalue_sendable(state, destination, value) {
                    parts.set(&[], true);
                }
                parts
            }
        }
    }
    pub(super) fn install_crossing_parts(
        &self,
        state: &mut State,
        destination: PlaceId,
        parts: &crossing::Parts,
        depth: usize,
    ) {
        let access = &self.body.places[destination.0 as usize];
        if access.projection.is_empty() {
            state.crossing_parts[access.root.0 as usize] = parts.clone();
        } else {
            state.crossing_parts[access.root.0 as usize]
                .install(&crossing::path(&access.projection), parts);
        }
        state.crossing_parts[access.root.0 as usize].restrict(depth);
    }
    pub(super) fn set_current_sendable(
        &self,
        state: &mut State,
        destination: PlaceId,
        sendable: bool,
    ) {
        let place = &self.body.places[destination.0 as usize];
        if sendable {
            state.crossing_parts[place.root.0 as usize]
                .set(&crossing::path(&place.projection), true);
        }
        if place.projection.is_empty() {
            state.must[place.root.0 as usize].set(MustFacts::SENDABLE, sendable);
        } else if !sendable {
            state.must[place.root.0 as usize].set(MustFacts::SENDABLE, false);
        }
    }

    pub(super) fn mode_transfer(
        &mut self,
        state: &State,
        destination: TypeId,
        value: &Operand,
        report: bool,
    ) {
        let mut store = self.working_store();
        self.mode_transfer_in(state, &mut store, destination, value, false, report);
    }
    fn mode_transfer_in(
        &mut self,
        state: &State,
        store: &mut TypeStore,
        destination: TypeId,
        value: &Operand,
        borrowed_capture: bool,
        report: bool,
    ) {
        let mut actual = self.transferred_type(state, value, store);
        if borrowed_capture {
            let promised = &store.get(destination).unwrap().modes.sendability;
            let supplied = store.get(actual).unwrap().modes.sendability.clone();
            let scope_correct = match (promised, &store.get(destination).unwrap().kind) {
                (Sendability::DependsOn(parameters), TypeKind::Parameter(parameter)) => {
                    parameters.iter().all(|p| p == parameter)
                }
                _ => true,
            };
            let justified = scope_correct
                && match (promised, &supplied) {
                    (Sendability::NotSendable, _) | (_, Sendability::Sendable) => true,
                    (Sendability::DependsOn(wanted), Sendability::DependsOn(actual)) => {
                        actual.iter().all(|p| wanted.contains(p))
                    }
                    _ => false,
                };
            if !justified {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: "value modes",
                    },
                );
            }
        }
        if borrowed_capture {
            let receiving = store.get(destination).unwrap();
            let mut row = store.get(actual).unwrap().clone();
            // Borrowing changes access permission, retaining current referent evidence.
            if receiving.modes.uniqueness == Uniqueness::Aliased {
                row.modes.uniqueness = Uniqueness::Aliased;
            }
            row.modes.relation = self
                .placement(&row.modes.relation)
                .union(&self.lifetime(&receiving.modes.relation));
            actual = store.push(row);
        }
        let returned = self.site.instruction.is_none()
            && self.site.block.is_some_and(|b| {
                matches!(&self.body.blocks[b.0 as usize].terminator,
                Terminator::Return { value: returned, .. } if returned == value)
            });
        let destination = if returned {
            self.return_type(store).0
        } else {
            destination
        };
        if self.receiving_root(self.body.places[value.place.0 as usize].root)
            && self
                .lifetime(&store.get(destination).unwrap().modes.relation)
                .is_global()
            && !self
                .lifetime(&store.get(actual).unwrap().modes.relation)
                .is_global()
        {
            let returning = self.site.instruction.is_none()
                && self.site.block.is_some_and(|b| {
                    matches!(
                        self.body.blocks[b.0 as usize].terminator,
                        Terminator::Return { .. }
                    )
                });
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: if returning { "return" } else { "assignment" },
                },
            );
        }
        let expected = store.get(destination).unwrap();
        let actual = store.get(actual).unwrap();
        if (expected.modes.uniqueness == Uniqueness::Unique
            && actual.modes.uniqueness != Uniqueness::Unique
            && needs_release(self, destination, &store))
            || (!borrowed_capture
                && hirsel_types::crossing_requirement(expected) == Sendability::Sendable
                && actual.modes.sendability != Sendability::Sendable
                // A returned environment within the declared creation inputs
                // has their evidence; `return_creation` proves that envelope.
                && !(returned
                    && erased_environment(&actual.kind)
                    && self.returns_within_creation(state, value)))
        {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "value modes",
                },
            );
        }
    }
    pub(super) fn rvalue_modes(
        &mut self,
        state: &State,
        destination: PlaceId,
        rvalue: &Rvalue,
        report: bool,
    ) {
        let ty = self.place_type(destination);
        match rvalue {
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                let mut store = self.working_store();
                if let Some(parts) =
                    self.constructor_layout(&mut store, ty, constructor, fields.len())
                {
                    for field in fields {
                        if let Some(expected) = parts.get(field.field as usize).copied() {
                            if store.get(expected).unwrap().sendability_requirement
                                == hirsel_types::SendabilityRequirement::Sendable
                            {
                                let actual = self.transferred_type(state, &field.value, &mut store);
                                if store.get(actual).unwrap().modes.sendability
                                    != Sendability::Sendable
                                {
                                    self.flow_error(
                                        report,
                                        ViolationKind::TypeMismatch {
                                            entity: "constructor field",
                                        },
                                    );
                                }
                            }
                            self.nested_crossing_receipt(
                                state,
                                &store,
                                expected,
                                &field.value,
                                report,
                            );
                        }
                    }
                }
            }
            _ => (),
        }
        let row = self.definition.types.get(ty).unwrap();
        if let (Sendability::DependsOn(promised), Rvalue::Use(value)) =
            (&row.modes.sendability, rvalue)
            && erased_environment(&row.kind)
        {
            let mut store = self.working_store();
            let actual = self.transferred_type(state, value, &mut store);
            let evidence = store.get(actual).unwrap().modes.sendability.clone();
            let justified = match &evidence {
                Sendability::Sendable => match &store.get(actual).unwrap().kind {
                    TypeKind::Parameter(parameter) => promised.iter().all(|p| p == parameter),
                    _ => true,
                },
                Sendability::DependsOn(actual) => actual.iter().all(|p| promised.contains(p)),
                Sendability::NotSendable => false,
            };
            if !justified {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: "sendable receipt",
                    },
                );
            }
        }
        if (row.sendability_requirement == hirsel_types::SendabilityRequirement::Sendable
            || row.modes.sendability == Sendability::Sendable)
            && !self.rvalue_sendable(state, destination, rvalue)
        {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: if matches!(rvalue, Rvalue::Closure { .. }) {
                        "closure sendability"
                    } else {
                        "sendable receipt"
                    },
                },
            );
        }
        if let Rvalue::Closure { body, captures, .. } = rvalue {
            let child = &self.definition.bodies[body.0 as usize];
            let receiving: Vec<_> = child
                .locals
                .iter()
                .filter(|local| local.storage == Storage::Capture)
                .map(|local| local.ty)
                .collect();
            let child_types = self.definition.types.clone();
            let specialised = self.environment_specialised(ty, *body, captures);
            for (root, value) in receiving.into_iter().zip(captures) {
                if specialised && self.current_content_source(value) {
                    if !self.content_proof(Some(state), value.place).deep {
                        self.flow_error(
                            report,
                            ViolationKind::TypeMismatch {
                                entity: "closure capture",
                            },
                        );
                    }
                }
                let mut store = self.working_store();
                if let Ok(expected) = hirsel_types::import_type(&child_types, root, &mut store) {
                    self.mode_transfer_in(
                        state,
                        &mut store,
                        expected,
                        value,
                        matches!(value.usage, UseKind::Borrow | UseKind::Mut),
                        report,
                    );
                }
            }
        }
        match rvalue {
            Rvalue::Use(value) => self.mode_transfer(state, ty, value, report),
            Rvalue::Construct { .. } | Rvalue::Closure { .. }
                if self.definition.types.get(ty).unwrap().modes.uniqueness
                    == Uniqueness::Unique =>
            {
                let values: Vec<_> = match rvalue {
                    Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
                    Rvalue::Closure { captures, .. } => captures.iter().collect(),
                    _ => unreachable!(),
                };
                for value in values {
                    let mut store = self.working_store();
                    let actual = self.transferred_type(state, value, &mut store);
                    let counted =
                        needs_release(self, self.place_type(value.place), &self.definition.types);
                    let shared = counted
                        && matches!(
                            self.layout(&mut store, self.place_type(value.place)),
                            Some(TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Shared,
                                ..
                            })
                        );
                    let owned = value.usage == UseKind::Take
                        || value.usage == UseKind::Copy && self.flat_value(value.place);
                    if !shared
                        && counted
                        && (!owned
                            || store.get(actual).unwrap().modes.uniqueness != Uniqueness::Unique)
                    {
                        self.flow_error(
                            report,
                            ViolationKind::TypeMismatch {
                                entity: "unique construction",
                            },
                        );
                    }
                }
            }
            _ => (),
        }
    }
    pub(crate) fn unique_type(&self, store: &mut TypeStore, ty: TypeId) -> TypeId {
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let root = store.push(row);
        let mut row = store.get(root).unwrap().clone();
        row.modes.sendability = hirsel_types::structural_sendability(
            store,
            root,
            &self.generic_declarations(),
            &mut |path| {
                Ok::<_, hirsel_types::QueryError>(
                    self.signatures.get(path).cloned().map(std::sync::Arc::new),
                )
            },
        )
        .unwrap_or_else(|error| self.proof_failure(error, hirsel_types::Sendability::NotSendable));
        store.push(row)
    }
    pub(super) fn operand_fresh_receipt(&self, state: &State, value: &Operand) -> bool {
        let place = &self.body.places[value.place.0 as usize];
        value.usage == UseKind::Take
            && !state.shared[place.root.0 as usize]
            && state.must[place.root.0 as usize].has(MustFacts::OWNED)
            && (place.projection.is_empty()
                && state.must[place.root.0 as usize].has(MustFacts::FRESH_RECEIPT)
                || self.operand_unique(state, value))
    }
    pub(super) fn rvalue_fresh_receipt(&self, state: &State, value: &Rvalue) -> bool {
        match value {
            Rvalue::Use(value) => self.operand_fresh_receipt(state, value),
            Rvalue::Construct { .. }
            | Rvalue::Closure { .. }
            | Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique | hirsel_syntax::UnaryOp::Box,
                ..
            } => true,
            _ => false,
        }
    }
    pub(super) fn set_fresh_receipt(&self, state: &mut State, destination: PlaceId, fresh: bool) {
        let place = &self.body.places[destination.0 as usize];
        if place.projection.is_empty() {
            state.must[place.root.0 as usize].set(MustFacts::FRESH_RECEIPT, fresh);
        }
    }
    pub(super) fn operand_unique(&self, state: &State, value: &Operand) -> bool {
        if !needs_release(self, self.place_type(value.place), &self.definition.types) {
            return false;
        }
        if value.usage == UseKind::Copy {
            return self.flat_value(value.place);
        }
        let place = &self.body.places[value.place.0 as usize];
        let root = place.root.0 as usize;
        if !place.projection.is_empty() {
            // A declared unique payload may move from an unshared owned
            // receipt. Otherwise the parent must prove deep uniqueness;
            // neither proof survives a counted share or a Shared boundary.
            // Exclusive access to such a part is unique access too (MODE-2).
            return matches!(value.usage, UseKind::Take | UseKind::Mut)
                && !state.shared[root]
                && (state.current_unique[root]
                    || state.must[root].has(MustFacts::OWNED)
                        && state.must[root].has(MustFacts::FRESH_RECEIPT)
                        && self
                            .definition
                            .types
                            .get(place.ty)
                            .unwrap()
                            .modes
                            .uniqueness
                            == Uniqueness::Unique)
                && !matches!(
                    self.definition.types.get(place.ty).unwrap().kind,
                    TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::Shared,
                        ..
                    }
                );
        }
        !state.shared[root]
            && (state.current_unique[root]
                || self
                    .definition
                    .types
                    .get(self.place_type(value.place))
                    .unwrap()
                    .modes
                    .uniqueness
                    == Uniqueness::Unique)
    }
    pub(super) fn rvalue_unique(&self, state: &State, value: &Rvalue) -> bool {
        match value {
            Rvalue::Constant(_) | Rvalue::Size(_) | Rvalue::Shape(_) | Rvalue::FunctionReference { .. } => true,
            Rvalue::ReadConstant { definition } => self.signatures.get(definition).is_some_and(|s| matches!(s.kind, SignatureKind::Const(ty) if self.bound(&s.types, ty, hirsel_types::Bound::Plain, &hirsel_types::GenericDeclarations::default()) || self.bound(&s.types, ty, hirsel_types::Bound::Flat, &hirsel_types::GenericDeclarations::default()) || matches!(s.types.get(ty).unwrap().kind, TypeKind::Builtin { kind: hirsel_types::BuiltinType::Shared, .. }))),
            Rvalue::Use(value) => {
                self.operand_unique(state, value)
            }
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                ..
            } => true,
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Box,
                value,
            } => self.operand_unique(state, value),
            Rvalue::Construct { fields, .. } => fields.iter().all(|f| {
                !needs_release(self, self.place_type(f.value.place), &self.definition.types)
                    || self.operand_unique(state, &f.value)
            }),
            Rvalue::Closure { captures, .. } => captures.iter().all(|v| {
                !needs_release(self, self.place_type(v.place), &self.definition.types)
                    || v.usage == UseKind::Take && self.operand_unique(state, v)
                    || v.usage == UseKind::Copy && self.flat_value(v.place)
            }),
            _ => false,
        }
    }
    pub(super) fn set_current_unique(&self, state: &mut State, destination: PlaceId, unique: bool) {
        let place = &self.body.places[destination.0 as usize];
        if place.projection.is_empty() {
            state.current_unique[place.root.0 as usize] = unique;
        } else if needs_release(self, place.ty, &self.definition.types)
            && !matches!(
                self.definition.types.get(place.ty).unwrap().kind,
                hirsel_types::TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Shared,
                    ..
                }
            )
        {
            // Plain bits introduce no counted aliases; Shared is an explicit
            // boundary of the parent's uniquely owned graph (MODE-2).
            state.current_unique[place.root.0 as usize] &= unique;
        }
    }
    pub(super) fn reset_unique(&self, state: &mut State, destination: PlaceId) {
        let place = &self.body.places[destination.0 as usize];
        if place.projection.is_empty() {
            state.shared[place.root.0 as usize] = false;
            state.borrow_only[place.root.0 as usize] = false;
        }
    }
    pub(super) fn mut_requires_unique(
        &self,
        callee: &Callee,
        parameter: hirsel_types::ParamId,
    ) -> bool {
        match callee {
            Callee::Definition { path, .. } => self.signatures.get(path).is_some_and(|s| matches!(&s.kind, SignatureKind::Function(f) if f.parameters.get(parameter.0 as usize).is_some_and(|p| s.types.get(p.ty).unwrap().modes.uniqueness == Uniqueness::Unique))),
            Callee::Indirect { value, .. } => matches!(&self.definition.types.get(self.place_type(value.place)).unwrap().kind, TypeKind::Function(f) if f.parameters.get(parameter.0 as usize).is_some_and(|p| self.definition.types.get(p.ty).unwrap().modes.uniqueness == Uniqueness::Unique)),
        }
    }
}

/// MODE-4: an Iter or function value's crossing mode is its producer's
/// environment fact, which Local receiving storage masks in its row; the
/// Sendable must fact carries the current one (#88).
pub(super) fn erased_environment(kind: &hirsel_types::TypeKind) -> bool {
    matches!(
        kind,
        hirsel_types::TypeKind::Function(_)
            | hirsel_types::TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::Iter,
                ..
            }
    )
}
