use super::*;

impl Checker<'_> {
    pub(super) fn validate_header(&mut self, f: &FunctionType, span: Span) -> bool {
        let mut valid = true;
        for id in f.parameters.iter().map(|p| p.ty).chain([f.result]) {
            valid &= self.header_type(self.t, id, f, span, &mut HashSet::default());
        }
        valid
    }
    fn header_type(
        &mut self,
        store: &TypeInterner,
        id: TypeId,
        f: &FunctionType,
        span: Span,
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        if !seen.insert(id) {
            return true;
        }
        let Some(t) = store.get(id) else { return true };
        let mut valid = true;
        if t.brand.alternatives.iter().any(|o| matches!(o.root, OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: p }) if f.parameters.get(p.0 as usize).is_some_and(|p| p.passing == PassingMode::Take))) {
            self.explained(
                const { RuleId::of("PROV-1") },
                span,
                "a brand cannot name a taken parameter",
                Explanation::default()
                    .provided(Fact::new(Cause::TakenOwner, span, &["the parameter"]))
                    .required(Fact::new(Requirement::OwnerOutlives, span, &[])),
            );
            valid = false;
        }
        // A taken arena ends with the call, so nothing the caller still sees
        // can be declared to live in it ("a value in a cannot outlive a").
        // The conservative reading covers every position, as for brands.
        let arena = self
            .signature
            .metadata
            .roles
            .heap
            .as_ref()
            .map(|h| h.arena.clone());
        if t.modes.relation.reaches().iter().any(|r| matches!(r, Reach::Place(o)
            if arena.as_ref().is_some_and(|arena| arena_parameter(store, f, o, 0, arena))
                && matches!(o.root, OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: p }) if f.parameters.get(p.0 as usize).is_some_and(|p| p.passing == PassingMode::Take))))
        {
            self.explained(
                const { RuleId::of("PROV-4") },
                span,
                "a placement cannot name a taken parameter",
                Explanation::default()
                    .provided(Fact::new(Cause::TakenOwner, span, &["the arena parameter"]))
                    .required(Fact::new(Requirement::OwnerOutlives, span, &[])),
            );
            valid = false;
        }
        for child in t.kind.value_parts() {
            valid &= self.header_type(store, child, f, span, seen);
        }
        valid
    }
    pub(super) fn compatible(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        intrinsic: Option<Intrinsic>,
        sibling: bool,
    ) -> bool {
        let actual_id = actual;
        let expected_id = expected;
        let a = self.t.get(actual).unwrap().clone();
        let b = self.t.get(expected).unwrap().clone();
        if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
            return false;
        }
        if !b.brand.alternatives.is_empty() {
            let stale = self.invalid.get(&actual).copied();
            if let Some(invalidation) = stale {
                let mut d = Diagnostic::error(
                    if sibling {
                        const { RuleId::of("PROV-5") }
                    } else {
                        const { RuleId::of("PROV-1") }
                    },
                    span,
                    "the brand's owner was invalidated",
                );
                let wanted = b
                    .brand
                    .alternatives
                    .first()
                    .map(|o| self.origin_site(o).0)
                    .unwrap_or_default();
                Explanation {
                    root: Some(hirsel_modes::explain::root_key(
                        self.signature.definition.path(),
                        ("ended", invalidation.start, invalidation.end),
                    )),
                    ..Explanation::default()
                }
                .provided(Fact::new(Cause::IdentityEnded, invalidation, &[]))
                .required(Fact::new(
                    Requirement::Brand,
                    self.receiving.as_ref().map_or(span, |r| r.span),
                    &[
                        &wanted,
                        &hirsel_modes::explain::qualifier(hirsel_modes::explain::Qualifier::Brand(
                            &wanted,
                        )),
                        &self
                            .receiving
                            .as_ref()
                            .map_or_else(|| "this use".into(), |r| r.description.clone()),
                    ],
                ))
                .render(&mut d);
                self.emit(d);
                return false;
            }
            if a.brand.alternatives.len() != 1 || b.brand.alternatives.len() != 1 {
                let explanation = self.unbranded_explanation(&b.brand, span);
                self.explained(
                    if sibling {
                        const { RuleId::of("PROV-5") }
                    } else {
                        const { RuleId::of("PROV-2") }
                    },
                    span,
                    "a union or erased origin cannot satisfy a branded use",
                    explanation,
                );
                return false;
            }
            if brand_join(&a.brand, &Brand::default()) != brand_join(&b.brand, &Brand::default()) {
                let mut diagnostic = Diagnostic::error(
                    if sibling {
                        const { RuleId::of("PROV-5") }
                    } else {
                        const { RuleId::of("PROV-1") }
                    },
                    span,
                    "the value belongs to a different origin",
                );
                self.brand_explanation(&a.brand.alternatives[0], &b.brand.alternatives[0], span)
                    .render(&mut diagnostic);
                if let Some(invalidation) = b.brand.alternatives.iter().find_map(|required| {
                    self.identity_changes.iter().find_map(|(changed, span)| {
                        (changed.root == required.root
                            && (required.fields.starts_with(&changed.fields)
                                || changed.fields.starts_with(&required.fields)))
                        .then_some(*span)
                    })
                }) {
                    diagnostic.secondary.push(Label {
                        span: invalidation,
                        message: "identity ended here".into(),
                    });
                }
                self.emit(diagnostic);
                return false;
            }
        }
        let arenas =
            |r: &Relation| r.retain(|r| matches!(r, Reach::Place(o) if self.arena_place(o)));
        // `in heap` is the empty relation: a global value meets an arena
        // relation; a bounded value elsewhere does not (PROV-4).
        let (expected, mut actual) = (arenas(&b.modes.relation), arenas(&a.modes.relation));

        // v22 R3: a value is within S when its arena loans are on arenas S
        // names or that the relation of a binding S names names.
        let expected = self.admitted_arenas(&b.modes.relation, expected);
        // A frame bound alone admits any arena; a relation naming bindings
        // admits only the arenas they hold (v22 R3), perhaps none.
        let within = self.names_bindings(&b.modes.relation);
        if within {
            if let Some(held) = self
                .receiving
                .as_ref()
                .and_then(|r| r.value)
                .filter(|value| self.ty(*value) == Some(actual_id))
                .and_then(|value| self.read_holdings(value))
            {
                actual = held;
            }
        }
        if (!expected.is_global() || within)
            && !a.modes.relation.is_global()
            && !actual
                .reaches()
                .iter()
                .all(|r| expected.reaches().contains(r))
        {
            if within {
                let (message, explanation) =
                    self.within_explanation(&actual, &expected, &b.modes.relation, span);
                self.explained(const { RuleId::of("PROV-4") }, span, &message, explanation);
                return false;
            }
            // Name an arena the value holds outside the relation.
            let escaping = actual.retain(|r| !expected.reaches().contains(r));
            let explanation = self.placement_explanation(&escaping, &b.modes.relation, span);
            self.explained(
                const { RuleId::of("PROV-4") },
                span,
                "the value's placement does not outlive the required arena",
                explanation,
            );
            return false;
        }
        // The receiving requirement applies to every type. Current content
        // supplies the proof; locality remains an independent obligation.
        if crossing_requirement(&b) != Sendability::NotSendable
            && !self.receives_sendability(actual_id, &crossing_requirement(&b))
        {
            let value = self.received_span(span);
            let (receiver, mut description) = self.receiving.as_ref().map_or_else(
                || (span, "this place".to_owned()),
                |r| (r.span, r.description.clone()),
            );
            let whole = self.receiving.as_ref().and_then(|r| r.value);
            if whole.and_then(|value| self.ty(value)) != Some(actual_id) {
                description = format!("a part of {description}");
            }
            let owned_local = self
                .receiving
                .as_ref()
                .is_some_and(|receiving| !receiving.result)
                && !self.lifetime(&a.modes.relation).is_global()
                && !matches!(
                    a.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                )
                && !self.property(actual_id, Bound::Flat)
                && whole.is_some_and(|value| {
                    matches!(self.source_place(value).root, SourceRoot::Value(_))
                });
            if owned_local {
                self.explained(
                    const { RuleId::of("MODE-1") },
                    span,
                    "an owned result retains a pre-existing local source bound",
                    Explanation::default()
                        .provided(Fact::new(Cause::LocalValue, value, &[]))
                        .required(Fact::new(
                            Requirement::GlobalBinding,
                            receiver,
                            &[&description, "in"],
                        )),
                );
                return false;
            }
            self.explained(
                const { RuleId::of("MODE-4") },
                span,
                "a value that is not sendable cannot be kept where its type is `sendable`",
                Explanation::default()
                    .provided(Fact::new(Cause::NotSendable, value, &[]))
                    .required(Fact::new(
                        Requirement::SendableType,
                        receiver,
                        &[&description],
                    )),
            );
            return false;
        }
        if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
            if let Uses::Only(allowed) = &bf.uses {
                let allowed = self.callable_uses(bf, allowed);
                let valid = match &af.uses {
                    Uses::Only(actual) => subset(&self.callable_uses(af, actual), &allowed),
                    Uses::Unrestricted => false,
                };
                if !valid {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    self.explained(
                        if intrinsic == Some(Intrinsic::MutexWith) {
                            const { RuleId::of("CONC-5") }
                        } else if b.modes.relation.is_global() {
                            const { RuleId::of("CLO-4") }
                        } else {
                            const { RuleId::of("CAP-3") }
                        },
                        span,
                        "function value authority exceeds the required uses restriction",
                        Explanation::default()
                            .provided(Fact::new(Cause::Authority, value, &[]))
                            .required(Fact::new(Requirement::Uses, receiver, &[])),
                    );
                    return false;
                }
            }
            // MEM-5: the allocation contract is part of a function type, so
            // `@noalloc` is received only from a value that promises it.
            if bf.allocation == Allocation::NoAlloc && af.allocation != Allocation::NoAlloc {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    const { RuleId::of("MEM-5") },
                    span,
                    "the function value may allocate where the function type is `@noalloc`",
                    self.header_facts(actual_id, expected_id, value, receiver),
                );
                return false;
            }
            // Contract 6: each expected state guarantee needs the same
            // selector on the value; a stronger promise may satisfy it.
            if !state_promises_satisfy(&af.state_promises, &bf.state_promises) {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                let advance = bf.state_promises.iter().any(|expected| {
                    expected.advance_only
                        && !af.state_promises.iter().any(|actual| {
                            actual.selector == expected.selector && actual.advance_only
                        })
                });
                self.explained(
                    if advance { const { RuleId::of("MEM-14") } } else { const { RuleId::of("CLO-4") } },
                    span,
                    "the function value does not promise the state guarantees the function type requires",
                    self.header_facts(actual_id, expected_id, value, receiver),
                );
                return false;
            }
            if !self.captures_promised(af, bf) {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    const { RuleId::of("CLO-4") },
                    span,
                    "the function value does not promise the creation inputs the function type selects",
                    self.header_facts(actual_id, expected_id, value, receiver),
                );
                return false;
            }
            let nested = af
                .parameters
                .iter()
                .map(|p| p.ty)
                .zip(bf.parameters.iter().map(|p| p.ty))
                .chain([(af.result, bf.result)])
                .zip(Variance::Covariant.edges(&b.kind));
            for ((actual, expected), variance) in nested {
                if let Some((rule, message)) =
                    self.header_difference(actual, expected, variance, true)
                {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    self.explained(
                        if sibling && matches!(rule.as_str(), "PROV-1" | "PROV-2") {
                            const { RuleId::of("PROV-5") }
                        } else {
                            rule
                        },
                        span,
                        message,
                        self.header_facts(actual_id, expected_id, value, receiver),
                    );
                    return false;
                }
            }
        }
        let parts = a
            .kind
            .graph_edges()
            .into_iter()
            .zip(b.kind.graph_edges())
            .zip(Variance::Covariant.edges(&b.kind))
            .filter(|((_, b), _)| b.role.parameter_binders() == 0);
        for ((a, b), variance) in parts {
            // A payload is received within its owner's placement. Generic
            // arguments keep their separately declared, invariant contracts.
            let (actual, expected) = if b.role == TypeEdgeRole::Component {
                (
                    self.placed_item(actual_id, a.ty),
                    self.placed_item(expected_id, b.ty),
                )
            } else {
                (a.ty, b.ty)
            };
            // Reading a borrowed payload cannot keep its arena storage.
            let borrowed = self.receiving.as_ref().is_some_and(|r| r.borrowed);
            // MEM-4: seal transfers the complete storage graph, including
            // arena-placed tuple/record components. I and V independently prove
            // closure over the designated grant before accepting the transfer.
            if !borrowed
                && intrinsic != Some(Intrinsic::ArenaSeal)
                && !self.keeps_placement(actual, expected, span, false)
                || variance == Variance::Exact && !self.keeps_exact_promises(actual, expected, span)
                || !self.compatible(actual, expected, span, intrinsic, sibling)
            {
                return false;
            }
        }
        true
    }
    /// PROV-4: "a value in a cannot outlive a". Storage, a result or a part
    /// declared without a placement promises nothing about an arena, so an
    /// arena value cannot be kept there; a heap value can. A `local(x, ..)`
    /// result is bounded by its sources instead, the root of an argument or
    /// of an annotated binding receives its actual placement, and a plain
    /// value's placement is inert. A local arena value kept where a global is
    /// required is PROV-4 too: docs/spec/diagnostics.md orders placement before required modes,
    /// and ownership judges a read's locality by current content, which may
    /// be Global under a written local storage contract. Only a part of a
    /// Local aggregate checked where it is constructed defers to that owner:
    /// the declared field row does not know the owner's lifetime, which is
    /// checked where the owner is kept.
    pub(super) fn keeps_placement(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        local_owner_part: bool,
    ) -> bool {
        // A plain value is copied bytes: its placement is inert.
        if self.property(actual, Bound::Plain) {
            return true;
        }
        let types = self.t;
        let (Some(a), Some(b)) = (types.get(actual), types.get(expected)) else {
            return true;
        };
        let (al, bl) = (
            self.lifetime(&a.modes.relation),
            self.lifetime(&b.modes.relation),
        );
        // A let relation naming bindings admits the arenas they hold, and
        // no other (v22 R3, "within S").
        let within = bl.named_parameters().is_none() && self.names_bindings(&b.modes.relation);
        if within && !self.placed(&b.modes.relation) && self.placed(&a.modes.relation) {
            let allowed =
                self.admitted_arenas(&b.modes.relation, self.placement(&b.modes.relation));
            let held = self
                .receiving
                .as_ref()
                .and_then(|r| r.value)
                .filter(|value| self.ty(*value) == Some(actual))
                .and_then(|value| self.read_holdings(value))
                .unwrap_or_else(|| self.placement(&a.modes.relation));
            if !held.contained_in(&allowed)
                && !matches!(a.kind, TypeKind::Error)
                && !matches!(b.kind, TypeKind::Error)
            {
                let relation = b.modes.relation.clone();
                let (message, explanation) =
                    self.within_explanation(&held, &allowed, &relation, span);
                self.explained(const { RuleId::of("PROV-4") }, span, &message, explanation);
                return false;
            }
            return true;
        }
        if !self.placed(&b.modes.relation)
            && bl.named_parameters().is_none()
            && (!local_owner_part || al.is_global() || !bl.is_global())
            && self.placed(&a.modes.relation)
            && !matches!(a.kind, TypeKind::Error)
            && !matches!(b.kind, TypeKind::Error)
        {
            let (actual, expected) = (a.modes.relation.clone(), b.modes.relation.clone());
            let explanation = self.placement_explanation(&actual, &expected, span);
            self.explained(
                const { RuleId::of("PROV-4") },
                span,
                "an arena value cannot be kept where no placement is declared",
                explanation,
            );
            return false;
        }
        // A relation naming parameters admits the arenas those parameters'
        // own relations name, and no other (PROV-4, "within S").
        if let Some(parameters) = bl
            .named_parameters()
            .filter(|_| self.placed(&a.modes.relation))
        {
            let mut allowed = self.placement(&b.modes.relation);
            for parameter in parameters {
                let ParameterRef::Body { scope, parameter } = parameter else {
                    return true;
                };
                let Some(binding) = self.out.body.bindings.iter().find(|binding| {
                    self.lexical.site(binding.owner) == scope
                        && binding.binding == ResolvedName::Parameter(parameter)
                }) else {
                    return true;
                };
                let row = &self.t.get(binding.ty).unwrap().modes.relation;
                allowed = allowed.union(&self.placement(row));
            }
            if !self.placement(&a.modes.relation).contained_in(&allowed)
                && !matches!(a.kind, TypeKind::Error)
                && !matches!(b.kind, TypeKind::Error)
            {
                let explanation =
                    self.placement_explanation(&a.modes.relation, &b.modes.relation, span);
                self.explained(
                    const { RuleId::of("PROV-4") },
                    span,
                    "an arena value cannot be kept where no placement is declared",
                    explanation,
                );
                return false;
            }
        }
        true
    }
    /// PROV-4: "a value in a cannot outlive a". A value owns the blocks of
    /// its parts, so an environment that outlives its creating call cannot
    /// hold a non-Plain row placed in an Arena or Region at any depth of the
    /// captured value. A callable's header describes what it receives, not
    /// what it holds, so function types are not entered.
    pub(super) fn retains_scoped_placement(&mut self, ty: TypeId) -> bool {
        let mut pending = vec![ty];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let Some(row) = self.t.get(ty) else {
                continue;
            };
            if matches!(row.kind, TypeKind::Error | TypeKind::Function(_)) {
                continue;
            }
            let scoped = self.placed(&row.modes.relation);
            let edges = row.kind.value_parts();
            if scoped && !self.property(ty, Bound::Plain) {
                return true;
            }
            pending.extend(edges);
        }
        false
    }
    /// Brands are identity evidence: a data value borrowed or taken by a
    /// parameter that names no brand is valid at any depth, the callee simply
    /// does not know it. A `mut` parameter hands the storage back, and a
    /// value that is not data has interior storage, so either could come back
    /// holding an unbranded part where the caller's type promises a brand.
    pub(super) fn keeps_nested_brands(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        passing: PassingMode,
        argument: ExprId,
    ) -> bool {
        let types = self.t;
        let mut pending: Vec<_> = types
            .get(actual)
            .unwrap()
            .kind
            .value_parts()
            .into_iter()
            .zip(types.get(expected).unwrap().kind.value_parts())
            .collect();
        let mut seen = HashSet::default();
        let mut forgotten = false;
        while let Some((a, b)) = pending.pop() {
            if !seen.insert((a, b)) {
                continue;
            }
            let (Some(a), Some(b)) = (types.get(a), types.get(b)) else {
                continue;
            };
            if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
                continue;
            }
            forgotten |= b.brand.alternatives.is_empty() && !a.brand.alternatives.is_empty();
            pending.extend(a.kind.value_parts().into_iter().zip(b.kind.value_parts()));
        }
        let data = forgotten && passing != PassingMode::Mut && {
            let store = self.t;
            let generics = self.declarations();
            structural_equality(store, actual, Bound::Data, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false))
        };
        if forgotten && !data {
            let receiver = self
                .receiving
                .as_ref()
                .map_or(self.span(argument), |r| r.span);
            self.explained(
                const { RuleId::of("PROV-2") },
                self.span(argument),
                "the parameter could hand this storage back without the brand its type names",
                Explanation::default()
                    .provided(Fact::new(
                        Cause::BrandForgotten,
                        self.value_site(argument),
                        &[],
                    ))
                    .required(Fact::new(Requirement::UnbrandedParameter, receiver, &[])),
            );
            return false;
        }
        true
    }
    /// MODE-2: a readonly/taken view may forget owned payload evidence. Mut
    /// hand-back must preserve the caller's complete nested storage contract.
    pub(super) fn keeps_nested_uniqueness(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        view: bool,
        span: Span,
    ) -> bool {
        let store = self.t;
        // Callable headers have their own exact check above. Inferred literal
        // body evidence is intentionally separate from the callable promise.
        let mut pending: Vec<_> = store
            .get(actual)
            .unwrap()
            .kind
            .value_parts()
            .into_iter()
            .zip(store.get(expected).unwrap().kind.value_parts())
            .collect();
        let mut seen = HashSet::default();
        let mut different = false;
        while let Some((a, b)) = pending.pop() {
            if !seen.insert((a, b)) {
                continue;
            }
            let (Some(a), Some(b)) = (store.get(a), store.get(b)) else {
                continue;
            };
            if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
                continue;
            }
            different |= a.modes.uniqueness != b.modes.uniqueness;
            pending.extend(a.kind.value_parts().into_iter().zip(b.kind.value_parts()));
        }
        if !different {
            return true;
        }
        let actual = if view {
            let mut expression = self
                .receiving
                .as_ref()
                .and_then(|receiving| receiving.value)
                .or_else(|| {
                    self.out
                        .body
                        .expressions
                        .iter()
                        .find(|row| self.span(row.expression) == span)
                        .map(|row| row.expression)
                });
            let mut direct = None;
            while let Some(value) = expression.take() {
                match &self.tree.expressions[value.0 as usize].kind {
                    ExprKind::Block { tail, .. } | ExprKind::Return(tail) => expression = *tail,
                    ExprKind::Unary {
                        op: UnaryOp::Take,
                        value,
                    } => expression = Some(*value),
                    ExprKind::Call { .. } => {
                        direct = self
                            .out
                            .body
                            .calls
                            .iter()
                            .find(|call| call.expression == value)
                            .and_then(|call| match &call.target {
                                CallTarget::Definition(path) => Some(path.clone()),
                                _ => None,
                            });
                    }
                    _ => (),
                }
            }
            if let Some(path) = direct
                && hirsel_types::unique_summary::same_module(
                    &path,
                    self.signature.definition.path(),
                )
                && let Some(signature) = self.read(DefId::intern(&path))
            {
                hirsel_types::unique_summary::result_evidence(
                    self.t,
                    actual,
                    &signature.metadata.inferred_unique.results,
                    &signature.metadata.roles,
                    &mut |id| self.read(id),
                )
            } else {
                actual
            }
        } else {
            actual
        };
        let valid = self.proof(false, |memo| memo.nested_uniqueness(expected, actual, view));
        if !valid && !self.nested_element_diagnostic(actual, expected, span) {
            let value = self.received_span(span);
            let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
            self.explained(
                const { RuleId::of("MODE-2") },
                span,
                "the receiving type cannot change this nested uniqueness contract",
                Explanation::default()
                    .provided(Fact::new(Cause::Aliased, value, &[]))
                    .required(Fact::new(Requirement::Unique, receiver, &[])),
            );
        }
        valid
    }
    /// The provided and required facts of a function value whose header
    /// differs from the required function type, naming both types.
    fn header_facts(
        &self,
        actual: TypeId,
        expected: TypeId,
        value: Span,
        receiver: Span,
    ) -> Explanation {
        let spell = |root| {
            hirsel_type_analysis::spelling::type_spelling(
                self.t,
                root,
                &self.out.body.resolution.generic_declarations,
                &self.signature.definition.path().0.package,
                true,
            )
        };
        Explanation::default()
            .provided(Fact::new(Cause::Header, value, &[&spell(actual)]))
            .required(Fact::new(
                Requirement::Header,
                receiver,
                &[&spell(expected)],
            ))
    }
    /// A value kept in storage, a result or a constructed part.
    pub(super) fn kept(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        expression: ExprId,
        sibling: bool,
        receiving: Receiving,
        local_owner_part: bool,
    ) -> bool {
        let saved = self.receiving.replace(Receiving {
            value: Some(expression),
            ..receiving
        });
        let kept = self.kept_by(
            actual,
            expected,
            span,
            expression,
            sibling,
            local_owner_part,
        );
        self.receiving = saved;
        kept
    }
    fn kept_by(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        expression: ExprId,
        sibling: bool,
        local_owner_part: bool,
    ) -> bool {
        let place = self.source_place(expression);
        let view = place.access != SourceAccess::Copy
            && (matches!(place.access, SourceAccess::Take | SourceAccess::Unique)
                || matches!(place.root, SourceRoot::Value(_)) && place.projections.is_empty()
                || place.projections.is_empty()
                    && self
                        .binding(expression)
                        .is_some_and(|binding| binding.usage == UseKind::Take));
        self.keeps_placement(actual, expected, span, local_owner_part)
            && self.compatible(actual, expected, span, None, sibling)
            && self.keeps_nested_uniqueness(actual, expected, view, span)
    }
    /// Only the root of a value weakens to its use. Parameter and result types
    /// inside a function type are exact, so a callable header that differs from
    /// the required one in a mode, brand or placement is another type. A
    /// callable inside the header keeps its own promise in the direction it
    /// is read (CLO-2, `hirsel_types::Variance`): a result may promise more
    /// than required, a parameter may ask for less than it is given. The same
    /// holds for the uniqueness of a header row itself (`header`), never for
    /// a row below it (MODE-2).
    fn header_difference(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        variance: Variance,
        header: bool,
    ) -> Option<(RuleId, &'static str)> {
        let types = self.t;
        let (a, b) = (types.get(actual)?.clone(), types.get(expected)?.clone());
        if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
            return None;
        }
        if a.modes.relation != b.modes.relation {
            return Some((
                const { RuleId::of("MODE-1") },
                "a function header's relation differs from the required function type",
            ));
        }
        if a.modes.uniqueness != b.modes.uniqueness
            && !(header && hirsel_types::header_weakens_uniqueness(&b, &a, variance))
        {
            return Some((
                const { RuleId::of("MODE-2") },
                "a function header's uniqueness differs from the required function type",
            ));
        }
        if a.modes.multiplicity != b.modes.multiplicity {
            return Some((
                const { RuleId::of("MODE-3") },
                "a function header's multiplicity differs from the required function type",
            ));
        }
        // MODE-4: a callable's crossing mode is part of its type, read in
        // the direction of the edge, as `hirsel_types::nested_promise` does.
        {
            let (ap, bp) = (crossing_requirement(&a), crossing_requirement(&b));
            let differs = match variance {
                Variance::Covariant => !Self::crossing_satisfies(&ap, &bp),
                Variance::Contravariant => !Self::crossing_satisfies(&bp, &ap),
                Variance::Exact => ap != bp,
            };
            if differs {
                return Some((
                    const { RuleId::of("MODE-4") },
                    "a function header's sendability differs from the required function type",
                ));
            }
        }
        if a.brand.alternatives.is_empty() != b.brand.alternatives.is_empty() {
            return Some((
                const { RuleId::of("PROV-1") },
                "a function header's brand differs from the required function type",
            ));
        }
        if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
            let supplied = variance == Variance::Contravariant || self.promises(af, bf);
            let demanded = variance == Variance::Covariant || self.promises(bf, af);
            if !supplied {
                return Some((
                    const { RuleId::of("CLO-2") },
                    "a function in the header promises less than the required function type",
                ));
            }
            if !demanded {
                return Some((
                    const { RuleId::of("CLO-2") },
                    "a function in the header requires more than the required function type gives",
                ));
            }
        }
        let nested = |kind: &TypeKind| match kind {
            TypeKind::Function(f) => f
                .parameters
                .iter()
                .map(|p| p.ty)
                .chain([f.result])
                .collect(),
            kind => kind.value_parts(),
        };
        let callable = matches!(b.kind, TypeKind::Function(_));
        nested(&a.kind)
            .into_iter()
            .zip(nested(&b.kind))
            .zip(variance.edges(&b.kind))
            .find_map(|((a, b), variance)| self.header_difference(a, b, variance, callable))
    }
    /// CLO-2: the callee writes a `mut` argument back through its parameter,
    /// so the argument's storage and the parameter state the same callable
    /// promises, at the root and at every depth: storage that promised less
    /// would return holding what the parameter admits, and storage that
    /// promised more would be read as what it never held.
    pub(super) fn keeps_handback_promises(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        argument: ExprId,
        callee: ExprId,
    ) -> bool {
        let actual = self.cleanup_contract(actual);
        let expected = self.cleanup_contract(expected);
        let root = (actual, expected);
        let mut pending = vec![(actual, expected)];
        let mut seen = HashSet::default();
        while let Some((actual, expected)) = pending.pop() {
            if !seen.insert((actual, expected)) {
                continue;
            }
            let types = self.t;
            let (Some(a), Some(b)) = (types.get(actual).cloned(), types.get(expected).cloned())
            else {
                continue;
            };
            // MODE-3: a mutable alias receives and hands back the same
            // callable contract; owned many-to-once reception may weaken.
            if a.modes.multiplicity != b.modes.multiplicity {
                self.explained(
                    const { RuleId::of("MODE-3") },
                    span,
                    "a `mut` callable argument must have exactly the parameter's once/many multiplicity: the callee may replace it with any function of the parameter's type. Match the binding and parameter types, or take the value and return the new one",
                    self.header_facts(actual, expected, self.received_span(span), self.receiving.as_ref().map_or(span, |r| r.span)),
                );
                if let Some(diagnostic) = self.out.diagnostics.last_mut()
                    && diagnostic.primary == span
                    && diagnostic.rule.as_str() == "MODE-3"
                {
                    diagnostic
                        .obligations
                        .push(hirsel_diag::Obligation::Multiplicity);
                }
                return false;
            }
            if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
                if !(self.promises(af, bf) && self.promises(bf, af)) {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    let message = self.handback_message(
                        argument,
                        callee,
                        ((actual, expected) == root).then_some(root),
                    );
                    self.explained(
                        const { RuleId::of("CLO-2") },
                        span,
                        &message,
                        self.header_facts(actual, expected, value, receiver),
                    );
                    return false;
                }
            }
            // MODE-4: sendable storage handed to a parameter that is not
            // would come back holding what crosses no task, still typed as
            // crossing; storage that is not sendable would be read as
            // sendable inside. So a callable's crossing mode is exact too.
            if !self.property(actual, Bound::Plain)
                && crossing_requirement(&a) != crossing_requirement(&b)
            {
                let mut place = argument;
                while let ExprKind::Unary {
                    op: UnaryOp::Mut,
                    value,
                } = self.tree.expressions[place.0 as usize].kind
                {
                    place = value;
                }
                let name = self.value_name(Some(place));
                let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
                let (value, receiver) = (
                    self.received_span(span),
                    self.receiving.as_ref().map_or(span, |r| r.span),
                );
                let (provided, required) = if b.modes.sendability == Sendability::Sendable {
                    (Cause::NotSendable, Requirement::SendableType)
                } else {
                    (Cause::Sendable, Requirement::NotSendableType)
                };
                let description = format!("the parameter of `{callee}`");
                self.explained(
                    const { RuleId::of("MODE-4") },
                    span,
                    &format!(
                        "a `mut` argument's crossing mode must be exactly the parameter's: `{callee}` may store in `{name}` any value of the parameter's type, which is then used as the argument's. Write the binding's type as the parameter states it, or take the value and return the new one"
                    ),
                    Explanation::default()
                        .provided(Fact::new(provided, value, &[]))
                        .required(Fact::new(required, receiver, &[&description])),
                );
                return false;
            }
            // The erased state's captures (CLO-2) and the cleanup summary
            // (MEM-5) are written back with the value, so they are exact too.
            // A core operation that only advances an erased state declares
            // its parameter at the argument's own captures (spec #91).
            let differs = self.state_difference(&a, &b);
            if let Some((rule, what)) = differs {
                let name = self.value_name(Some(argument));
                let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
                self.error(
                    rule,
                    span,
                    &format!(
                        "a `mut` argument's {what} must be exactly the parameter's: `{callee}` may store in `{name}` any value of the parameter's type, which is then used as the argument's. Write the binding's type as the parameter states it, or take the value and return the new one"
                    ),
                );
                return false;
            }
            pending.extend(
                a.kind
                    .graph_edges()
                    .into_iter()
                    .zip(b.kind.graph_edges())
                    .map(|(a, b)| (a.ty, b.ty)),
            );
        }
        true
    }
    /// Alias substitution and callback signatures may spell the same cleanup
    /// promise as NoAlloc or DependsOn. Resolve from the exact declaring binder
    /// before invariant comparison, as V does for owned receipts. Traverse the
    /// whole contract here so nested callable binders retain their own bounds.
    fn cleanup_contract(&mut self, root: TypeId) -> TypeId {
        instantiate_cleanup_bounds(self.t, root, &self.declarations())
            .unwrap_or_else(|error| self.proof_failure(error, root))
    }
    /// Whether an erased state's captures (CLO-2) or a cleanup summary
    /// (MEM-5) differ between two rows that must state the same contract.
    fn state_difference(&mut self, a: &Type, b: &Type) -> Option<(RuleId, &'static str)> {
        let state = |row: &Type| {
            row.erased_captures
                .as_ref()
                .map_or(AuthoritySummary::Unknown, ErasedCaptures::authority)
        };
        let dependent =
            |row: &Type| matches!(row.erased_captures, Some(ErasedCaptures::FromArguments(_)));
        let same_state = if dependent(a) || dependent(b) {
            a.erased_captures == b.erased_captures
        } else {
            let held = self.scoped_authority(state(a));
            let allowed = self.scoped_authority(state(b));
            held == allowed || (subset(&held, &allowed) && subset(&allowed, &held))
        };
        if !same_state {
            Some((
                const { RuleId::of("CLO-2") },
                "erased state's captured authority",
            ))
        } else if a.drop_allocation != b.drop_allocation {
            Some((
                const { RuleId::of("MEM-5") },
                "cleanup summary (whether destroying it may allocate)",
            ))
        } else {
            None
        }
    }
    /// A nominal type's argument, or a non-sequence builtin's, is exact
    /// (`hirsel_types::Variance`): the declaration may use it in either
    /// direction, so a callable, an erased state or a cleanup summary in it
    /// promises exactly what the expected type states, at every depth. v22
    /// is silent here; this is the conservative reading (spec #92).
    fn keeps_exact_promises(&mut self, actual: TypeId, expected: TypeId, span: Span) -> bool {
        let actual = self.cleanup_contract(actual);
        let expected = self.cleanup_contract(expected);
        let mut pending = vec![(actual, expected)];
        let mut seen = HashSet::default();
        while let Some((actual, expected)) = pending.pop() {
            if !seen.insert((actual, expected)) {
                continue;
            }
            let types = self.t;
            let (Some(a), Some(b)) = (types.get(actual).cloned(), types.get(expected).cloned())
            else {
                continue;
            };
            // A plain value has no crossing contract to compare. The cheap row
            // comparison runs first: a plain proof walks the whole subgraph,
            // which would make this walk quadratic in nesting depth.
            let differs = if crossing_requirement(&a) != crossing_requirement(&b)
                && !self.property(actual, Bound::Plain)
            {
                Some((
                    const { RuleId::of("MODE-4") },
                    "crossing receiving contract",
                ))
            } else {
                match (&a.kind, &b.kind) {
                    (TypeKind::Function(af), TypeKind::Function(bf))
                        if !(self.promises(af, bf) && self.promises(bf, af)) =>
                    {
                        Some((
                            const { RuleId::of("CLO-2") },
                            "callable promise (`uses` and `@noalloc`)",
                        ))
                    }
                    _ => self.state_difference(&a, &b),
                }
            };
            if let Some((rule, what)) = differs {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    rule,
                    span,
                    &format!(
                        "a type argument's {what} must be exactly the expected type's: a generic type may use its argument in either direction. Write the value's type as the expected type states it"
                    ),
                    self.header_facts(actual, expected, value, receiver),
                );
                return false;
            }
            pending.extend(
                a.kind
                    .graph_edges()
                    .into_iter()
                    .zip(b.kind.graph_edges())
                    .map(|(a, b)| (a.ty, b.ty)),
            );
        }
        true
    }
    /// Both sides of a refused `mut` hand-back and the two ways to make them
    /// agree; the types are quoted where they have a short spelling.
    fn handback_message(
        &self,
        argument: ExprId,
        callee: ExprId,
        root: Option<(TypeId, TypeId)>,
    ) -> String {
        let headline =
            "a `mut` argument's function type must promise exactly what the parameter's does";
        let mut place = argument;
        while let ExprKind::Unary {
            op: UnaryOp::Mut,
            value,
        } = self.tree.expressions[place.0 as usize].kind
        {
            place = value;
        }
        let whole = self
            .source_rows
            .places
            .get(&place)
            .is_some_and(|i| self.out.body.places[*i].projections.is_empty());
        let binding = self.binding(place).filter(|_| whole);
        let name = self.value_name(Some(place));
        let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
        let annotated = binding.as_ref().is_some_and(|b| match b.site {
            NameSite::Pattern(pattern) => self
                .pattern_row(pattern)
                .is_some_and(|p| p.annotation.is_some()),
            _ => true,
        });
        let spelled = root.and_then(|(actual, expected)| {
            Some((
                self.function_spelling(actual, false)?,
                self.function_spelling(expected, true)?,
            ))
        });
        let stores = format!(
            "`{callee}` may store in it any function of the parameter's type, which is then used as the argument's"
        );
        match (binding.is_some(), annotated, spelled) {
            (true, false, Some((inferred, parameter))) => format!(
                "{headline}: the inferred type of `{name}` is `{inferred}` and the `mut` parameter is `{parameter}`; {stores}. Write the binding's type, `let mut {name}: {parameter} = ...`, or narrow the parameter to the inferred type"
            ),
            (true, true, Some((declared, parameter))) => format!(
                "{headline}: `{name}` is declared `{declared}` and the `mut` parameter is `{parameter}`; {stores}. Declare both with the same `uses` and `@noalloc`"
            ),
            (true, false, None) => format!(
                "{headline}: the inferred type of `{name}` keeps the `uses` and `@noalloc` its initializer proves, and the `mut` parameter promises differently; {stores}. Write the binding's type as the parameter states it, or narrow the parameter"
            ),
            _ => format!("{headline}: {stores}. Declare both with the same `uses` and `@noalloc`"),
        }
    }
    /// CLO-2: `value` keeps every promise `place` makes: no allocation where
    /// none is promised, and no authority beyond the place's `uses`.
    /// CLO-4: a callable type's `@captures` selection is a promise about its
    /// result's environment. A value promises it when its own selection keeps
    /// no input the required one leaves out; an unknown environment does not.
    fn captures_promised(&self, value: &FunctionType, place: &FunctionType) -> bool {
        let selection = |result: TypeId| match &self.t.get(result)?.erased_captures {
            Some(ErasedCaptures::FromArguments(selectors)) => Some(selectors.clone()),
            _ => None,
        };
        match selection(place.result) {
            None => true,
            Some(required) => selection(value.result)
                .is_some_and(|held| held.iter().all(|selector| required.contains(selector))),
        }
    }
    /// Own selectors are compared under their own callable's formal types.
    /// A capability has its exact nominal authority; inert inputs contribute
    /// none. A callable's ceiling can be attenuated by an actual value, so its
    /// selector stays symbolic and cannot authorize a sibling callback.
    fn callable_uses(
        &mut self,
        function: &FunctionType,
        uses: &AuthoritySummary,
    ) -> AuthoritySummary {
        let AuthoritySummary::Known(terms) = uses else {
            return AuthoritySummary::Unknown;
        };
        let mut summary = AuthoritySummary::inert();
        for term in terms.iter() {
            let replacement = if let AuthorityTerm::Parameter(ParameterRef::Bound {
                depth: 0,
                parameter,
            }) = term
            {
                function.parameters.get(parameter.0 as usize).and_then(|p| {
                    self.capability_definition(p.ty)
                        .map(|path| {
                            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path)].into())
                        })
                        .or_else(|| {
                            (self.authority(p.ty) == AuthoritySummary::inert())
                                .then(AuthoritySummary::inert)
                        })
                })
            } else {
                None
            };
            summary = summary.join(
                replacement.unwrap_or_else(|| AuthoritySummary::Known(vec![term.clone()].into())),
            );
        }
        self.scoped_authority(summary)
    }
    fn promises(&mut self, value: &FunctionType, place: &FunctionType) -> bool {
        state_promises_satisfy(&value.state_promises, &place.state_promises)
            && self.captures_promised(value, place)
            && (place.allocation != Allocation::NoAlloc || value.allocation == Allocation::NoAlloc)
            && match (&value.uses, &place.uses) {
                (_, Uses::Unrestricted) => true,
                (Uses::Only(held), Uses::Only(allowed)) => {
                    let allowed = self.callable_uses(place, allowed);
                    subset(&self.callable_uses(value, held), &allowed)
                }
                (Uses::Unrestricted, Uses::Only(_)) => false,
            }
    }
    /// A callable promise may weaken its result; bounds inspect the independently
    /// recorded complete body result instead of strengthening that promise.
    pub(super) fn complete_value_type(&mut self, ty: TypeId) -> TypeId {
        let Some(result) = self.closures.by_type(ty).and_then(|c| c.result_evidence) else {
            return ty;
        };
        let mut row = self.t.get(ty).unwrap().clone();
        if let TypeKind::Function(f) = &mut row.kind {
            f.result = result;
        }
        self.intern(row)
    }
}
