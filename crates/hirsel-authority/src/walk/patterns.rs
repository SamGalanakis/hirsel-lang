use super::*;

#[derive(Clone, Copy)]
enum PatternOwnership {
    Borrowed,
    Owned,
    Unique,
}

impl Checker<'_> {
    pub(super) fn binding(&self, e: ExprId) -> Option<BindingDecision> {
        let name = self.resolved.get(&e)?;
        work::count(Counter::AuthoritySourceRows);
        let scope = self
            .source_rows
            .scopes
            .get(&NameSite::Expression(e))
            .map(|i| self.out.body.resolution.reference_scopes[*i].1);
        let i = self.source_rows.binding(&self.out.body, name, scope)?;
        Some(self.out.body.bindings[i].clone())
    }
    pub(super) fn root_binding(
        &self,
        owner: NameSite,
        name: &ResolvedName,
    ) -> Option<&BindingDecision> {
        let i = self
            .source_rows
            .binding(&self.out.body, name, Some(owner))?;
        Some(&self.out.body.bindings[i])
    }
    pub(super) fn pattern_row(&self, p: hirsel_syntax::PatternId) -> Option<&PatternDecision> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows
            .patterns
            .get(&p)
            .map(|i| &self.out.body.patterns[*i])
    }
    fn pattern_row_mut(&mut self, p: hirsel_syntax::PatternId) -> Option<&mut PatternDecision> {
        work::count(Counter::AuthoritySourceRows);
        let i = *self.source_rows.patterns.get(&p)?;
        self.out.body.patterns.get_mut(i)
    }
    fn binding_site_index(&self, p: NameSite) -> Option<usize> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows.sites.get(&p).copied()
    }
    /// MODE-2 construction owns its parts. Plain bits, flat copies and Shared
    /// boundaries need no exclusive source; other counted parts must be taken
    /// from current unique evidence, not copied from a unique borrowed place.
    pub(super) fn unique_construction_part(&mut self, expression: ExprId) -> bool {
        let Some(ty) = self.ty(expression) else {
            return false;
        };
        if self.property(ty, Bound::Plain) {
            return true;
        }
        let row = self.out.body.types.get(ty).unwrap();
        if matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        ) {
            return true;
        }
        let unique = row.modes.uniqueness == Uniqueness::Unique;
        let source = self.source_place(expression);
        let owned = matches!(source.root, SourceRoot::Value(_)) && source.projections.is_empty()
            || matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || source.projections.is_empty()
                && self
                    .binding(expression)
                    .is_some_and(|binding| binding.usage == UseKind::Take);
        let copying =
            source.access == SourceAccess::Copy || source.access == SourceAccess::Read && !owned;
        if copying && self.property(ty, Bound::Flat) {
            return true;
        }
        unique && owned && !copying
    }
    /// Inferred aggregate parts describe their receiving values. A shared read
    /// cannot retain the source field's exclusive physical storage contract.
    pub(super) fn construction_part_type(&mut self, expression: ExprId) -> Option<TypeId> {
        let ty = self.ty(expression)?;
        if self.unique_construction_part(expression) {
            return Some(ty);
        }
        let source = self.source_place(expression);
        let owned = matches!(source.root, SourceRoot::Value(_)) && source.projections.is_empty()
            || matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || source.projections.is_empty()
                && self
                    .binding(expression)
                    .is_some_and(|binding| binding.usage == UseKind::Take);
        if owned && source.access != SourceAccess::Copy {
            return Some(ty);
        }
        let mut row = self.out.body.types.get(ty)?.clone();
        row.modes.uniqueness = Uniqueness::Aliased;
        let shared = self.out.body.types.push(row.clone());
        let store = self.out.body.types.clone();
        let generics = self.declarations();
        row.modes.sendability =
            sendability_after_sharing(&store, shared, &generics, &mut |path| Ok(self.read(path)))
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        Some(self.out.body.types.push(row))
    }
    pub(super) fn copy_value_type(&mut self, e: ExprId, value: ExprId) {
        if let (Some(_), Some(src)) = (self.ty(e), self.ty(value)) {
            // A value marker preserves the complete occurrence graph, including
            // origins substituted into nominal arguments and aggregate payloads.
            let ty = self.out.body.types.get(src).unwrap().clone();
            let t = self.out.body.types.push(ty);

            if let Some(stale) = self.invalid.get(&src).copied() {
                self.invalid.insert(t, stale);
            }
            self.set_ty(e, t);
        }
    }
    pub(super) fn join_bindings(&mut self, before: &BindingTypes, branch: &BindingTypes) {
        for site in before.changed_keys(&[branch, &self.binding_types]) {
            if let (Some(base), Some(a), Some(b)) = (
                before.get(&site),
                branch.get(&site),
                self.binding_types.get(&site),
            ) {
                let t = self.join_types(*base, &[*a, *b]);
                self.binding_types.insert(site, t);
            }
        }
    }
    pub(super) fn cursor_loop(&mut self, mut plan: CursorPlan, pattern: hirsel_syntax::PatternId) {
        let Some(source) = self.ty(plan.source) else {
            return;
        };
        let Some(identity) = self.identity(plan.source) else {
            return;
        };
        self.out
            .cursor_identities
            .insert(plan.source, identity.clone());
        if let Err(error) = super::cursors::refine(self, &mut plan, source, &identity) {
            self.out.failure.get_or_insert(error);
            return;
        }
        self.bind_pattern(pattern, plan.step.item, plan.source, true);
    }
    pub(super) fn bind_pattern(
        &mut self,
        pattern: hirsel_syntax::PatternId,
        actual: TypeId,
        value: ExprId,
        iteration: bool,
    ) {
        let source = self.source_place(value);
        let owned = matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || matches!(source.root, SourceRoot::Value(_))
            || self
                .binding(value)
                .is_some_and(|binding| binding.usage == UseKind::Take);
        let deep_unique = owned
            && self.out.body.types.get(actual).unwrap().modes.uniqueness == Uniqueness::Unique;
        let ownership = if deep_unique {
            PatternOwnership::Unique
        } else if owned {
            PatternOwnership::Owned
        } else {
            PatternOwnership::Borrowed
        };
        self.bind_pattern_value(pattern, actual, value, ownership, false, iteration);
    }
    fn bind_pattern_value(
        &mut self,
        pattern: hirsel_syntax::PatternId,
        actual: TypeId,
        value: ExprId,
        ownership: PatternOwnership,
        part: bool,
        iteration: bool,
    ) {
        let Some(decision) = self.pattern_row(pattern).cloned() else {
            return;
        };
        let mut ty = self.out.body.types.get(decision.ty).unwrap().clone();
        let a = self.out.body.types.get(actual).unwrap().clone();
        ty.sendability_requirement = a.sendability_requirement;
        ty.brand = a.brand.clone();
        ty.modes = a.modes.clone();
        ty.kind = a.kind.clone();
        ty.erased_captures = a.erased_captures.clone();
        ty.allocation_target = a.allocation_target;
        // MEM-5: a cleanup summary travels with the value it describes.
        ty.drop_allocation = a.drop_allocation.clone();
        let refined = self.out.body.types.push(ty);

        if let Some(stale) = self.invalid.get(&actual).copied() {
            self.invalid.insert(refined, stale);
        }
        if let Some(row) = self.pattern_row_mut(pattern) {
            row.ty = refined;
        }
        if let PatternDecisionKind::Or(alternatives) = &decision.kind {
            // Each alternative receives the whole value. Which one bound a
            // name is dynamic, so no alternative's part identity survives.
            for alternative in alternatives {
                self.bind_pattern_value(*alternative, actual, value, ownership, part, iteration);
            }
            for alternative in alternatives {
                if let Some(b) = self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    b.site == NameSite::Pattern(*alternative)
                }) {
                    self.identities.remove(&binding_key(b));
                }
            }
            return;
        }
        let boundary = matches!(
            self.out.body.types.get(refined).unwrap().kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        );
        let ownership = if boundary {
            PatternOwnership::Borrowed
        } else {
            ownership
        };
        let deep_unique = matches!(ownership, PatternOwnership::Unique);
        let received = if part && !boundary && !self.property(refined, Bound::Plain) {
            let mut row = self.out.body.types.get(refined).unwrap().clone();
            // I checks the current ownership of this root. J must retain
            // the declaration's payload contract for an owned transfer.
            row.modes.uniqueness = if deep_unique
                || matches!(ownership, PatternOwnership::Owned)
                    && a.modes.uniqueness == Uniqueness::Unique
            {
                Uniqueness::Unique
            } else {
                Uniqueness::Aliased
            };
            self.out.body.types.push(row)
        } else {
            refined
        };
        let task_handles = self.task_handles(value);
        let value_partition = self.value_partition(value);
        // A taken value, and a copy of a computed value that ends with this
        // statement, make the binding a new owner; a copy of a place keeps
        // that place's identity (PROV-1). Blocks are transparent (spec #50):
        // `{take x}` is `take x`. Lowering applies the same test.
        // A loop source describes a collection/cursor, not the yielded value.
        // Its item keeps the projected brand and lifetime above, but receives
        // its own per-iteration identity instead of aliasing the container.
        let value_identity = if iteration {
            None
        } else {
            match self.tree.expressions[self.transparent_value(value).0 as usize].kind {
                ExprKind::Unary {
                    op: UnaryOp::Take, ..
                } => None,
                ExprKind::Unary {
                    op: UnaryOp::Copy,
                    value: copied,
                } if self
                    .identity(copied)
                    .is_some_and(|o| o.root == OriginRoot::Expression(self.lexical, copied)) =>
                {
                    None
                }
                _ => self.identity(value),
            }
        };
        // Call results are owned receipts, even when local to an argument
        // (spec #43). Their source elaboration's Borrow is a consumer view,
        // not a copied alias that detaches when this binding is mutated.
        let owned_call = !part
            && !iteration
            && matches!(
                self.tree.expressions[value.0 as usize].kind,
                ExprKind::Call { .. }
            );
        let derivation = self
            .frames
            .last()
            .map(|frame| (frame.owner, self.used(value), value));
        let writes_iteration = iteration
            || match &self.source_place(value).root {
                SourceRoot::Binding { owner, binding } => {
                    self.root_binding(*owner, binding).is_some_and(|binding| {
                        self.iteration_sources.contains_key(&binding_key(binding))
                    })
                }
                _ => false,
            };
        if let Some(index) = self.binding_site_index(NameSite::Pattern(pattern)) {
            let row = &mut self.out.body.bindings[index];
            row.ty = received;
            if row.usage == UseKind::Mut && writes_iteration {
                self.iteration_sources.insert(binding_key(row), value);
            }
            if owned_call {
                row.usage = UseKind::Take;
            }
            self.binding_types.insert(binding_key(row), received);
            self.task_bindings.insert(binding_key(row), task_handles);
            if !row.mutable {
                if let Some(derivation) = derivation {
                    self.derivations.insert(binding_key(row), derivation);
                }
            }
            if let Some(o) = value_identity {
                self.identities.insert(
                    binding_key(row),
                    value_partition.unwrap_or_else(|| Identity::new(o)),
                );
            }
        }
        let mut inspected = actual;
        if !matches!(
            decision.kind,
            PatternDecisionKind::Binding(_) | PatternDecisionKind::Wildcard
        ) {
            while let Some(TypeKind::Box(payload)) = self.layout(inspected) {
                inspected = self.projected_item(inspected, payload);
            }
        }
        let actual_fields: Vec<_> = match self.layout(inspected) {
            Some(TypeKind::Record(fields)) => fields.into_iter().map(|field| field.ty).collect(),
            Some(TypeKind::Tuple(fields)) => fields.to_vec(),
            Some(TypeKind::Variant(variants)) => match &decision.kind {
                PatternDecisionKind::Constructor(ContextualName::Variant { variant, .. }) => {
                    variants
                        .get(variant.0 as usize)
                        .map(|variant| variant.fields.iter().map(|field| field.1).collect())
                        .unwrap_or_default()
                }
                _ => vec![],
            },
            Some(kind) => kind.value_parts(),
            None => vec![],
        };
        let roots: BTreeMap<_, _> = decision
            .fields
            .iter()
            .filter_map(|(field, child)| {
                let b = self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    b.site == NameSite::Pattern(*child)
                })?;
                let root = match b.binding {
                    ResolvedName::Local(l) => OriginRoot::Local(self.lexical, l),
                    _ => return None,
                };
                Some((
                    *field,
                    Origin {
                        root,
                        fields: vec![].into(),
                    },
                ))
            })
            .collect();
        for (field, child) in decision.fields {
            if let Some(part) = actual_fields.get(field.0 as usize) {
                let part = super::facts::siblings(&mut self.out.body.types, *part, &roots);
                let part = self.projected_item(refined, part);
                self.bind_pattern_value(child, part, value, ownership, true, iteration);
                if let Some(b) = self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    b.site == NameSite::Pattern(child)
                }) {
                    self.identities.remove(&binding_key(b));
                }
            }
        }
    }
}
