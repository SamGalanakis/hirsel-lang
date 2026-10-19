use super::*;

impl Checker<'_> {
    /// CAP-2 charges a branded resource at use. Derive this from declarations,
    /// including new core resources, rather than maintaining a handle-name list.
    pub(super) fn capability_call_rule(
        &mut self,
        intrinsic: Option<Intrinsic>,
        f: &FunctionType,
        missing: &BTreeSet<ParameterRef>,
    ) -> RuleId {
        match intrinsic {
            Some(Intrinsic::UnsafeOperation) => const { RuleId::of("MEM-9") },
            Some(
                Intrinsic::Synchronization
                | Intrinsic::MutexCreate
                | Intrinsic::MutexWith
                | Intrinsic::AtomicCreate,
            ) => const { RuleId::of("CAP-4") },
            Some(
                Intrinsic::HandleOperation
                | Intrinsic::ChannelSend
                | Intrinsic::TaskAwait
                | Intrinsic::TaskDetach,
            ) => const { RuleId::of("CAP-2") },
            _ => {
                let branded_resource = f.parameters.iter().any(|p| {
                    let Some(row) = self.out.body.types.get(p.ty) else {
                        return false;
                    };
                    // A resource charges CAP-2 only for its governing argument.
                    // An unrelated missing capability remains CAP-1 even when
                    // the function also accepts a branded resource.
                    if !row.brand.alternatives.iter().any(|origin| {
                        matches!(&origin.root,
                            OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p)
                                if missing.contains(p))
                    }) {
                        return false;
                    }
                    let TypeKind::Named { definition, .. } = &row.kind else {
                        return false;
                    };
                    let definition = definition.clone();
                    self.read(&definition)
                        .is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.resource))
                });
                if branded_resource {
                    const { RuleId::of("CAP-2") }
                } else {
                    const { RuleId::of("CAP-1") }
                }
            }
        }
    }
    pub(super) fn capability_definition(&mut self, ty: TypeId) -> Option<DefPath> {
        let TypeKind::Named { definition, .. } = self.out.body.types.get(ty)?.kind.clone() else {
            return None;
        };
        let signature = self.read(&definition)?;
        matches!(&signature.kind, SignatureKind::Type(t) if t.capability.is_some())
            .then_some(definition)
    }
    pub(super) fn scoped_authority(&mut self, summary: AuthoritySummary) -> AuthoritySummary {
        let store = self.out.body.types.clone();
        self.scoped_authority_in(&store, summary)
    }
    /// `scoped_authority`, reading the body's parameter rows from `store`,
    /// which holds every row of the body's store.
    pub(super) fn scoped_authority_in(
        &mut self,
        store: &TypeStore,
        summary: AuthoritySummary,
    ) -> AuthoritySummary {
        fn expand(
            checker: &mut Checker<'_>,
            store: &TypeStore,
            summary: AuthoritySummary,
            active: &mut BTreeSet<ParameterRef>,
        ) -> AuthoritySummary {
            let AuthoritySummary::Known(terms) = summary else {
                return AuthoritySummary::Unknown;
            };
            terms
                .into_iter()
                .fold(AuthoritySummary::inert(), |a, term| {
                    let next = if let AuthorityTerm::Parameter(
                        p @ ParameterRef::Body { scope, parameter },
                    ) = term
                    {
                        if !active.insert(p) {
                            return AuthoritySummary::Unknown;
                        }
                        let ty = checker
                            .out
                            .body
                            .bindings
                            .iter()
                            .find(|b| {
                                b.owner == scope && b.binding == ResolvedName::Parameter(parameter)
                            })
                            .map(|b| b.ty);
                        let summary = ty
                            .map(|t| match &store.get(t).unwrap().kind {
                                TypeKind::Function(f) => f.authority.clone(),
                                _ => checker.raw_authority(store, t),
                            })
                            .unwrap_or(AuthoritySummary::Unknown);
                        // A preservation parameter's self capture names the
                        // incoming envelope, not a recursively captured value.
                        let expanded = if summary
                            == AuthoritySummary::Known(vec![AuthorityTerm::Parameter(p)].into())
                        {
                            summary
                        } else {
                            expand(checker, store, summary, active)
                        };
                        active.remove(&p);
                        expanded
                    } else {
                        AuthoritySummary::Known(vec![term].into())
                    };
                    a.join(next)
                })
        }
        expand(self, store, summary, &mut BTreeSet::new())
    }
    pub(super) fn authority(&mut self, id: TypeId) -> AuthoritySummary {
        self.authority_in(&self.out.body.types.clone(), id)
    }
    fn authority_in(&mut self, store: &TypeStore, id: TypeId) -> AuthoritySummary {
        let summary = self.raw_authority(store, id);
        self.scoped_authority(summary)
    }
    pub(super) fn raw_authority(&mut self, store: &TypeStore, id: TypeId) -> AuthoritySummary {
        let roles = self.signature.metadata.roles.clone();
        let declarations = self.declarations();
        structural_authority(store, id, &roles, &declarations, &mut |_, path| {
            Ok::<_, hirsel_types::QueryError>(path.and_then(|path| self.read(path)))
        })
        .unwrap()
    }
    pub(super) fn contains_resource(&mut self, root: TypeId) -> bool {
        let mut store = self.out.body.types.clone();
        let mut pending = vec![(root, false)];
        let mut seen = BTreeSet::new();
        let mut nominal_roots = hirsel_types::NominalExpansions::default();
        let mut checked_closures = BTreeSet::new();
        while let Some((t, exit)) = pending.pop() {
            if exit {
                nominal_roots.leave(t, 0);
                continue;
            }
            if !seen.insert(t) {
                continue;
            }
            let Some(row) = store.get(t).cloned() else {
                return true;
            };
            match row.kind {
                TypeKind::Error => return true,
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let Some(signature) = self.read(&definition) else {
                        return true;
                    };
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        return true;
                    };
                    if declaration.resource {
                        return true;
                    }
                    if declaration.bounds.iter().any(|b| {
                        matches!(
                            b,
                            Bound::Plain | Bound::Copyable | Bound::Data | Bound::Flat
                        )
                    }) {
                        continue;
                    }
                    if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                        return true;
                    }
                    if !nominal_roots
                        .enter(t, 0, |_| &store)
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    {
                        continue;
                    }
                    pending.push((t, true));
                    let Some(representation) = declaration.representation else {
                        return true;
                    };
                    let representation = if arguments.is_empty() {
                        import_type(&signature.types, representation, &mut store)
                    } else {
                        substitute_type(
                            &signature.types,
                            representation,
                            &mut store,
                            Substitution {
                                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                                roles: &self.signature.metadata.roles,
                                arguments: &arguments,
                                authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            },
                        )
                    };
                    let Ok(representation) = representation else {
                        return true;
                    };
                    pending.push((representation, false));
                }
                TypeKind::Parameter(_) => {
                    if !self.property_in(store.clone(), t, Bound::Copyable, None) {
                        return true;
                    }
                }
                TypeKind::Function(_) => {
                    // Revision 1 erases captures from function types. Known
                    // constructions retain their complete capture layout;
                    // otherwise a once environment has no resource-free proof.
                    let matching: Vec<_> = self
                        .closures
                        .iter()
                        .filter(|closure| {
                            semantic_equal(&store, t, &self.out.body.types, closure.ty)
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    if matching.is_empty() && row.modes.multiplicity == Multiplicity::Once {
                        return true;
                    }
                    for closure in matching {
                        if checked_closures.insert(closure.expression) {
                            for capture in closure.captures {
                                let Ok(t) =
                                    import_type(&self.out.body.types, capture.ty, &mut store)
                                else {
                                    return true;
                                };
                                pending.push((t, false));
                            }
                        }
                    }
                }
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Sender
                        | BuiltinType::Receiver
                        | BuiltinType::Task
                        | BuiltinType::Region
                        | BuiltinType::Iter,
                    ..
                } => return true,
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Shared
                        | BuiltinType::SlotId
                        | BuiltinType::Ptr
                        | BuiltinType::FnPtr,
                    ..
                } => (),
                kind => pending.extend(kind.value_parts().into_iter().rev().map(|t| (t, false))),
            }
        }
        false
    }
    pub(super) fn property(&mut self, id: TypeId, bound: Bound) -> bool {
        self.property_in(self.out.body.types.clone(), id, bound, None)
    }
    fn property_in(
        &mut self,
        store: TypeStore,
        id: TypeId,
        bound: Bound,
        declaration_generics: Option<&GenericDeclarations>,
    ) -> bool {
        if bound == Bound::Serializable {
            let generics = declaration_generics
                .cloned()
                .unwrap_or_else(|| self.declarations());
            return structural_serializable(&store, id, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        let property = match bound {
            Bound::Plain => Some(StructuralProperty::Plain),
            Bound::Copyable => Some(StructuralProperty::Copyable),
            Bound::Sendable => Some(StructuralProperty::Sendable),
            _ => None,
        };
        if let Some(property) = property {
            let generics = declaration_generics
                .cloned()
                .unwrap_or_else(|| self.declarations());
            return structural_property(&store, id, property, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        debug_assert_eq!(bound, Bound::Flat);
        let generics = declaration_generics
            .cloned()
            .unwrap_or_else(|| self.declarations());
        structural_flat(&store, id, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(self.read(path))
        })
        .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    pub(super) fn scoped(&mut self, id: TypeId) -> bool {
        self.scoped_in(self.out.body.types.clone(), id, &mut BTreeSet::new())
    }
    fn scoped_in(&mut self, store: TypeStore, id: TypeId, active: &mut BTreeSet<DefPath>) -> bool {
        // Specialize generic and erased authority before opening a shape. A
        // nominal generic holder must not hide the scoped seal of its actual.
        if let AuthoritySummary::Known(terms) = self.authority_in(&store, id) {
            for term in terms {
                if let AuthorityTerm::Capability(path) = term {
                    if self.read(&path).is_some_and(|s|
                        matches!(&s.kind, SignatureKind::Type(d) if d.capability == Some(CapabilityKind::Scoped))) {
                        return true;
                    }
                }
            }
            return false;
        }
        let Some(t) = store.get(id) else { return false };
        if t.is_heap_target(&self.signature.metadata.roles) {
            return false;
        }
        if let TypeKind::Named {
            definition,
            arguments,
        } = &t.kind
        {
            let Some(s) = self.read(definition) else {
                return false;
            };
            let SignatureKind::Type(d) = &s.kind else {
                return false;
            };
            if d.capability == Some(CapabilityKind::Scoped) {
                return true;
            }
            if !active.insert(definition.clone()) {
                return false;
            }
            // Opaque authority also exposes contained scoped capabilities.
            if let AuthoritySummary::Known(terms) = &d.authority {
                for term in terms {
                    if let AuthorityTerm::Capability(cap) = term {
                        if self.read(cap).is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.capability == Some(CapabilityKind::Scoped))) { active.remove(definition); return true }
                    }
                }
            }
            let r = d
                .representation
                .is_some_and(|r| self.scoped_in(s.types.clone(), r, active))
                || arguments.iter().any(|a| match a {
                    GenericArgument::Type(actual) => self.scoped_in(store.clone(), *actual, active),
                    _ => false,
                });
            active.remove(definition);
            return r;
        }
        t.kind
            .value_parts()
            .into_iter()
            .any(|t| self.scoped_in(store.clone(), t, active))
    }
    pub(super) fn sendable(&mut self, id: TypeId) -> bool {
        self.property(id, Bound::Sendable)
    }
    /// MODE-4 apart from the root's current uniqueness, which I judges from
    /// flow: whether the value crosses once its root is unique.
    pub(super) fn sendable_when_unique(&mut self, id: TypeId) -> bool {
        if self.sendable(id) {
            return true;
        }
        let row = self.out.body.types.get(id).unwrap().clone();
        if row.modes.uniqueness == Uniqueness::Unique || self.property(id, Bound::Plain) {
            return false;
        }
        let mut store = self.out.body.types.clone();
        let mut row = row;
        row.modes.uniqueness = Uniqueness::Unique;
        let unique = store.push(row.clone());
        let declarations = self.declarations();
        row.modes.sendability =
            structural_sendability(&store, unique, &declarations, &mut |path| {
                Ok(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        let root = store.push(row);
        self.property_in(store, root, Bound::Sendable, None)
    }
    /// Whether a row's sendability is evidence about what it captures
    /// (MODE-4: "computed where it is built and is part of its type") rather
    /// than a summary of its parts.
    pub(super) fn carries_crossing_evidence(row: &Type) -> bool {
        matches!(
            row.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        )
    }
    /// MODE-4 for what a value holds: its crossing evidence apart from its
    /// own lifetime, which MODE-1 judges. Placement still counts: an arena
    /// value does not cross.
    fn content_sendable(&mut self, id: TypeId) -> bool {
        let mut store = self.out.body.types.clone();
        let mut row = store.get(id).unwrap().clone();
        row.modes.relation = self.placement(&row.modes.relation);
        let root = store.push(row);
        self.property_in(store, root, Bound::Sendable, None)
    }
    /// A symbolic crossing promise is a conjunction: the actual may require
    /// only dependencies that the receiving type already requires.
    pub(super) fn receives_sendability(&mut self, actual: TypeId, expected: &Sendability) -> bool {
        let row = self.out.body.types.get(actual).unwrap().clone();
        if row.modes.sendability == Sendability::NotSendable
            && row.modes.relation.named_parameters().is_some()
        {
            return false;
        }
        if row.modes.sendability == Sendability::Sendable {
            return self.content_sendable(actual);
        }
        let mut store = self.out.body.types.clone();
        let mut row = row;
        row.modes.relation = self.placement(&row.modes.relation);
        let actual = store.push(row);
        let generics = self.declarations();
        let derived =
            content_sendability(&store, actual, &generics, &mut |path| Ok(self.read(path)))
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        Self::crossing_satisfies(&derived, expected)
    }
    pub(super) fn crossing_satisfies(actual: &Sendability, expected: &Sendability) -> bool {
        match (actual, expected) {
            (_, Sendability::NotSendable) | (Sendability::Sendable, _) => true,
            (Sendability::DependsOn(actual), Sendability::DependsOn(expected)) => {
                actual.iter().all(|p| expected.contains(p))
            }
            _ => false,
        }
    }
    /// Fresh owned storage crosses when each transferred part crosses, even
    /// when its erased environment proof grants no deep-unique permission.
    pub(super) fn construction_sendability(&mut self, expression: ExprId, parts: &[ExprId]) {
        let Some(root) = self.ty(expression) else {
            return;
        };
        let row = self.out.body.types.get(root).unwrap();
        if !row.modes.relation.is_global() {
            return;
        }
        let proved = parts.iter().all(|part| {
            self.construction_part_type(*part)
                .is_some_and(|ty| self.sendable(ty))
        });
        let mut row = self.out.body.types.get(root).unwrap().clone();
        row.modes.sendability = if proved {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        };
        let root = self.out.body.types.push(row);
        self.set_ty(expression, root);
    }
}
