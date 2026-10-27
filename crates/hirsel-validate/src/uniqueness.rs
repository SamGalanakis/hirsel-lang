//! Sharing ends uniqueness until storage receives a fresh whole value.
use super::*;
use hirsel_types::{Sendability, TypeId, TypeInterner, TypeKind, Uniqueness};

// A mode transformation that leaves its immutable input unchanged needs no
// canonical lookup. Changed rows still use the interner's complete equality.
fn update_type(store: &TypeInterner, original: TypeId, row: hirsel_types::Type) -> TypeId {
    if store.get(original) == Some(&row) {
        original
    } else {
        store.intern(row)
    }
}

impl Checker<'_> {
    /// Core consuming cursors retain their owner's current evidence, rather
    /// than manufacturing a fresh unique payload from a type argument.
    pub(super) fn consuming_protocol(&self, callee: &Callee) -> bool {
        let Callee::Definition { path, .. } = callee else {
            return false;
        };
        static OWNING: std::sync::LazyLock<std::collections::BTreeSet<hirsel_syntax::DefPath>> =
            std::sync::LazyLock::new(|| {
                hirsel_types::core_roles::loop_protocols()
                    .into_iter()
                    .filter(|(name, _, _)| *name != "SetElements")
                    .flat_map(|(_, open, next)| [open, next])
                    .collect()
            });
        OWNING.contains(path)
    }
    pub(crate) fn flat_value(&self, place: PlaceId) -> bool {
        self.bound(
            self.t,
            self.place_type(place),
            hirsel_types::Bound::Flat,
            &self.generic_declarations(),
        )
    }
    pub(crate) fn copied_type(&self, store: &TypeInterner, value: &Operand, ty: TypeId) -> TypeId {
        if value.usage != UseKind::Copy {
            return ty;
        }
        if !self.flat_value(value.place) && self.copy_detaches(store, ty) {
            // MEM-11, CONC-2: a copied Shared root, Mutex or Atomic, or an
            // aggregate of them and plain bits, is an independently counted
            // value, never region-placed: it is global.
            let mut row = store.get(ty).unwrap().clone();
            row.modes.relation = self.placement(&row.modes.relation);
            let root = update_type(store, ty, row.clone());
            row.modes.sendability = self
                .proof(
                    store,
                    root,
                    super::proofs::Proof::Sendability,
                    &self.generic_declarations(),
                )
                .unwrap_or_else(|error| {
                    self.proof_failure(error, hirsel_types::Sendability::NotSendable)
                });
            return update_type(store, ty, row);
        }
        if !self.flat_value(value.place) {
            return ty;
        }
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let root = update_type(store, ty, row.clone());
        row.modes.sendability = self
            .proof(
                store,
                root,
                super::proofs::Proof::Content,
                &self.generic_declarations(),
            )
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
        update_type(store, ty, row)
    }
    pub(crate) fn weakened_type(
        &self,
        store: &TypeInterner,
        ty: TypeId,
        generics: &hirsel_types::GenericDeclarations,
    ) -> TypeId {
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Aliased;
        if !erased_environment(&row.kind) {
            row.modes.sendability = Sendability::NotSendable;
        }
        let root = update_type(store, ty, row.clone());
        row.modes.sendability = self
            .proof(store, root, super::proofs::Proof::AfterSharing, generics)
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
        update_type(store, ty, row)
    }
    pub(super) fn transferred_type(
        &self,
        state: &State,
        value: &Operand,
        store: &TypeInterner,
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
                self.proof(
                    store,
                    ty,
                    super::proofs::Proof::AfterSharing,
                    &self.generic_declarations(),
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
                let part = update_type(store, ty, proof);
                let unique = self.unique_type(store, part);
                proof = store.get(unique).unwrap().clone();
            }
            row.modes.sendability = proof.modes.sendability;
        }
        let part_path = crossing::path(&self.body.places[value.place.0 as usize].projection);
        let part_proof = state.crossing_parts[root].get(&part_path);
        let independent = (!part_path.is_empty() || part_proof.is_some())
            && self
                .proof(
                    store,
                    ty,
                    super::proofs::Proof::AfterSharing,
                    &self.generic_declarations(),
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
        let ty = update_type(store, ty, row);
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

            let root = update_type(store, ty, row);
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
                return update_type(store, unique, row);
            }
            return unique;
        }
        let projected = !self.body.places[value.place.0 as usize]
            .projection
            .is_empty();
        if (state.shared[root] && (!projected || part_proof != Some(true))
            || value.usage == UseKind::Copy)
            && needs_release(self, ty, self.t)
        {
            let weakened = self.weakened_type(store, ty, &self.generic_declarations());
            if !projected
                && value.usage != UseKind::Copy
                && state.must[root].has(MustFacts::SENDABLE)
            {
                let mut row = store.get(weakened).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                update_type(store, weakened, row)
            } else {
                weakened
            }
        } else {
            let access = &self.body.places[value.place.0 as usize];
            if access.projection.is_empty() && state.must[root].has(MustFacts::SENDABLE) {
                let mut row = store.get(ty).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                update_type(store, ty, row)
            } else if projected && !self.operand_unique(state, value) {
                let mut row = store.get(ty).unwrap().clone();
                row.modes.uniqueness = Uniqueness::Aliased;
                update_type(store, ty, row)
            } else {
                ty
            }
        }
    }
    /// A closure's crossing receipt concerns the captures' current content,
    /// not the physical receiving rows. Keep symbolic obligations as well as
    /// unconditional evidence; a counted copy must reprove after sharing.
    pub(crate) fn closure_sendability(
        &self,
        state: Option<&State>,
        captures: &[Operand],
    ) -> Sendability {
        let store = self.t;
        captures
            .iter()
            .fold(Sendability::Sendable, |joined, capture| {
                let ty = state.map_or_else(
                    || self.place_type(capture.place),
                    |state| self.transferred_type(state, capture, store),
                );
                let query = if capture.usage == UseKind::Copy {
                    super::proofs::Proof::AfterSharing
                } else {
                    super::proofs::Proof::Content
                };
                let proof = self
                    .proof(store, ty, query, self.generic_declarations())
                    .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
                match (joined, proof) {
                    (Sendability::NotSendable, _) | (_, Sendability::NotSendable) => {
                        Sendability::NotSendable
                    }
                    (Sendability::Sendable, other) | (other, Sendability::Sendable) => other,
                    (Sendability::DependsOn(mut a), Sendability::DependsOn(b)) => {
                        a.extend(b);
                        a.sort();
                        a.dedup();
                        Sendability::DependsOn(a)
                    }
                }
            })
    }
    pub(super) fn rvalue_sendable(
        &self,
        state: &State,
        destination: PlaceId,
        rvalue: &Rvalue,
    ) -> bool {
        let declared = self.place_type(destination);
        let store = self.t;
        if !self
            .placement(&store.get(declared).unwrap().modes.relation)
            .is_global()
        {
            return false;
        }
        match rvalue {
            Rvalue::Use(value) => {
                let ty = self.transferred_type(state, value, store);
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
                let ty = self.transferred_type(state, &field.value, store);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }),
            Rvalue::Closure { captures, .. } => {
                self.closure_sendability(Some(state), captures) == Sendability::Sendable
            }
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } => {
                let incoming = self.transferred_type(state, value, store);
                let mut row = store.get(incoming).unwrap().clone();
                row.modes.relation = self.placement(&row.modes.relation);
                let incoming = store.intern(row);
                let ty = self.unique_type(store, incoming);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Box,
                value,
            } => {
                let ty = self.transferred_type(state, value, store);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
            _ => {
                let ty = self.derive_sendability(store, declared);
                store.get(ty).unwrap().modes.sendability == Sendability::Sendable
            }
        }
    }
    pub(super) fn crossing_receipts(
        &self,
        store: &TypeInterner,
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
    fn has_crossing_receipt(&self, store: &TypeInterner, root: TypeId) -> bool {
        // A graph with no written owned requirement owes no part receipt.
        // Inspect declarations before expanding recursive generic instances.
        let mut pending = vec![root];
        let mut seen = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(row) = store.get(id) else { continue };
            if row.sendability_requirement == hirsel_types::SendabilityRequirement::Sendable {
                return true;
            }
            pending.extend(row.kind.value_parts());
            if let TypeKind::Named { definition, .. } = &row.kind {
                if let Some(signature) = self.signatures.get(*definition) {
                    if let hirsel_types::SignatureKind::Type(declaration) = &signature.kind {
                        pending.extend(declaration.representation);
                    }
                }
            }
        }
        false
    }
    pub(super) fn nested_crossing_receipt(
        &mut self,
        state: &State,
        store: &TypeInterner,
        root: TypeId,
        value: &Operand,
        report: bool,
    ) {
        self.nested_unique_transfer(state, store, root, value, report);
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
        let (function, arguments, roles) = match callee {
            Callee::Definition { path, arguments } => {
                let Some(signature) = self.signatures.get(path).cloned() else {
                    return;
                };
                let SignatureKind::Function(function) = signature.kind else {
                    return;
                };
                (function, arguments, signature.metadata.roles)
            }
            Callee::Indirect {
                forward_caller: _,
                value,
                arguments,
            } => {
                let TypeKind::Function(function) =
                    self.t.row(self.place_type(value.place)).kind.clone()
                else {
                    return;
                };
                (function, arguments, hirsel_types::NominalRoles::default())
            }
        };
        let store = self.t;
        for value in values {
            let root = hirsel_types::generic_crossing_parameter(
                store,
                self.memo,
                &function,
                value.parameter.0 as usize,
                arguments,
                &roles,
            );
            if let Ok(Some(root)) = root {
                if matches!(value.value.usage, UseKind::Take | UseKind::Copy)
                    && (state.nested_shared
                        [self.body.places[value.value.place.0 as usize].root.0 as usize]
                        || values.iter().any(|v| v.value.usage == UseKind::Copy))
                    && self.has_unique_parts(store, root)
                {
                    let incoming = self.copied_with(&value.value, values.iter().map(|v| &v.value));
                    self.nested_unique_transfer(state, store, root, &incoming, report);
                }
                self.nested_crossing_receipt(state, store, root, &value.value, report);
            }
        }
    }
    pub(super) fn operand_crossing_parts(&self, state: &State, value: &Operand) -> crossing::Parts {
        let access = &self.body.places[value.place.0 as usize];
        let mut parts = state.crossing_parts[access.root.0 as usize]
            .project(&crossing::path(&access.projection));
        let store = self.t;
        let transferred = self.transferred_type(state, value, store);
        let evidence = &store.get(transferred).unwrap().modes.sendability;
        if *evidence == Sendability::Sendable {
            parts.set(&[], true);
        } else if value.usage == UseKind::Copy {
            let mut referent = value.clone();
            referent.usage = UseKind::Borrow;
            let current = self.transferred_type(state, &referent, store);
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
        if !self.has_crossing_receipt(self.t, ty) {
            parts.set(&[], false);
            return;
        }
        let stable: Vec<_> = self
            .crossing_receipts(self.t, ty)
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
                    let row = self.t.get(id).unwrap();
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
                matches!(self.t.get(id).unwrap().kind, TypeKind::Function(_))
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
            state
                .must
                .set_fact(place.root.0 as usize, MustFacts::SENDABLE, sendable);
        } else if !sendable {
            state
                .must
                .set_fact(place.root.0 as usize, MustFacts::SENDABLE, false);
        }
    }

    fn copied_with<'a>(
        &self,
        value: &Operand,
        mut others: impl Iterator<Item = &'a Operand>,
    ) -> Operand {
        let mut incoming = value.clone();
        if !matches!(value.usage, UseKind::Take | UseKind::Copy) {
            return incoming;
        }
        let access = &self.body.places[value.place.0 as usize];
        if others.any(|other| {
            let source = &self.body.places[other.place.0 as usize];
            other.usage == UseKind::Copy
                && access.root == source.root
                && (access.projection.starts_with(&source.projection)
                    || source.projection.starts_with(&access.projection))
                && !super::handle_root(self.t, self.place_type(other.place))
                && !self.flat_value(other.place)
        }) {
            incoming.usage = UseKind::Copy;
        }
        incoming
    }

    fn local_value_transport(&self, expected: TypeId) -> bool {
        let (Some(block), Some(instruction)) = (self.site.block, self.site.instruction) else {
            return false;
        };
        let InstructionKind::Assign { destination, value } =
            self.body.blocks[block.0 as usize].instructions[instruction as usize].kind
        else {
            return false;
        };
        let place = &self.body.places[destination.0 as usize];
        matches!(self.body.rvalues[value.0 as usize], Rvalue::Use(_))
            && place.projection.is_empty()
            && place.ty == expected
            && matches!(
                self.body.locals[place.root.0 as usize].storage,
                Storage::Temporary | Storage::Local
            )
    }

    pub(super) fn has_unique_parts(&self, store: &TypeInterner, root: TypeId) -> bool {
        if let Some(required) = self.unique_parts_cache.borrow().get(&root) {
            return *required;
        }
        match hirsel_types::owned_unique_parts(
            store,
            root,
            &self.generic_declarations(),
            &mut |p| Ok(self.signatures.get(p).cloned().map(std::sync::Arc::new)),
        ) {
            Ok(parts) => {
                let required = !parts.is_empty();
                self.unique_parts_cache.borrow_mut().insert(root, required);
                required
            }
            Err(error) => self.proof_failure(error, false),
        }
    }

    pub(super) fn rvalue_shares_unique_parts(
        &self,
        state: &State,
        destination: PlaceId,
        value: &Rvalue,
    ) -> bool {
        let Rvalue::Use(value) = value else {
            return false;
        };
        let source = &self.body.places[value.place.0 as usize];
        if !(value.usage == UseKind::Copy || state.nested_shared[source.root.0 as usize])
            || !self.has_unique_parts(self.t, self.place_type(destination))
            || self.flat_value(value.place)
            || super::handle_root(self.t, self.place_type(value.place))
        {
            return false;
        }
        true
    }

    fn nested_unique_transfer(
        &mut self,
        state: &State,
        store: &TypeInterner,
        destination: TypeId,
        value: &Operand,
        report: bool,
    ) {
        let access = &self.body.places[value.place.0 as usize];
        // Borrowed pattern transports retain a view of the referent's type;
        // only an owned receiving position exports its payload contracts.
        if !matches!(value.usage, UseKind::Take | UseKind::Copy)
            || !(state.nested_shared[access.root.0 as usize] || value.usage == UseKind::Copy)
            || !self.has_unique_parts(store, destination)
            || self.flat_value(value.place)
            || super::handle_root(self.t, self.place_type(value.place))
            // Copy also stages borrowed matches in environment-local slots.
            // Such a slot retains the lost proof; calls, fields and returns
            // still check it before exporting any owned payload contract.
            || self.local_value_transport(destination)
            || self.site.instruction.is_none()
                && self.site.block.is_some_and(|block| {
                    matches!(&self.body.blocks[block.0 as usize].terminator,
                        Terminator::Call { callee, .. } if self.consuming_protocol(callee))
                })
        {
            return;
        }
        self.flow_error(
            report,
            ViolationKind::TypeMismatch {
                entity: "value modes",
            },
        );
    }

    pub(super) fn mode_transfer(
        &mut self,
        state: &State,
        destination: TypeId,
        value: &Operand,
        report: bool,
    ) {
        let store = self.t;
        self.mode_transfer_in(state, store, destination, value, false, report);
    }
    fn mode_transfer_in(
        &mut self,
        state: &State,
        store: &TypeInterner,
        destination: TypeId,
        value: &Operand,
        borrowed_capture: bool,
        report: bool,
    ) {
        self.nested_unique_transfer(state, store, destination, value, report);
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
            actual = store.intern(row);
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
                let store = self.t;
                if let Some(parts) = self.constructor_layout(store, ty, constructor, fields.len()) {
                    for field in fields {
                        if let Some(expected) = parts.get(field.field as usize).copied() {
                            if matches!(field.value.usage, UseKind::Take | UseKind::Copy)
                                && (state.nested_shared[self.body.places
                                    [field.value.place.0 as usize]
                                    .root
                                    .0
                                    as usize]
                                    || fields.iter().any(|f| f.value.usage == UseKind::Copy))
                                && self.has_unique_parts(store, expected)
                            {
                                let incoming =
                                    self.copied_with(&field.value, fields.iter().map(|f| &f.value));
                                self.nested_unique_transfer(
                                    state, store, expected, &incoming, report,
                                );
                            }
                            if store.get(expected).unwrap().sendability_requirement
                                == hirsel_types::SendabilityRequirement::Sendable
                            {
                                let actual = self.transferred_type(state, &field.value, store);
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
        let row = self.t.get(ty).unwrap();
        if let (Sendability::DependsOn(_), Rvalue::Closure { captures, .. }) =
            (&row.modes.sendability, rvalue)
        {
            if row.modes.sendability != self.closure_sendability(Some(state), captures) {
                self.flow_error(
                    report,
                    ViolationKind::TypeMismatch {
                        entity: "closure sendability",
                    },
                );
            }
            if report {
                self.scratch.closure_receipts.borrow_mut().remove(&(
                    self.site.body.0,
                    self.site.block.map(|block| block.0),
                    self.site.instruction,
                ));
            }
        }
        if let (Sendability::DependsOn(promised), Rvalue::Use(value)) =
            (&row.modes.sendability, rvalue)
            && erased_environment(&row.kind)
        {
            let store = self.t;
            let actual = self.transferred_type(state, value, store);
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
                let store = self.t;
                {
                    let expected = root;
                    // A capture's positive claim requires current content,
                    // even when its storage row still has an old proof.
                    let claimed = store.get(expected).unwrap();
                    if value.usage == UseKind::Take
                        && claimed.modes.sendability == Sendability::Sendable
                        && !erased_environment(&claimed.kind)
                    {
                        let current = self.transferred_type(state, value, store);
                        if store.get(current).unwrap().modes.sendability != Sendability::Sendable {
                            self.flow_error(
                                report,
                                ViolationKind::TypeMismatch {
                                    entity: "closure capture",
                                },
                            );
                        }
                    }
                    self.mode_transfer_in(
                        state,
                        store,
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
                if self.t.get(ty).unwrap().modes.uniqueness == Uniqueness::Unique =>
            {
                let values: Vec<_> = match rvalue {
                    Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
                    Rvalue::Closure { captures, .. } => captures.iter().collect(),
                    _ => unreachable!(),
                };
                for value in values {
                    let store = self.t;
                    let actual = self.transferred_type(state, value, store);
                    let counted = needs_release(self, self.place_type(value.place), self.t);
                    let shared = counted
                        && matches!(
                            self.layout(store, self.place_type(value.place)),
                            Some(TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::Shared
                                    | hirsel_types::BuiltinType::Mutex
                                    | hirsel_types::BuiltinType::Atomic,
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
    pub(crate) fn unique_type(&self, store: &TypeInterner, ty: TypeId) -> TypeId {
        let mut row = store.get(ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let root = update_type(store, ty, row);
        let mut row = store.get(root).unwrap().clone();
        row.modes.sendability = self
            .proof(
                store,
                root,
                super::proofs::Proof::Sendability,
                &self.generic_declarations(),
            )
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
        update_type(store, root, row)
    }
    pub(super) fn operand_fresh_receipt(&self, state: &State, value: &Operand) -> bool {
        let place = &self.body.places[value.place.0 as usize];
        value.usage == UseKind::Take
            && !state.shared[place.root.0 as usize]
            && state.must[place.root.0 as usize].has(MustFacts::OWNED)
            // Only the whole receipt is fresh. A part reached through an
            // unqualified wrapper may share its declared unique descendants
            // with a counted copy made before the receipt, so a part must
            // prove its own uniqueness.
            && ((place.projection.is_empty()
                && state.must[place.root.0 as usize].has(MustFacts::FRESH_RECEIPT))
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
            state
                .must
                .set_fact(place.root.0 as usize, MustFacts::FRESH_RECEIPT, fresh);
        }
    }
    pub(super) fn operand_unique(&self, state: &State, value: &Operand) -> bool {
        if !needs_release(self, self.place_type(value.place), self.t) {
            return false;
        }
        if value.usage == UseKind::Copy {
            return self.flat_value(value.place);
        }
        let place = &self.body.places[value.place.0 as usize];
        let root = place.root.0 as usize;
        if !place.projection.is_empty() {
            // A declared unique payload may move from an unshared owned
            // receipt when every part above it is declared unique too.
            // Otherwise the parent must prove deep uniqueness; neither proof
            // survives a counted share or a Shared boundary. Exclusive access
            // to such a part is unique access too (MODE-2).
            return matches!(value.usage, UseKind::Take | UseKind::Mut)
                && !state.shared[root]
                && (state.current_unique[root]
                    || state.unique_parts[root].get(&crossing::path(&place.projection))
                        == Some(true)
                    || state.must[root].has(MustFacts::OWNED)
                        && state.must[root].has(MustFacts::FRESH_RECEIPT)
                        && self.t.get(place.ty).unwrap().modes.uniqueness == Uniqueness::Unique
                        && self.scratch.receipt_paths[self.site.body.0 as usize]
                            .get()
                            .is_some_and(|paths| paths[value.place.0 as usize]))
                && !matches!(
                    self.t.get(place.ty).unwrap().kind,
                    TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::Shared,
                        ..
                    }
                );
        }
        !state.shared[root]
            && (state.current_unique[root]
                || self
                    .t
                    .get(self.place_type(value.place))
                    .unwrap()
                    .modes
                    .uniqueness
                    == Uniqueness::Unique)
    }
    pub(super) fn rvalue_unique(&self, state: &State, value: &Rvalue) -> bool {
        match value {
            Rvalue::Constant(_) | Rvalue::Size(_) | Rvalue::Shape(_) | Rvalue::FunctionReference { .. } => true,
            Rvalue::ReadConstant { definition, .. } => self.signatures.get(definition).is_some_and(|s| matches!(s.kind, SignatureKind::Const { ty: ty, .. } if self.bound(self.t, ty, hirsel_types::Bound::Plain, &hirsel_types::GenericDeclarations::default()) || self.bound(self.t, ty, hirsel_types::Bound::Flat, &hirsel_types::GenericDeclarations::default()) || matches!(&self.t.get(ty).unwrap().kind, TypeKind::Builtin { kind: hirsel_types::BuiltinType::Shared, .. }))),
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
                !needs_release(self, self.place_type(f.value.place), self.t)
                    || self.operand_unique(state, &f.value)
            }),
            Rvalue::Closure { captures, .. } => captures.iter().all(|v| {
                !needs_release(self, self.place_type(v.place), self.t)
                    || v.usage == UseKind::Take && self.operand_unique(state, v)
                    || v.usage == UseKind::Copy && self.flat_value(v.place)
            }),
            _ => false,
        }
    }
    pub(super) fn set_current_unique(&self, state: &mut State, destination: PlaceId, unique: bool) {
        let place = &self.body.places[destination.0 as usize];
        if place.projection.is_empty() {
            state.current_unique.set(place.root.0 as usize, unique);
        } else if needs_release(self, place.ty, self.t)
            && !matches!(
                self.t.get(place.ty).unwrap().kind,
                hirsel_types::TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Shared,
                    ..
                }
            )
        {
            // Plain bits introduce no counted aliases; Shared is an explicit
            // boundary of the parent's uniquely owned graph (MODE-2).
            state.current_unique.set(
                place.root.0 as usize,
                state.current_unique[place.root.0 as usize] & unique,
            );
        }
    }
    pub(super) fn reset_unique(&self, state: &mut State, destination: PlaceId) {
        let place = &self.body.places[destination.0 as usize];
        if place.projection.is_empty() {
            state.shared.set(place.root.0 as usize, false);
            state.nested_shared.set(place.root.0 as usize, false);
            state.borrow_only.set(place.root.0 as usize, false);
        }
    }
    pub(super) fn mut_requires_unique(
        &self,
        callee: &Callee,
        parameter: hirsel_types::ParamId,
    ) -> bool {
        match callee {
            Callee::Definition { path, .. } => self.signatures.get(path).is_some_and(|s| matches!(&s.kind, SignatureKind::Function(f) if f.parameters.get(parameter.0 as usize).is_some_and(|p| self.t.get(p.ty).unwrap().modes.uniqueness == Uniqueness::Unique))),
            Callee::Indirect { value, .. } => matches!(&self.t.get(self.place_type(value.place)).unwrap().kind, TypeKind::Function(f) if f.parameters.get(parameter.0 as usize).is_some_and(|p| self.t.get(p.ty).unwrap().modes.uniqueness == Uniqueness::Unique)),
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

impl Checker<'_> {
    pub(super) fn operand_unique_parts(&self, state: &State, value: &Operand) -> crossing::Parts {
        let place = &self.body.places[value.place.0 as usize];
        if value.usage == UseKind::Copy || state.shared[place.root.0 as usize] {
            return crossing::Parts::default();
        }
        let mut parts =
            state.unique_parts[place.root.0 as usize].project(&crossing::path(&place.projection));
        if self.operand_unique(state, value) {
            parts.set(&[], true);
        }
        parts
    }
    pub(super) fn rvalue_unique_parts(
        &self,
        state: &State,
        destination: PlaceId,
        value: &Rvalue,
    ) -> crossing::Parts {
        match value {
            Rvalue::Use(value) => self.operand_unique_parts(state, value),
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                let mut parts = crossing::Parts::default();
                if let Constructor::Variant { variant, .. } = constructor {
                    let roles = self
                        .signatures
                        .get(self.definition.path)
                        .map(|s| s.metadata.roles.clone())
                        .unwrap_or_default();
                    for (row, _) in hirsel_types::unique_summary::result_rows(
                        self.t,
                        self.place_type(destination),
                        &roles,
                        &mut |d| self.signatures.get(d).cloned().map(std::sync::Arc::new),
                    ) {
                        if let hirsel_types::UniqueResultRow::Payload {
                            variant: other,
                            field,
                        } = row
                            && other != *variant
                        {
                            parts.set(&[crossing::Part::Variant(other, field)], true);
                        }
                    }
                }
                for field in fields {
                    let prefix = match constructor {
                        Constructor::Variant { variant, .. } => {
                            vec![crossing::Part::Variant(*variant, field.field)]
                        }
                        Constructor::Sequence => vec![crossing::Part::Elements],
                        _ => vec![crossing::Part::Field(field.field)],
                    };
                    let mut incoming = self.operand_unique_parts(state, &field.value);
                    if !needs_release(self, self.place_type(field.value.place), self.t) {
                        incoming.set(&[], true);
                    }
                    parts.install(&prefix, &incoming);
                }
                parts
            }
            _ => crossing::Parts::default(),
        }
    }
    pub(super) fn install_unique_parts(
        &self,
        state: &mut State,
        destination: PlaceId,
        parts: &crossing::Parts,
        depth: usize,
    ) {
        let place = &self.body.places[destination.0 as usize];
        let prefix = crossing::path(&place.projection);
        if prefix.is_empty() {
            state.unique_parts[place.root.0 as usize] = parts.clone();
        } else {
            // Replacing a part can invalidate an ancestor's complete proof.
            state.unique_parts[place.root.0 as usize] = crossing::Parts::default();
            state.unique_parts[place.root.0 as usize].install(&prefix, parts);
        }
        state.unique_parts[place.root.0 as usize].restrict(depth);
    }
}
