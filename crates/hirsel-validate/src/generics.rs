//! Generic contracts are conditional proofs; every instantiation discharges them.
use super::*;
use hirsel_types::*;

pub(super) fn children(kind: &TypeKind) -> Vec<TypeId> {
    match kind {
        TypeKind::Tuple(ts) => ts.to_vec(),
        TypeKind::Record(fs) => fs.iter().map(|f| f.ty).collect(),
        TypeKind::Variant(vs) => vs
            .iter()
            .flat_map(|v| v.fields.iter().map(|f| f.1))
            .collect(),
        TypeKind::Box(t) => vec![*t],
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => arguments
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                _ => None,
            })
            .collect(),
        TypeKind::Function(f) => f
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain([f.result])
            .chain(f.error)
            .collect(),
        _ => vec![],
    }
}
fn implies(bounds: &[Bound], bound: Bound) -> bool {
    bounds.contains(&bound) || (bound == Bound::NoallocDrop && bounds.contains(&Bound::Plain))
}
impl Checker<'_, '_> {
    pub(super) fn table_key_arguments(
        &mut self,
        path: &hirsel_syntax::DefPath,
        arguments: &[GenericArgument],
    ) {
        for &position in core_roles::table_key_parameters(path) {
            let valid = arguments.get(position).is_some_and(|argument| {
                let GenericArgument::Type(root) = argument else {
                    return false;
                };
                super::serialization::ordered_key(
                    self.t,
                    self.memo,
                    *root,
                    &self.generic_declarations(),
                    self.signatures,
                )
                .unwrap_or_else(|error| self.proof_failure(error, false))
            });
            if !valid {
                self.error(ViolationKind::TypeMismatch {
                    entity: "table key schema",
                });
            }
        }
    }

    /// A copy of this row is an independent global value: plain bits and
    /// `Shared`, `Mutex` or `Atomic` handles only (MEM-11, CONC-2).
    pub(super) fn copy_detaches(&self, source: &TypeInterner, root: TypeId) -> bool {
        self.holds(
            source,
            root,
            super::proofs::Proof::Detaches,
            self.generic_declarations(),
        )
        .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    pub(super) fn bound(
        &self,
        source: &TypeInterner,
        root: TypeId,
        bound: Bound,
        generics: &hirsel_types::GenericDeclarations,
    ) -> bool {
        if bound == Bound::Serializable {
            return super::serialization::eligible(
                source,
                self.memo,
                root,
                generics,
                self.signatures,
            )
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        if matches!(bound, Bound::Data | Bound::Key) {
            return self.proven(
                source,
                root,
                super::proofs::Proof::Equality(bound),
                generics,
            );
        }
        if let Some(query) = match bound {
            Bound::Plain => Some(super::proofs::Proof::Plain),
            Bound::Copyable => Some(super::proofs::Proof::Copyable),
            Bound::Sendable => Some(super::proofs::Proof::Sendable),
            _ => None,
        } {
            return self.proven(source, root, query, generics);
        }
        if bound == Bound::Flat {
            return self.proven(source, root, super::proofs::Proof::Flat, generics);
        }
        if bound == Bound::Inert {
            return self.authority_with_generics(source, root, generics)
                == AuthoritySummary::inert();
        }
        debug_assert_eq!(bound, Bound::NoallocDrop);
        let store = source;
        let mut pending = vec![(root, false)];
        let mut seen = HashSet::default();
        let mut nominals = NominalExpansions::default();
        while let Some((t, exit)) = pending.pop() {
            if exit {
                nominals.leave(t, 0);
                continue;
            }
            if !seen.insert(t) {
                continue;
            }
            let Some(row) = store.get(t).cloned() else {
                return false;
            };
            match row.kind.clone() {
                TypeKind::Error => return false,
                TypeKind::Parameter(p) => {
                    if !matches!(generics.get(p), Some(GenericParameter::Type { bounds, .. }) if implies(bounds, bound))
                    {
                        return false;
                    }
                }
                TypeKind::Primitive(_) => (),
                TypeKind::Function(_) => {
                    // The callable cleanup promise is distinct from invocation allocation;
                    // local closure producers independently prove it from owned captures.
                    if !self.drop_bound(&row.drop_allocation, generics) {
                        return false;
                    }
                }
                TypeKind::Box(child) => {
                    pending.push((child, false));
                }
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let Some(signature) = self.signatures.get(definition) else {
                        return false;
                    };
                    let SignatureKind::Type(d) = &signature.kind else {
                        return false;
                    };
                    if row.is_heap_target(&signature.metadata.roles) {
                        continue;
                    }
                    if d.resource && d.destructor_allocation != Allocation::NoAlloc {
                        return false;
                    }
                    if implies(&d.bounds, bound) {
                        continue;
                    }
                    if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                        return false;
                    }
                    if !nominals
                        .enter(t, 0, self.t)
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    {
                        continue;
                    }
                    pending.push((t, true));
                    let Some(rep) = d.representation else {
                        return false;
                    };
                    let Ok(rep) = hirsel_types::substitute(
                        self.t,
                        self.memo,
                        rep,
                        &Substitution {
                            binder: Some(hirsel_types::GenericBinder::Bound(0)),
                            arguments: &arguments,
                            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            roles: &signature.metadata.roles,
                        },
                    ) else {
                        return false;
                    };
                    pending.push((rep, false));
                }
                TypeKind::Builtin { kind, arguments } => {
                    if matches!(
                        kind,
                        BuiltinType::Ptr
                            | BuiltinType::FnPtr
                            | BuiltinType::SlotId
                            | BuiltinType::Region
                            | BuiltinType::TypeShape
                    ) {
                        continue;
                    }
                    if kind == BuiltinType::Iter {
                        if !self.drop_bound(&row.drop_allocation, generics) {
                            return false;
                        }
                        continue;
                    }
                    if kind == BuiltinType::SortedMap {
                        return false;
                    }
                    pending.extend(arguments.iter().filter_map(|a| match a {
                        GenericArgument::Type(t) => Some((*t, false)),
                        _ => None,
                    }));
                }
                kind => pending.extend(children(&kind).into_iter().map(|t| (t, false))),
            }
        }
        true
    }
    fn drop_bound(
        &self,
        summary: &DropAllocation,
        generics: &hirsel_types::GenericDeclarations,
    ) -> bool {
        match summary {
            DropAllocation::NoAlloc => true,
            DropAllocation::MayAllocate => false,
            DropAllocation::DependsOn(ps) => ps.iter().all(|p| matches!(generics.get(*p), Some(GenericParameter::Type { bounds, .. }) if implies(bounds, Bound::NoallocDrop))),
        }
    }
    // Reconstruct local callback evidence from its producer and reachable return
    // values. A contextual callable result promise may already have weakened it.
    fn complete_callable(&self, place: PlaceId, target: &TypeInterner) -> Vec<TypeId> {
        fn producers(
            body: &ReadBody<'_>,
            root: hirsel_tir::LocalId,
            active: &mut BTreeSet<u32>,
        ) -> Option<BTreeSet<u32>> {
            if !active.insert(root.0) {
                return None;
            }
            let mut found = BTreeSet::new();
            let mut known = false;
            for block in &body.blocks {
                for instruction in &block.instructions {
                    let value = match &instruction.kind {
                        InstructionKind::Assign { destination, value }
                            if body.places[destination.0 as usize].root == root =>
                        {
                            Some(&body.rvalues[value.0 as usize])
                        }
                        _ => None,
                    };
                    match value {
                        Some(Rvalue::Closure { body, .. }) => {
                            found.insert(body.0);
                            known = true;
                        }
                        Some(Rvalue::Use(value)) => {
                            found.extend(producers(
                                body,
                                body.places[value.place.0 as usize].root,
                                active,
                            )?);
                            known = true;
                        }
                        Some(_) => return None,
                        None => (),
                    }
                }
                if matches!(&block.terminator, Terminator::Call { returns: Some(CallReturn { destination, .. }), .. } if body.places[destination.0 as usize].root == root)
                {
                    return None;
                }
            }
            active.remove(&root.0);
            known.then_some(found)
        }
        let original = self.place_type(place);
        let TypeKind::Function(_) = &target.get(original).unwrap().kind else {
            return vec![original];
        };
        let root = self.body.places[place.0 as usize].root;
        let Some(producers) = producers(self.body, root, &mut BTreeSet::new()) else {
            return vec![original];
        };
        let mut evidence = vec![];
        for id in producers {
            let body = &self.definition.bodies[id as usize];
            let mut checker = Checker {
                definition: self.definition,
                body,
                t: self.t,
                memo: self.memo,
                signatures: self.signatures,
                declarations: self.declarations,
                scratch: self.scratch,
                contract: self.contract,
                site: Site {
                    body: BodyId(id),
                    block: None,
                    instruction: None,
                },
                errors: vec![],
                captured: BTreeMap::new(),
                returned: BTreeMap::new(),
                static_data: None,
                proof_errors: std::cell::RefCell::new(Vec::new()),
                unique_parts_cache: Default::default(),
            };
            checker.flow(&[], false);
            self.proof_errors
                .borrow_mut()
                .extend(checker.proof_errors.into_inner());
            for result in checker.returned.into_values() {
                let result = target.intern(result);
                let mut row = target.get(original).unwrap().clone();
                let TypeKind::Function(f) = &mut row.kind else {
                    unreachable!()
                };
                f.result = result;
                evidence.push(target.intern(row));
            }
        }
        if evidence.is_empty() {
            vec![original]
        } else {
            evidence
        }
    }
    pub(super) fn call_bounds(
        &mut self,
        source: &TypeInterner,
        f: &FunctionType,
        args: &[GenericArgument],
        values: &[CallArgument],
        flow_proof: Option<&super::flow_origins::CallOriginProof>,
    ) {
        fn occurrences(
            store: &TypeInterner,
            t: TypeId,
            parameter: TypeParamId,
            depth: u32,
            active: &mut HashSet<TypeId>,
        ) -> Vec<Vec<usize>> {
            if !active.insert(t) {
                return vec![];
            }
            let paths = if store.get(t).is_some_and(|row| {
                row.kind == TypeKind::Parameter(TypeParameterRef::Bound { depth, parameter })
            }) {
                vec![vec![]]
            } else {
                children(&store.get(t).unwrap().kind)
                    .iter()
                    .enumerate()
                    .flat_map(|(i, child)| {
                        occurrences(
                            store,
                            *child,
                            parameter,
                            depth
                                + u32::from(matches!(
                                    store.get(t).unwrap().kind,
                                    TypeKind::Function(_)
                                )),
                            active,
                        )
                        .into_iter()
                        .map(move |mut path| {
                            path.insert(0, i);
                            path
                        })
                    })
                    .collect()
            };
            active.remove(&t);
            paths
        }
        let actual_store = self.t;
        for parameter in sharing_type_parameters(source, f) {
            if let Some(GenericArgument::Type(actual)) = args.get(parameter.0 as usize) {
                if has_owned_crossing_requirement(
                    actual_store,
                    *actual,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, true))
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "generic crossing transport",
                    });
                }
            }
        }
        for (ordinal, (parameter, argument)) in f.generics.iter().zip(args).enumerate() {
            let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(fallback)) =
                (parameter, argument)
            else {
                continue;
            };
            let mut evidence = vec![];
            let mut malformed = false;
            for value in values {
                let Some(formal) = f.parameters.get(value.parameter.0 as usize) else {
                    continue;
                };
                for path in occurrences(
                    source,
                    formal.ty,
                    TypeParamId(ordinal as u32),
                    0,
                    &mut HashSet::default(),
                ) {
                    let roots = self.complete_callable(value.value.place, actual_store);
                    for root in roots {
                        let root = if flow_proof
                            .is_some_and(|p| p.current_unique.contains(&value.value.place.0))
                        {
                            self.unique_type(actual_store, root)
                        } else {
                            root
                        };
                        let root = if flow_proof
                            .is_some_and(|p| p.weakened.contains(&value.value.place.0))
                        {
                            self.weakened_type(actual_store, root, &self.generic_declarations())
                        } else {
                            root
                        };
                        let mut actual = Some(root);
                        for edge in &path {
                            actual = actual.and_then(|t| {
                                children(&actual_store.get(t)?.kind).get(*edge).copied()
                            });
                        }
                        match actual {
                            Some(t) => evidence.push(t),
                            None => malformed = true,
                        }
                    }
                }
            }
            if evidence.is_empty() {
                evidence.push(*fallback);
            }
            for bound in bounds {
                if malformed
                    || evidence.iter().any(|t| {
                        if *bound == Bound::Sendable {
                            self.proof(
                                actual_store,
                                *t,
                                super::proofs::Proof::Content,
                                &self.generic_declarations(),
                            )
                            .unwrap_or_else(|error| {
                                self.proof_failure(error, Sendability::NotSendable)
                            }) != Sendability::Sendable
                        } else {
                            !self.bound(actual_store, *t, *bound, &self.generic_declarations())
                        }
                    })
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "generic bound",
                    });
                }
            }
        }
    }
    pub(super) fn nominal_bounds(&mut self) {
        let context = self.generic_declarations();
        // A function actual can disappear completely during substitution (for
        // example, a parameter absent from the value signature). Validate its
        // declaration identity before relying on the instantiated callable's
        // types; an empty bound list still requires a well-formed actual.
        if self.body.rvalues.iter().any(|value| {
            matches!(value, Rvalue::FunctionReference { arguments, .. }
            if arguments.iter().any(|argument| {
                matches!(argument, GenericArgument::Type(actual)
                    if !generic_references_valid(self.t, *actual, &context))
            }))
        }) {
            self.error(ViolationKind::InvalidTypeGraph);
        }
        let mut pending: Vec<_> = self
            .body
            .locals
            .iter()
            .map(|l| (l.ty, context.clone()))
            .chain([(self.body.result, context.clone())])
            .collect();
        let mut seen = vec![];
        while let Some((t, context)) = pending.pop() {
            if seen.contains(&(t, context.clone())) {
                continue;
            }
            seen.push((t, context.clone()));
            let kind = self.t.get(t).unwrap().kind.clone();
            if let TypeKind::Named {
                definition,
                arguments,
            } = &kind
            {
                if let Some(signature) = self.signatures.get(*definition).cloned() {
                    let declared = signature.generics();
                    if declared.len() != arguments.len() {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "nominal generic arguments",
                        });
                    }
                    for (p, a) in declared.iter().zip(arguments) {
                        match (p, a) {
                            (
                                GenericParameter::Type { bounds, .. },
                                GenericArgument::Type(actual),
                            ) => {
                                if bounds
                                    .iter()
                                    .any(|b| !self.bound(self.t, *actual, *b, &context))
                                {
                                    self.error(ViolationKind::TypeMismatch {
                                        entity: "nominal generic bound",
                                    });
                                }
                            }
                            (GenericParameter::Size { .. }, GenericArgument::Size(_)) => (),
                            _ => self.error(ViolationKind::TypeMismatch {
                                entity: "nominal generic argument kind",
                            }),
                        }
                    }
                }
            }
            if let TypeKind::Function(f) = &kind {
                let inner = context.nested(&f.generics);
                pending.extend(
                    f.parameters
                        .iter()
                        .map(|p| p.ty)
                        .chain([f.result])
                        .chain(f.error)
                        .map(|t| (t, inner.clone())),
                );
            } else {
                pending.extend(children(&kind).into_iter().map(|t| (t, context.clone())));
            }
        }
    }
}
