use super::*;

impl Checker<'_> {
    /// Explain provision from H's association, retaining the call's rule/primary.
    /// Like rust-analyzer's mismatched_arg_count, expected/provided data belongs
    /// to the checked call; source order is not parameter order (LANG-12).
    fn capability_argument_error(
        &mut self,
        rule: RuleId,
        expression: ExprId,
        call: &CheckedCall,
        ordinal: usize,
        parameter: &Parameter,
        definition: Option<&DefPath>,
    ) {
        let argument = call.arguments.iter().find(|a| {
            a.parameter
                .as_ref()
                .is_some_and(|(i, _)| *i as usize == ordinal)
        });
        let actual = argument.and_then(|a| self.ty(a.expression));
        let names = &self.out.body.resolution.generic_declarations;
        let package = &self.signature.definition.path().0.package;
        let (expected, provided) = if let Some(actual) = actual {
            let [expected, provided] = hirsel_type_analysis::spelling::type_spellings_apart(
                self.t,
                [parameter.ty, actual],
                names,
                package,
                true,
            );
            (expected, Some(provided))
        } else {
            (
                hirsel_type_analysis::spelling::type_spelling(
                    self.t,
                    parameter.ty,
                    names,
                    package,
                    true,
                ),
                None,
            )
        };
        let name = parameter
            .name
            .as_ref()
            .or(parameter.label.as_ref())
            .map_or_else(|| format!("#{}", ordinal + 1), Clone::clone);
        let message = match &provided {
            Some(provided) => format!(
                "capability parameter `{name}` expects `{expected}`, but the supplied argument has type `{provided}`"
            ),
            None => format!(
                "explicit capability argument omitted: parameter `{name}` requires `{expected}`"
            ),
        };
        let before = self.out.diagnostics.len();
        self.error(rule.clone(), self.span(expression), &message);
        if self.out.diagnostics.len() == before {
            return;
        }
        // Clause purity can change the owner and primary; preserve that policy.
        if self.out.diagnostics.last().unwrap().rule != rule {
            return;
        }
        let label = argument.zip(provided).map(|(argument, provided)| Label {
            span: self.span(argument.expression),
            message: format!(
                "this argument supplies `{provided}` for capability parameter `{name}: {expected}`"
            ),
        });
        let advice = definition.zip(actual).and_then(|(definition, actual)| {
            let actual = self.capability_definition(actual)?;
            let expected = self.capability_definition(parameter.ty)?;
            if definition.is_core("out.print") && expected.is_core("Stdout") && actual.is_core("Stderr") {
                Some("help: If stderr is intended, use the declared `err.print` operation with this `Stderr`; choosing the output stream is a program decision.")
            } else if definition.is_core("err.print") && expected.is_core("Stderr") && actual.is_core("Stdout") {
                Some("help: If stdout is intended, use the declared `out.print` operation with this `Stdout`; choosing the output stream is a program decision.")
            } else {
                None
            }
        });
        let diagnostic = self.out.diagnostics.last_mut().unwrap();
        diagnostic.secondary.extend(label);
        if let Some(advice) = advice {
            diagnostic.notes.push(advice.into());
        }
        diagnostic.notes.push(format!("help: Pass a `{expected}` value explicitly for `{name}` and forward it from the caller. A `uses` clause describes authority but does not create a capability value."));
    }
    /// Replacement ends a source receipt, including writes through an existing
    /// mutable iteration view. A preservation declaration keeps it instead.
    pub(super) fn forget_receipt(&mut self, value: ExprId) {
        self.update_receipt(value, None);
    }
    /// A mutable hand-back can replace the old value, but its authority still
    /// comes from this call's operands and declared environment. Retain that
    /// source ceiling without claiming preservation of its erased type facts.
    fn handback_receipt(&mut self, value: ExprId, call: ExprId, authority: AuthoritySummary) {
        let receipt = self.frames.last().and_then(|frame| {
            (authority != AuthoritySummary::Unknown).then_some((frame.owner, authority, call))
        });
        self.update_receipt(value, receipt);
    }
    fn update_receipt(
        &mut self,
        value: ExprId,
        receipt: Option<(NameSite, AuthoritySummary, ExprId)>,
    ) {
        let mut pending = vec![value];
        let mut seen = BTreeSet::new();
        while let Some(value) = pending.pop() {
            let SourceRoot::Binding { owner, binding } = &self.source_place(value).root else {
                continue;
            };
            let Some(binding) = self.root_binding(*owner, binding) else {
                continue;
            };
            let key = binding_key(binding);
            if !seen.insert(key.clone()) {
                continue;
            }
            let previous = self.derivations.remove(&key);
            if let Some(receipt) = &receipt
                && previous.is_some_and(|(owner, _, _)| owner == receipt.0)
            {
                self.derivations.insert(key.clone(), receipt.clone());
            }
            if let Some(source) = self.iteration_sources.get(&key) {
                pending.push(*source);
            }
        }
    }
    /// A selected parameter's unknown environment is already covered by the
    /// enclosing declaration. Discharge only that exact receipt, independently
    /// of every other operand and of the callee's ambient authority.
    pub(super) fn receipt_authority(
        &mut self,
        value: ExprId,
        authority: AuthoritySummary,
        frame: &Frame,
    ) -> AuthoritySummary {
        let mut pending = vec![(value, authority)];
        let mut seen = BTreeSet::new();
        let mut result = AuthoritySummary::inert();
        while let Some((value, authority)) = pending.pop() {
            if authority != AuthoritySummary::Unknown {
                result = result.join(authority);
                continue;
            }
            let value = self.transparent_value(value);
            if !seen.insert(value) {
                continue;
            }
            if self.binding(value).is_some_and(|binding| {
                binding.owner == frame.owner
                    && matches!(binding.binding, ResolvedName::Parameter(p) if frame.receipts.contains(&p))
            }) {
                continue;
            }
            if let Some(source) = self.binding(value).and_then(|binding| {
                self.derivations
                    .get(&binding_key(&binding))
                    .and_then(|(owner, _, source)| (*owner == frame.owner).then_some(*source))
            }) {
                pending.push((source, self.used(source)));
                continue;
            }
            // Construction retains exactly its operands. Discharge selected
            // parameters separately so unknown erased evidence cannot hide a
            // second operand's known authority. A `mut` access charges the
            // same source; its hand-back promises are checked independently.
            let children = match &self.tree.expressions[value.0 as usize].kind {
                ExprKind::Tuple(parts) | ExprKind::Sequence(parts) => Some(parts.clone()),
                ExprKind::Unary {
                    op: UnaryOp::Take | UnaryOp::Copy | UnaryOp::Box | UnaryOp::Mut,
                    value,
                } => Some(vec![*value]),
                ExprKind::Call { arguments, .. }
                    if self
                        .call_row(value)
                        .is_some_and(|call| matches!(call.target, CallTarget::Constructor(_))) =>
                {
                    Some(arguments.iter().map(|a| a.value).collect())
                }
                _ => None,
            };
            if let Some(children) = children {
                pending.extend(children.into_iter().map(|child| (child, self.used(child))));
                continue;
            }
            if let Some(captures) = self
                .captures
                .get(&value)
                .filter(|table| {
                    matches!(
                        table.context,
                        CaptureContext::Escaping
                            | CaptureContext::Task
                            | CaptureContext::Scoped
                            | CaptureContext::OwningScoped
                    )
                })
                .map(|table| table.captures.clone())
            {
                for capture in captures {
                    let selected = capture.owner == frame.owner
                        && matches!(capture.binding, ResolvedName::Parameter(p) if frame.receipts.contains(&p));
                    if !selected {
                        if let Some(source) = self
                            .derivations
                            .get(&binding_key(&BindingDecision {
                                owner: capture.owner,
                                binding: capture.binding.clone(),
                                site: capture.owner,
                                ty: capture.ty,
                                usage: capture.usage,
                                mutable: false,
                            }))
                            .and_then(|(owner, _, source)| {
                                (*owner == frame.owner).then_some(*source)
                            })
                        {
                            pending.push((source, self.used(source)));
                        } else {
                            result = result.join(self.authority(capture.ty));
                        }
                    }
                }
                continue;
            }
            return AuthoritySummary::Unknown;
        }
        result
    }
    fn callback_result_span(&self, call: ExprId) -> Span {
        if let ExprKind::Call { arguments, .. } = &self.tree.expressions[call.0 as usize].kind {
            if let Some(argument) = arguments.last() {
                if let ExprKind::Closure { body, .. } =
                    self.tree.expressions[argument.value.0 as usize].kind
                {
                    return self.result_span(body);
                }
            }
        }
        self.span(call)
    }
    pub(super) fn call_row(&self, e: ExprId) -> Option<&CheckedCall> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows
            .calls
            .get(&e)
            .map(|i| &self.out.body.calls[*i])
    }

    pub(super) fn declarations(&self) -> GenericDeclarations {
        self.out.body.resolution.generic_declarations.clone()
    }
    /// A declared result at one call: its allocator, origins, lifetime,
    /// authority and erased captures come from the actual arguments.
    fn result_at(
        &mut self,
        declared: TypeId,
        allocation_targets: &BTreeMap<ParameterRef, AllocationTarget>,
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
        authorities: &BTreeMap<ParameterRef, AuthoritySummary>,
        captures: &BTreeMap<RelationPlace, CaptureProperties>,
        partitions: &BTreeMap<ParameterRef, Identity>,
    ) -> TypeId {
        let result = instantiate_allocation_targets(self.t, declared, allocation_targets)
            .expect("checked allocator result");
        let result = self.parameter_origins(result, origins, actuals, partitions);
        let types = self.t;
        let result = instantiate_parameter_authorities(types, result, authorities)
            .expect("checked result authority");
        instantiate_parameter_captures(types, result, captures).expect("checked result captures")
    }
    /// H inferred these actuals from the complete receiving header. Rebuild
    /// the declaration with J's authority evidence before checking receipt.
    pub(super) fn instantiate_function_reference(&mut self, e: ExprId) {
        let (declared, arguments) = if let Some(instance) = self
            .out
            .body
            .instantiations
            .iter()
            .find(|row| row.expression == e)
            .cloned()
        {
            let Some(signature) = self.read(DefId::intern(&instance.callee)) else {
                return;
            };
            let SignatureKind::Function(declared) = &signature.kind else {
                return;
            };
            (declared.clone(), instance.arguments)
        } else {
            let arguments = self
                .elaborations
                .get(&e)
                .map(|i| self.out.body.expressions[*i].instantiated_arguments.clone())
                .unwrap_or_default();
            if arguments.is_empty() {
                return;
            }
            let Some(ty) = self.ty(e) else { return };
            let TypeKind::Function(declared) = self.t.get(ty).unwrap().kind.clone() else {
                return;
            };
            (declared, arguments)
        };
        let Some(ty) = self.ty(e) else { return };
        let authorities: Vec<_> = arguments
            .iter()
            .map(|argument| match argument {
                GenericArgument::Type(ty) => self.authority(*ty),
                GenericArgument::Size(_) => AuthoritySummary::inert(),
            })
            .collect();
        for (parameter, argument) in declared.generics.iter().zip(&arguments) {
            if let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(actual)) =
                (parameter, argument)
            {
                for bound in bounds {
                    let (valid, rule) = match bound {
                        Bound::Sendable => (
                            self.receives_sendability(*actual, &Sendability::Sendable),
                            const { RuleId::of("MODE-4") },
                        ),
                        Bound::Inert => (
                            subset(&self.authority(*actual), &AuthoritySummary::inert()),
                            const { RuleId::of("CAP-2") },
                        ),
                        _ => continue,
                    };
                    if !valid {
                        self.error(
                            rule,
                            self.span(e),
                            "function value actual lacks its declared bound",
                        );
                    }
                }
            }
        }
        let mut function = declared.clone();
        for parameter in &mut function.parameters {
            parameter.ty = Ok::<TypeId, GraphError>(parameter.ty)
                .expect("checked function reference parameter");
            parameter.ty = self
                .substitute(parameter.ty, &arguments, &authorities)
                .expect("checked function reference instantiation");
        }
        let result =
            Ok::<TypeId, GraphError>(function.result).expect("checked function reference result");
        function.result = self
            .substitute(result, &arguments, &authorities)
            .expect("checked function reference result instantiation");
        function.error = result_error(self.t, function.result, &self.signature.metadata.roles);
        let parameters: Vec<_> = (0..function.parameters.len())
            .map(|ordinal| {
                AuthoritySummary::Known(
                    vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(
                        ordinal as u32,
                    )))]
                    .into(),
                )
            })
            .collect();
        function.authority = substitute_authority(&function.authority, &parameters, &authorities);
        match &function.uses {
            Uses::Unrestricted => function.uses = Uses::Only(function.authority.clone()),
            Uses::Only(allowed) => {
                let allowed = substitute_authority(allowed, &parameters, &authorities);
                function.authority = allowed.clone();
                function.uses = Uses::Only(allowed);
            }
        }
        function.generics = vec![].into();
        let mut row = self.t.get(ty).unwrap().clone();
        row.kind = TypeKind::Function(function);
        let ty = self.intern(row);
        self.set_ty(e, ty);
    }
    /// A named function used as a value is the capture-free closure that
    /// forwards its parameters to the definition. No source expression
    /// states that call, so its result is typed here, at the value's own
    /// parameters, and published as the closure's body result.
    pub(super) fn forwarding_closure(&mut self, e: ExprId) {
        let Some(ty) = self.ty(e) else { return };
        let TypeKind::Function(f) = self.t.get(ty).unwrap().kind.clone() else {
            return;
        };
        let scope = self.lexical.site(NameSite::Expression(e));
        let mut allocation_targets = BTreeMap::new();
        let mut origins = BTreeMap::new();
        let mut actuals = BTreeMap::new();
        let mut authorities = BTreeMap::new();
        let mut captures = BTreeMap::new();
        for (ordinal, parameter) in f.parameters.iter().enumerate() {
            let Ok(actual) = bind_parameter_scopes(self.t, parameter.ty, &[scope]) else {
                return;
            };
            let declared = ParameterRef::bound(ParamId(ordinal as u32));
            let row = self.t.get(actual).unwrap().clone();
            if let Some(target) = row.allocation_target {
                allocation_targets.insert(declared, target);
            }
            origins.insert(
                declared,
                OriginArgument {
                    identity: Origin {
                        root: OriginRoot::Parameter(ParameterRef::body(
                            scope,
                            ParamId(ordinal as u32),
                        )),
                        fields: vec![].into(),
                    },
                    brand: row.brand,
                    relation: if self.arena_type(actual) {
                        // An Arena argument bounds a result that names it by
                        // its own place: fresh blocks go there (PROV-4).
                        Relation::place(Origin {
                            root: OriginRoot::Parameter(ParameterRef::body(
                                scope,
                                ParamId(ordinal as u32),
                            )),
                            fields: vec![].into(),
                        })
                    } else {
                        row.modes.relation
                    },
                },
            );
            actuals.insert(declared, actual);
            let authority = self.authority(actual);
            authorities.insert(declared, authority.clone());
            let sendability = self.proof(Sendability::NotSendable, |memo| {
                hirsel_modes::proofs::sendability(memo, actual)
            });
            captures.insert(
                declared.into(),
                CaptureProperties::from_type(self.t.get(actual).unwrap(), authority, sendability),
            );
        }
        let result = self.result_at(
            f.result,
            &allocation_targets,
            &origins,
            &actuals,
            &authorities,
            &captures,
            &BTreeMap::new(),
        );

        if !matches!(self.resolved.get(&e), Some(ResolvedName::Definition(_))) {
            let formal = bind_parameter_scopes(self.t, f.result, &[scope])
                .expect("checked forwarding result scope");
            self.out.result_types.push((scope.site, formal));
        }
        if let Some(mut closure) = self.closures.get_mut(&e) {
            closure.result_evidence = Some(result);
            closure.ty = ty;
        } else {
            self.closures.insert(
                e,
                ClosureInfo {
                    context: hirsel_types::CaptureContext::Scoped,
                    result_evidence: Some(result),
                    expression: e,
                    captures: vec![],
                    ty,
                },
            );
        }
    }
    /// Instantiate a declared graph already in the body store with a call's
    /// generic arguments. Substituted nominal rows take their canonical plain
    /// spelling from their declarations; ownership derives summaries later,
    /// in each row's own generic scope.
    fn substitute(
        &mut self,
        declared: TypeId,
        arguments: &[GenericArgument],
        authorities: &[AuthoritySummary],
    ) -> Result<TypeId, GraphError> {
        let store = self.t;
        let substituted = substitute(
            store,
            self.m,
            declared,
            &Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &self.signature.metadata.roles,
                arguments,
                authorities,
            },
        )?;
        let canonical = canonical_plain_type(store, substituted, &mut |p| {
            Ok::<_, hirsel_types::QueryError>(self.read(p))
        })
        .unwrap_or_else(|error| self.proof_failure(error, substituted));
        Ok(canonical)
    }
    pub(super) fn cursor_result(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
    ) -> Option<TypeId> {
        let authorities: Vec<_> = arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => self.authority(*t),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let store = self.t;
        let roles = self.signature.metadata.roles.clone();
        let generics = self.declarations();
        let result = super::cursors::result(
            store,
            self.m,
            &roles,
            &generics,
            path,
            arguments,
            &authorities,
            origins,
            actuals,
            &mut |path| Ok(self.read(path)),
        );
        match result {
            Ok(result) => Some(result),
            Err(error) => {
                self.out.failure.get_or_insert(error);
                None
            }
        }
    }
    /// A mapper receives the owned error payload, not the enclosing Result.
    /// Its occurrence requirements still apply even though no ordinary call
    /// expression or argument appears in the source.
    pub(super) fn error_mapper(&mut self, expression: ExprId, value: ExprId) {
        let Some(path) = self
            .out
            .body
            .resolution
            .paths
            .iter()
            .find_map(|(site, name)| {
                if *site == NameSite::Expression(expression)
                    && let ResolvedName::Definition(path) = name
                {
                    Some(path.clone())
                } else {
                    None
                }
            })
        else {
            return;
        };
        let Some(signature) = self.read(DefId::intern(&path)) else {
            return;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return;
        };
        let [parameter] = function.parameters.as_slice() else {
            return;
        };
        let Some(source) = self.ty(value) else { return };
        let TypeKind::Named { arguments, .. } = &self.t.get(source).unwrap().kind else {
            return;
        };
        let [_, GenericArgument::Type(error)] = arguments.as_slice() else {
            return;
        };
        let declared_error = *error;
        let error = self.projected_item(source, declared_error);
        let error = if !self.property(error, Bound::Plain)
            && !matches!(
                self.t.get(error).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                }
            ) {
            // The failure branch takes a part, just like a taken pattern.
            // Current deep uniqueness comes from its owning Result, rather
            // than the error type argument's written holder contract.
            let mut row = self.t.get(error).unwrap().clone();
            row.modes.uniqueness = self.t.get(source).unwrap().modes.uniqueness;
            self.intern(row)
        } else {
            error
        };
        let arguments = self
            .out
            .body
            .instantiations
            .iter()
            .find(|row| row.expression == expression && row.callee == path)
            .map(|row| row.arguments.clone())
            .unwrap_or_default();
        let authorities: Vec<_> = arguments
            .iter()
            .map(|argument| match argument {
                GenericArgument::Type(ty) => self.authority(*ty),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let Some(expected) = Ok::<TypeId, GraphError>(parameter.ty).ok() else {
            return;
        };
        let expected = if arguments.is_empty() {
            expected
        } else {
            self.substitute(expected, &arguments, &authorities)
                .expect("checked mapper instantiation")
        };
        let span = self.span(expression);
        let saved = self.receiving.replace(Receiving {
            argument: true,
            ..Receiving::new(
                format!("the error parameter of `{}`", path.0.name),
                self.span(expression),
            )
        });
        self.compatible(error, expected, span, None, false);
        self.receiving = saved;
        // H checks ordinary structural bounds; crossing and authority bounds
        // use the actual error's evidence here, as for explicit calls.
        for (formal, actual) in function.generics.iter().zip(&arguments) {
            let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(actual)) =
                (formal, actual)
            else {
                continue;
            };
            for bound in bounds {
                let (valid, rule) = match bound {
                    Bound::Sendable => (
                        self.receives_sendability(*actual, &Sendability::Sendable),
                        const { RuleId::of("MODE-4") },
                    ),
                    Bound::Inert => (
                        subset(&self.authority(*actual), &AuthoritySummary::inert()),
                        const { RuleId::of("CAP-2") },
                    ),
                    _ => continue,
                };
                if !valid {
                    self.explained(
                        rule,
                        span,
                        "error mapper actual lacks its declared bound",
                        Explanation::default()
                            .provided(Fact::new(
                                if *bound == Bound::Sendable {
                                    Cause::NotSendable
                                } else {
                                    Cause::Authority
                                },
                                self.span(value),
                                &[],
                            ))
                            .required(Fact::new(
                                if *bound == Bound::Sendable {
                                    Requirement::Sendable
                                } else {
                                    Requirement::Uses
                                },
                                self.span(expression),
                                &[],
                            )),
                    );
                }
            }
        }
    }
    fn callable(&mut self, callee: ExprId) -> Option<(FunctionType, Option<DefPath>)> {
        if let Some(ResolvedName::Definition(d)) = self.resolved.get(&callee).cloned() {
            let s = self.read(DefId::intern(&d))?;
            if let SignatureKind::Function(f) = &s.kind {
                let mut imported = f.clone();
                for p in &mut imported.parameters {
                    p.ty = Ok::<TypeId, GraphError>(p.ty).ok()?;
                }
                imported.result = Ok::<TypeId, GraphError>(f.result).ok()?;
                return Some((imported, Some(d)));
            }
        }
        // A dotted callee (`h.action`) resolves to its root binding, whose
        // row is the holder's: only a whole binding is called by its row.
        let whole = self
            .source_rows
            .places
            .get(&callee)
            .is_none_or(|i| self.out.body.places[*i].projections.is_empty());
        let t = self
            .binding(callee)
            .filter(|_| whole)
            .and_then(|b| self.binding_types.get(&binding_key(&b)).copied())
            .or_else(|| self.ty(callee))?;
        if let TypeKind::Function(f) = &self.t.get(t)?.kind {
            Some((f.clone(), None))
        } else {
            None
        }
    }
    fn argument_construction_type(
        &mut self,
        expected: TypeId,
        arguments: &[Argument],
        expression: ExprId,
    ) -> TypeId {
        if matches!(self.t.get(expected).unwrap().kind, TypeKind::Function(_)) {
            return expected;
        }
        let partitions: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, argument)| {
                self.value_partition(argument.value)
                    .map(|partition| (ParameterRef::bound(ParamId(i as u32)), partition))
            })
            .collect();
        let mut origins = BTreeMap::new();
        let mut actuals = BTreeMap::new();
        let mut targets = BTreeMap::new();
        for (i, argument) in arguments.iter().enumerate() {
            let parameter = ParameterRef::bound(ParamId(i as u32));
            let Some(ty) = self.ty(argument.value) else {
                continue;
            };
            let row = self.t.get(ty).unwrap().clone();
            actuals.insert(parameter, ty);
            if let Some(target) = row.allocation_target {
                targets.insert(parameter, target);
            }
            if let Some(identity) = self.identity(argument.value) {
                // An Arena argument bounds a result that names it by its own
                // place: fresh blocks go there (PROV-4).
                let relation = if self.arena_type(ty) {
                    Relation::place(identity.clone())
                } else {
                    row.modes.relation
                };
                origins.insert(
                    parameter,
                    OriginArgument {
                        identity,
                        brand: row.brand,
                        relation,
                    },
                );
            }
        }
        // A copied or taken constructor call is built under the same context.
        let mut expression = expression;
        while let ExprKind::Unary {
            op: UnaryOp::Copy | UnaryOp::Take,
            value,
        } = self.tree.expressions[expression.0 as usize].kind
        {
            expression = value;
        }
        if let ExprKind::Call { callee, .. } = self.tree.expressions[expression.0 as usize].kind {
            if self.callable(callee).is_none() {
                if let Some(actual) = self.ty(expression) {
                    // H may have seeded nested construction contracts from an
                    // uninstantiated call parameter. Rewrite only that namespace;
                    // caller-owned generic type arguments and operand rows stay intact.
                    let actual = instantiate_allocation_targets(self.t, actual, &targets)
                        .expect("checked constructor allocator context");
                    let actual = self.parameter_origins(actual, &origins, &actuals, &partitions);
                    self.set_ty(expression, actual);
                }
            }
        }
        let expected = instantiate_allocation_targets(self.t, expected, &targets)
            .expect("checked construction allocator context");
        let expected = self.parameter_origins(expected, &origins, &actuals, &partitions);
        expected
    }
    pub(super) fn call(
        &mut self,
        e: ExprId,
        callee: ExprId,
        arguments: &[Argument],
        expected: Option<TypeId>,
    ) {
        let Some(call) = self.call_row(e).cloned() else {
            self.out
                .failure
                .get_or_insert(QueryError::Operational(OperationalFailure {
                    operation: "authority checked call".into(),
                    message: "missing H call mapping".into(),
                }));
            return;
        };
        if matches!(call.target, CallTarget::Constructor(_)) {
            self.constructor(e, callee, arguments, expected);
            return;
        }
        let Some((f, definition)) = self.callable(callee) else {
            for arg in arguments {
                self.expr(arg.value, None);
            }
            return;
        };
        let intrinsic = definition
            .as_ref()
            .and_then(|d| self.intrinsics.get(d))
            .copied();
        self.verdicts.last_mut().unwrap().call_prerequisites = Some(true);
        let source_arguments = arguments;
        let mut ordered: Vec<_> = call
            .arguments
            .iter()
            .filter_map(|a| {
                let (ordinal, _) = a.parameter.as_ref()?;
                source_arguments
                    .iter()
                    .find(|arg| arg.value == a.expression)
                    .map(|arg| (*ordinal as usize, arg.clone()))
            })
            .collect();
        ordered.sort_by_key(|(p, _)| *p);
        let ordered_arguments: Vec<_> = ordered.iter().map(|(_, a)| a.clone()).collect();
        let arguments = ordered_arguments.as_slice();
        if matches!(call.target, CallTarget::Definition(_)) {
            self.direct_callee = Some(callee);
        }
        self.expr(callee, None);
        self.direct_callee = None;
        // A called value's type is complete only after the value is evaluated:
        // a closure literal's inferred result is its body's. The call row's
        // declared result follows the callee, for ownership and lowering.
        let f = if matches!(call.target, CallTarget::Definition(_)) {
            f
        } else {
            let f = self.callable(callee).map_or(f, |(f, _)| f);
            if let Some(row) = self
                .out
                .body
                .calls
                .iter_mut()
                .find(|row| row.expression == e)
            {
                row.result = f.result;
            }
            f
        };
        let mut used = self.used(callee);
        for arg in source_arguments {
            let parameter = call
                .arguments
                .iter()
                .find(|a| a.expression == arg.value)
                .and_then(|a| a.parameter.as_ref().map(|(_, p)| p));
            let authority_arguments: BTreeMap<_, _> = ordered
                .iter()
                .filter_map(|(i, a)| {
                    self.ty(a.value)
                        .map(|t| (ParameterRef::bound(ParamId(*i as u32)), self.authority(t)))
                })
                .collect();
            let expected = parameter.map(|p| {
                instantiate_parameter_authorities(self.t, p.ty, &authority_arguments)
                    .expect("checked callback authority")
            });
            let expected =
                expected.map(|ty| self.argument_construction_type(ty, arguments, arg.value));
            let mut context = expected.and_then(|t| {
                let ty = self.t.get(t)?;
                let TypeKind::Function(_) = &ty.kind else {
                    return Some((
                        CaptureContext::Escaping,
                        Some(t),
                        const { RuleId::of("CLO-4") },
                    ));
                };
                let ctx = if matches!(
                    intrinsic,
                    Some(Intrinsic::TaskAsync | Intrinsic::TaskIsolate)
                ) {
                    CaptureContext::Task
                } else if !ty.modes.relation.is_global()
                    && ty.modes.multiplicity == Multiplicity::Once
                {
                    CaptureContext::OwningScoped
                } else if parameter.unwrap().passing != PassingMode::Take {
                    CaptureContext::Scoped
                } else {
                    CaptureContext::Escaping
                };
                let callback = if let TypeKind::Function(f) = &ty.kind {
                    f.clone()
                } else {
                    unreachable!()
                };
                let rule = if intrinsic == Some(Intrinsic::MutexWith) {
                    const { RuleId::of("CONC-5") }
                } else if callback.parameters.iter().any(|p| self.scoped(p.ty)) {
                    const { RuleId::of("CAP-3") }
                } else {
                    const { RuleId::of("CLO-2") }
                };
                Some((ctx, expected, rule))
            });
            // The reviewed arena callback's R is produced by its body. An
            // omitted result header is H's provisional inference, not a written
            // receiving contract whose unresolved brands bind callback parameters.
            // Capture mode was determined above; the complete receiving type is
            // checked below after the actual result refines R.
            let inferred_scope_result = definition
                .as_ref()
                .and_then(|path| hirsel_types::core_roles::arena_scope_callback(path))
                .is_some_and(|slot| {
                    call.arguments.iter().any(|argument| {
                        argument.expression == arg.value
                            && argument
                                .parameter
                                .as_ref()
                                .is_some_and(|(ordinal, _)| *ordinal == slot)
                    })
                })
                && matches!(&self.tree.expressions[arg.value.0 as usize].kind,
                    ExprKind::Closure { signature, .. } if signature.result.is_none());
            if inferred_scope_result && let Some((_, expected, _)) = &mut context {
                *expected = None;
            }
            self.expr(arg.value, context);
            used = used.join(self.used(arg.value));
        }
        if arguments.iter().any(|a| !self.valid(a.value)) {
            self.abandon_tasks(arguments);
            return;
        }
        if call.state == CallState::Recovery {
            self.abandon_tasks(arguments);
            return;
        }
        if call.state == CallState::MissingCapabilities {
            self.abandon_tasks(arguments);
            let supplied: BTreeSet<_> = call
                .arguments
                .iter()
                .filter_map(|a| a.parameter.as_ref().map(|(ordinal, _)| *ordinal))
                .collect();
            let missing = f
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(ordinal, p)| {
                    (!supplied.contains(&(ordinal as u32))
                        && self.capability_definition(p.ty).is_some())
                    .then_some(ParameterRef::bound(ParamId(ordinal as u32)))
                })
                .collect();
            let rule = self.capability_call_rule(intrinsic, &f, &missing);
            let (ordinal, parameter) = f
                .parameters
                .iter()
                .enumerate()
                .find(|(ordinal, p)| {
                    !supplied.contains(&(*ordinal as u32))
                        && self.capability_definition(p.ty).is_some()
                })
                .expect("missing capability parameter");
            self.capability_argument_error(rule, e, &call, ordinal, parameter, definition.as_ref());
            for a in &call.arguments {
                if let Some((_, p)) = &a.parameter {
                    if matches!(p.passing, PassingMode::Mut | PassingMode::Take) {
                        self.invalidate(a.expression, self.span(e), p.passing == PassingMode::Mut);
                    }
                }
            }
            // Supplied operands still have access effects. Judgments depending on
            // the invalid invocation/result are suppressed under section 23.
            if let Some(ty) = self.ty(e) {
                let mut row = self.t.get(ty).unwrap().clone();
                row.kind = TypeKind::Error;
                let error = self.intern(row);
                self.set_ty(e, error);
            }
            return;
        }
        let args: Vec<_> = arguments
            .iter()
            .map(|a| self.ty(a.value).unwrap())
            .collect();
        let authorities: Vec<_> = args.iter().map(|t| self.authority(*t)).collect();
        let authority_arguments: BTreeMap<_, _> = authorities
            .iter()
            .enumerate()
            .map(|(i, a)| (ParameterRef::bound(ParamId(i as u32)), a.clone()))
            .collect();
        let mut capture_arguments: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .map(|(i, t)| {
                // Preserve proven occurrence evidence before the declaration proof.
                let sendability = if self.sendable(*t) {
                    Sendability::Sendable
                } else {
                    self.proof(Sendability::NotSendable, |memo| {
                        hirsel_modes::proofs::sendability(memo, *t)
                    })
                };
                (
                    ParameterRef::bound(ParamId(i as u32)).into(),
                    CaptureProperties::from_type(
                        self.t.get(*t).unwrap(),
                        authorities[i].clone(),
                        sendability,
                    ),
                )
            })
            .collect();
        for (state, callback, slot) in mutable_capture_states(self.t, &f) {
            let store = self.t;
            let proof = mutable_capture_state_sendability(
                store,
                args[callback],
                slot,
                &self.declarations(),
                &mut |path| Ok(self.read(path)),
            )
            .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
            if let Some(capture) = capture_arguments.get_mut(&hirsel_types::RelationPlace {
                parameter: ParameterRef::bound(ParamId(state as u32)),
                fields: vec![].into(),
            }) {
                capture.sendability = combine_crossing_evidence(capture.sendability.clone(), proof);
            }
        }
        let mut allocation = f.allocation;
        for (parameter, actual) in f.parameters.iter().zip(&args) {
            if parameter.passing != PassingMode::Take {
                if let TypeKind::Function(callback) = &self.t.get(*actual).unwrap().kind {
                    if callback.allocation == Allocation::MayAllocate {
                        allocation = Allocation::MayAllocate;
                    }
                }
            }
        }
        self.out.call_allocations.retain(|(site, _)| *site != e);
        self.out.call_allocations.push((e, allocation));
        let mut generic_arguments = call.generic_arguments.clone();

        // Rustc15c1044 ty/erase_regions.rs:26-34: skip a transformation
        // with no possible target. Every write below requires a generic slot.
        if !generic_arguments.is_empty() {
            let mut pending: Vec<_> = f
                .parameters
                .iter()
                .zip(&args)
                .map(|(p, a)| (p.ty, *a, 0))
                .collect();
            while let Some((formal, actual, depth)) = pending.pop() {
                #[cfg(test)]
                GENERIC_REFINEMENT_ROWS.with(|rows| rows.set(rows.get() + 1));
                match self.t.get(formal).unwrap().kind.clone() {
                    TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(depth) => {
                        if let Some(GenericArgument::Type(argument)) =
                            generic_arguments.get_mut(p.parameter().0 as usize)
                        {
                            *argument = refine_inferred_value_contract(self.t, *argument, actual)
                                .expect("checked generic occurrence");
                        }
                    }
                    TypeKind::Function(f) => {
                        if let TypeKind::Function(a) = &self.t.get(actual).unwrap().kind {
                            pending.extend(
                                f.parameters
                                    .iter()
                                    .map(|p| p.ty)
                                    .chain([f.result])
                                    .zip(a.parameters.iter().map(|p| p.ty).chain([a.result]))
                                    .map(|(a, b)| (a, b, depth + 1)),
                            );
                        }
                    }
                    kind => pending.extend(
                        kind.value_parts()
                            .into_iter()
                            .zip(self.t.get(actual).unwrap().kind.value_parts())
                            .map(|(a, b)| (a, b, depth)),
                    ),
                }
            }
            // GEN-1/PROV-1: a type argument carries the brands of the values it
            // is instantiated at. The first parameter in declaration order that
            // supplies a single brand fixes it, and every other argument is then
            // checked against it, so a value of another owner cannot enter through
            // a generic parameter. A type argument no parameter mentions types no
            // value the callee receives, so it can produce none: the expected
            // result type instantiates it, and any brand holds vacuously.
            let mut mentioned = BTreeSet::new();
            for parameter in &f.parameters {
                let mut pending = vec![(parameter.ty, 0)];
                while let Some((ty, depth)) = pending.pop() {
                    #[cfg(test)]
                    GENERIC_REFINEMENT_ROWS.with(|rows| rows.set(rows.get() + 1));
                    let row = self.t.get(ty).unwrap();
                    if let TypeKind::Parameter(p) = &row.kind {
                        if p.binder() == GenericBinder::Bound(depth) {
                            mentioned.insert(p.parameter().0 as usize);
                        }
                    }
                    pending.extend(
                        row.kind
                            .graph_edges()
                            .into_iter()
                            .map(|edge| (edge.ty, depth + edge.role.parameter_binders())),
                    );
                }
            }
            let supplied = f
                .parameters
                .iter()
                .map(|p| p.ty)
                .zip(args.iter().copied())
                .map(|pair| (pair, true))
                .chain(
                    expected
                        .map(|expected| ((f.result, expected), false))
                        .into_iter(),
                );
            for ((formal, actual), argument) in supplied.collect::<Vec<_>>() {
                let mut pending = vec![(formal, actual)];
                while let Some((formal, actual)) = pending.pop() {
                    #[cfg(test)]
                    GENERIC_REFINEMENT_ROWS.with(|rows| rows.set(rows.get() + 1));
                    let (Some(formal_row), Some(actual_row)) =
                        (self.t.get(formal).cloned(), self.t.get(actual).cloned())
                    else {
                        continue;
                    };
                    match formal_row.kind {
                        TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(0) => {
                            let index = p.parameter().0 as usize;
                            if argument != mentioned.contains(&index) {
                                continue;
                            }
                            if let Some(GenericArgument::Type(instance)) =
                                generic_arguments.get_mut(index)
                            {
                                *instance = self.adopt_brands(*instance, actual);
                            }
                        }
                        TypeKind::Function(_) => {}
                        kind => {
                            if std::mem::discriminant(&kind)
                                == std::mem::discriminant(&actual_row.kind)
                            {
                                pending.extend(
                                    kind.value_parts()
                                        .into_iter()
                                        .zip(actual_row.kind.value_parts()),
                                );
                            }
                        }
                    }
                }
            }
        }

        // A reviewed arena scope returns exactly its callback's result. H's
        // generic inference precedes checking that callback; retain the closed
        // brands its result header now promises, including inside Region[T].
        // Ordinary callable parameters remain invariant and are not producers
        // from which arbitrary generic brands may be inferred.
        if let Some(slot) = definition
            .as_ref()
            .and_then(|path| hirsel_types::core_roles::arena_scope_callback(path))
            && let (Some(formal), Some(actual)) =
                (f.parameters.get(slot as usize), args.get(slot as usize))
            && let (TypeKind::Function(formal), TypeKind::Function(actual)) =
                (&self.t.row(formal.ty).kind, &self.t.row(*actual).kind)
            && formal.generics.is_empty()
            && let TypeKind::Parameter(parameter) = self.t.row(formal.result).kind
            && parameter.binder() == GenericBinder::Bound(1)
            && let Some(GenericArgument::Type(instance)) =
                generic_arguments.get_mut(parameter.parameter().0 as usize)
        {
            *instance = self.adopt_brands(*instance, actual.result);
        }
        // The seal stores the complete callable environment without invoking
        // it. Inference ran before capture analysis; retain intrinsic Once in
        // both the taking parameter and Region's payload type, rather than
        // turning it into a many-callable receiving requirement.
        if intrinsic == Some(Intrinsic::ArenaSeal)
            && let (Some(formal), Some(actual)) = (f.parameters.get(1), args.get(1))
            && let TypeKind::Parameter(parameter) = self.t.row(formal.ty).kind
            && let Some(GenericArgument::Type(argument)) =
                generic_arguments.get_mut(parameter.parameter().0 as usize)
            && matches!(self.t.row(*actual).kind, TypeKind::Function(_))
            && self.t.row(*actual).modes.multiplicity == Multiplicity::Once
        {
            let mut row = self.t.row(*argument).clone();
            row.modes.multiplicity = Multiplicity::Once;
            *argument = self.intern(row);
        }
        if let Some(row) = self
            .out
            .body
            .instantiations
            .iter_mut()
            .find(|i| i.expression == e && definition.as_ref() == Some(&i.callee))
        {
            row.arguments = generic_arguments.clone();
        }
        let mut refined_parameters = call.arguments.clone();
        if generic_arguments != call.generic_arguments {
            let authorities = generic_arguments
                .iter()
                .map(|a| match a {
                    GenericArgument::Type(t) => self.authority(*t),
                    _ => AuthoritySummary::inert(),
                })
                .collect::<Vec<_>>();
            for argument in &mut refined_parameters {
                if let Some((ordinal, parameter)) = &mut argument.parameter {
                    parameter.ty = self
                        .substitute(
                            f.parameters[*ordinal as usize].ty,
                            &generic_arguments,
                            &authorities,
                        )
                        .expect("refined checked parameter");
                }
            }
        }
        let row = self
            .out
            .body
            .calls
            .iter_mut()
            .find(|row| row.expression == e)
            .unwrap();
        row.generic_arguments = generic_arguments.clone();
        row.arguments = refined_parameters;
        let generic_authorities: Vec<_> = generic_arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => self.authority(*t),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let allocation_targets: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                self.t
                    .get(*t)
                    .unwrap()
                    .allocation_target
                    .map(|a| (ParameterRef::bound(ParamId(i as u32)), a))
            })
            .collect();
        let actuals: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .map(|(i, t)| (ParameterRef::bound(ParamId(i as u32)), *t))
            .collect();
        let partitions: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, argument)| {
                self.value_partition(argument.value)
                    .map(|partition| (ParameterRef::bound(ParamId(i as u32)), partition))
            })
            .collect();
        let origins: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let parameter = ParameterRef::bound(ParamId(i as u32));
                self.identity(a.value).map(|origin| {
                    (
                        parameter,
                        OriginArgument {
                            brand: self
                                .ty(a.value)
                                .map(|t| self.t.get(t).unwrap().brand.clone())
                                .unwrap_or_default(),
                            relation: match self.ty(a.value) {
                                // An Arena argument bounds a result that names
                                // it by its own place (PROV-4).
                                Some(t) if self.arena_type(t) => Relation::place(origin.clone()),
                                Some(t) => self.t.get(t).unwrap().modes.relation.clone(),
                                None => Relation::environment(),
                            },
                            identity: origin,
                        },
                    )
                })
            })
            .collect();
        for (a, origin) in arguments.iter().zip(0u32..).filter_map(|(a, i)| {
            origins
                .get(&ParameterRef::bound(ParamId(i)))
                .map(|o| (a, o.identity.root.clone()))
        }) {
            if matches!(&origin, OriginRoot::Expression(_, x) if *x == a.value
                || self.source_place(a.value).root == SourceRoot::Value(*x))
                && !self.temporaries.contains(&origin)
            {
                self.temporaries.push(origin);
            }
        }
        let mut required = self.scoped_authority(substitute_authority(
            &f.authority,
            &authorities,
            &generic_authorities,
        ));
        let mut transparent = false;
        for (i, p) in f.parameters.iter().enumerate() {
            let pty = self.t.get(p.ty).unwrap();
            if matches!(pty.kind, TypeKind::Function(_)) && p.passing == PassingMode::Borrow {
                transparent = true;
                if let Some(a) = authorities.get(i) {
                    required = required.join(a.clone());
                }
            }
        }
        // Calling an unrestricted scoped function parameter has unknown authority.
        if definition.is_none()
            && self
                .ty(callee)
                .is_some_and(|t| !self.t.get(t).unwrap().modes.relation.is_global())
        {
            required = match &f.uses {
                Uses::Unrestricted => AuthoritySummary::Unknown,
                Uses::Only(a) => self.scoped_authority(substitute_authority(
                    a,
                    &authorities,
                    &generic_authorities,
                )),
            };
            transparent = true;
        }
        let callee_authority = if definition.is_none() {
            self.ty(callee)
                .map(|t| self.authority(t))
                .unwrap_or(AuthoritySummary::inert())
        } else {
            AuthoritySummary::inert()
        };
        let provided = authorities
            .iter()
            .cloned()
            .fold(callee_authority, |a, b| a.join(b));
        let missing = f
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                let capability = self.capability_definition(parameter.ty)?;
                let supplied = args
                    .get(index)
                    .and_then(|actual| self.capability_definition(*actual));
                (supplied.as_ref() != Some(&capability))
                    .then_some(ParameterRef::bound(ParamId(index as u32)))
            })
            .collect();
        let provision_rule = self.capability_call_rule(intrinsic, &f, &missing);
        let span = self.span(e);
        let mut obligation_valid = self.verdicts.last().unwrap().call_prerequisites.unwrap();
        if obligation_valid {
            // A handle's retained authority is not provision of its governing
            // capability. Missing capability operands belong to CAP before
            // derivative arity/type/brand failures (v20 section 23).
            for (index, parameter) in f.parameters.iter().enumerate() {
                if let Some(capability) = self.capability_definition(parameter.ty) {
                    let supplied = args
                        .get(index)
                        .and_then(|actual| self.capability_definition(*actual));
                    if supplied.as_ref() != Some(&capability) {
                        self.capability_argument_error(
                            provision_rule.clone(),
                            e,
                            &call,
                            index,
                            parameter,
                            definition.as_ref(),
                        );
                        obligation_valid = false;
                        break;
                    }
                }
            }
        }
        // CAP-1: what the callee needs beyond its operands must be handed in
        // as one. An operand's own authority is supplied by that operand,
        // known or not (an open `mut` function parameter relayed onwards), and
        // an operand of unknown authority hides none of the others.
        let ambient = self.scoped_authority(substitute_authority(
            &f.authority,
            &vec![AuthoritySummary::inert(); f.parameters.len()],
            &generic_authorities,
        ));
        let supplied = authorities
            .iter()
            .chain([&provided])
            .filter(|a| **a != AuthoritySummary::Unknown)
            .cloned()
            .fold(AuthoritySummary::inert(), |a, b| a.join(b));
        if obligation_valid
            && definition.is_some()
            && !(transparent && required == AuthoritySummary::Unknown)
            && !subset(&required, &provided)
            && !subset(&ambient, &supplied)
        {
            self.error(
                provision_rule,
                span,
                "the declared governing capability must be supplied as a typed argument",
            );
            obligation_valid = false;
        }
        if obligation_valid {
            for (i, p) in f.parameters.iter().enumerate() {
                let Some(actual) = args.get(i) else { continue };
                // Rewrite the callee binder before inserting caller-owned generic
                // values: their Parameter roots belong to the caller, not this call.
                let declared = instantiate_allocation_targets(self.t, p.ty, &allocation_targets)
                    .expect("checked allocator arguments");
                let declared = self.parameter_origins(declared, &origins, &actuals, &partitions);
                let declared =
                    instantiate_parameter_authorities(self.t, declared, &authority_arguments)
                        .expect("checked call authority");
                let declared = instantiate_parameter_captures(self.t, declared, &capture_arguments)
                    .expect("checked call captures");
                let expected = if generic_arguments.is_empty() {
                    declared
                } else {
                    self.substitute(declared, &generic_arguments, &generic_authorities)
                        .unwrap_or(declared)
                };
                let receiving_expected = expected;
                // A proved whole-Iter preservation contract is quantified over
                // the actual incoming envelope, even when the formal is narrower.
                let expected = if p.passing == PassingMode::Mut
                    && f.state_promises.iter().any(|promise| {
                        promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                            && promise.selector.fields.is_empty()
                            && promise.preserve_captures
                    })
                    && matches!(
                        self.t.get(*actual).unwrap().kind,
                        TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        } | TypeKind::Named { .. }
                    ) {
                    // An erased carrier's cleanup is its declaration's; only
                    // its evidence is per value.
                    let incoming = self.t.get(*actual).unwrap();
                    let mut row = self.t.get(expected).unwrap().clone();
                    row.erased_captures = incoming.erased_captures.clone();
                    if !matches!(row.kind, TypeKind::Named { .. }) {
                        row.drop_allocation = incoming.drop_allocation.clone();
                    }
                    self.intern(row)
                } else {
                    expected
                };
                // CLO-4: a `mut` part of an erased carrier comes back within
                // the authority the carrier carries, unless the callee
                // preserves the part's whole incoming envelope.
                if p.passing == PassingMode::Mut
                    && let Some(bound) = self.carrier_place(arguments[i].value)
                    && !f.state_promises.iter().any(|promise| {
                        promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                            && promise.selector.fields.is_empty()
                            && promise.preserve_captures
                    })
                    && !self.authority(receiving_expected).is_subset_of(&bound)
                {
                    self.explained(
                        const { RuleId::of("CLO-4") },
                        self.span(arguments[i].value),
                        "a `mut` part of an erased carrier must come back within the authority it carries",
                        Explanation::default()
                            .provided(Fact::new(Cause::Authority, self.span(callee), &[]))
                            .required(Fact::new(Requirement::Uses, self.span(arguments[i].value), &[])),
                    );
                }
                let actual = self.receive_literal_result(arguments[i].value, *actual, expected);
                let actual = &actual;
                // A taken value is the callee's to keep, so an unplaced take
                // parameter is storage like any other; a borrow cannot keep it.
                let saved = self.receiving.replace(Receiving {
                    description: format!(
                        "a parameter of `{}`",
                        self.spelled(callee).unwrap_or_else(|| "the callee".into())
                    ),
                    span: self.span(callee),
                    value: Some(arguments[i].value),
                    annotated: None,
                    argument: true,
                    borrowed: p.passing == PassingMode::Borrow,
                    result: false,
                });
                // Seal transfers the designated arena's storage. I proves
                // closure over that grant and the heap before ownership flow
                // publishes its result; an ordinary taken parameter cannot do this.
                let kept = intrinsic == Some(Intrinsic::ArenaSeal) && i == 1
                    || p.passing != PassingMode::Take
                    || {
                        let argument = self.span(arguments[i].value);
                        self.keeps_placement(*actual, expected, argument, false)
                    };
                let valid = kept
                    && self.compatible(*actual, receiving_expected, span, intrinsic, false)
                    && (p.passing != PassingMode::Mut
                        || self.keeps_handback_promises(
                            *actual,
                            expected,
                            span,
                            arguments[i].value,
                            callee,
                        ))
                    && self.keeps_nested_brands(*actual, expected, p.passing, arguments[i].value)
                    && {
                        let valid = self.keeps_nested_uniqueness(
                            *actual,
                            expected,
                            p.passing != PassingMode::Mut,
                            self.span(arguments[i].value),
                        );
                        if !valid {
                            self.explain_contextual_contract(e, i, p.ty, *actual, expected);
                        }
                        valid
                    };
                self.receiving = saved;
                if !valid {
                    obligation_valid = false;
                    break;
                }
            }
        }
        if !obligation_valid {
            self.abandon_tasks(arguments);
        }
        if obligation_valid {
            let reported = self.out.diagnostics.len();
            self.generic_obligations(e, &f, arguments, intrinsic);
            // Literal reception may replace its header row. Construction
            // evidence stays indexed by the received row, not this old snapshot.
            let received_args: Vec<_> = arguments.iter().filter_map(|a| self.ty(a.value)).collect();
            self.static_intrinsic(e, intrinsic, &received_args, &f);
            // CONC-2: a handle or task set passed `take` to a parameter declared `Task` or
            // `TaskSet` carries its await-or-detach obligation into the callee.
            if self.out.diagnostics.len() > reported {
                self.abandon_tasks(arguments);
            } else if let Some(declared) = definition
                .as_ref()
                .and_then(|d| self.read(DefId::intern(d)))
            {
                if let SignatureKind::Function(df) = &declared.kind {
                    for (i, a) in &ordered {
                        if df.parameters.get(*i).is_some_and(|p| {
                            p.passing == PassingMode::Take && task_obligation(self.t, p.ty)
                        }) {
                            for task in self.task_handles(a.value) {
                                self.pending_tasks.remove(&task);
                            }
                        }
                    }
                }
            }
            if let Some(frame) = self.frames.last().cloned() {
                if let Uses::Only(allowed) = &frame.allowed {
                    let receipt_arguments: Vec<_> = arguments
                        .iter()
                        .zip(&authorities)
                        .map(|(argument, authority)| {
                            self.receipt_authority(argument.value, authority.clone(), &frame)
                        })
                        .collect();
                    let mut receipt_required = self.scoped_authority(substitute_authority(
                        &f.authority,
                        &receipt_arguments,
                        &generic_authorities,
                    ));
                    if let Uses::Only(uses) = &f.uses {
                        let selected: Vec<_> = arguments
                            .iter()
                            .map(|argument| {
                                self.receipt_authority(
                                    argument.value,
                                    self.used(argument.value),
                                    &frame,
                                )
                            })
                            .collect();
                        receipt_required = receipt_required.join(self.scoped_authority(
                            substitute_authority(uses, &selected, &generic_authorities),
                        ));
                    }
                    for (parameter, authority) in f.parameters.iter().zip(&receipt_arguments) {
                        if parameter.passing == PassingMode::Borrow
                            && matches!(
                                self.t.get(parameter.ty).unwrap().kind,
                                TypeKind::Function(_)
                            )
                        {
                            receipt_required = receipt_required.join(authority.clone());
                        }
                    }
                    let selected_callee =
                        self.receipt_authority(callee, AuthoritySummary::Unknown, &frame)
                            == AuthoritySummary::inert();
                    if definition.is_none()
                        && (selected_callee
                            || self.ty(callee).is_some_and(|ty| {
                                !self.t.get(ty).unwrap().modes.relation.is_global()
                            }))
                    {
                        let callee_required = match &f.uses {
                            // An inferred scoped closure publishes its captures
                            // as uses, which stays Unknown for a callback parameter.
                            // Discharge that environment only through its exact
                            // origin receipt, as for an unrestricted parameter.
                            Uses::Unrestricted | Uses::Only(AuthoritySummary::Unknown) => {
                                self.receipt_authority(callee, AuthoritySummary::Unknown, &frame)
                            }
                            Uses::Only(uses) => self.scoped_authority(substitute_authority(
                                uses,
                                &receipt_arguments,
                                &generic_authorities,
                            )),
                        };
                        // Discharging the callee's environment does not erase
                        // known declaration charges on its actual parameters.
                        // An unrestricted selected callback has unknown ambient
                        // evidence, discharged only by that exact receipt.
                        receipt_required = if f.authority == AuthoritySummary::Unknown {
                            callee_required
                        } else {
                            receipt_required.join(callee_required)
                        };
                        // Selecting the callee never selects callbacks supplied to it.
                        for (ty, authority) in args.iter().zip(&receipt_arguments) {
                            if matches!(self.t.get(*ty).unwrap().kind, TypeKind::Function(_)) {
                                receipt_required = receipt_required.join(authority.clone());
                            }
                        }
                    }
                    // CAP-2/CLO-2, spec #58: captures were checked when the
                    // frame was created. Parameters and calls may derive new
                    // capability values inside it; their operation's input
                    // authority is checked by provision, not as ambient uses.
                    let ambient = if self.in_clause
                        || matches!(frame.owner, NameSite::Item)
                        || transparent && receipt_required == AuthoritySummary::Unknown
                    {
                        receipt_required.clone()
                    } else if definition.is_none()
                        && self
                            .ty(callee)
                            .is_some_and(|ty| !self.t.get(ty).unwrap().modes.relation.is_global())
                    {
                        match &f.uses {
                            Uses::Unrestricted => {
                                self.receipt_authority(callee, AuthoritySummary::Unknown, &frame)
                            }
                            Uses::Only(_) => AuthoritySummary::inert(),
                        }
                    } else {
                        self.scoped_authority(substitute_authority(
                            &f.authority,
                            &vec![AuthoritySummary::inert(); f.parameters.len()],
                            &generic_authorities,
                        ))
                    };
                    // CAP-5/CLO-2: an owner projection may derive Dir from an
                    // authorized Seal parameter. Prove the actual input values,
                    // never a type-level Dir <= Seal relation. Environment
                    // authority required beyond those inputs remains ambient.
                    let derived_inputs = !self.in_clause
                        && required != AuthoritySummary::Unknown
                        && matches!(frame.owner, NameSite::Item)
                        && arguments
                            .iter()
                            .all(|a| subset(&self.used(a.value), allowed))
                        && (definition.is_some() || subset(&self.used(callee), allowed))
                        && subset(
                            &self.scoped_authority(substitute_authority(
                                &f.authority,
                                &vec![AuthoritySummary::inert(); f.parameters.len()],
                                &generic_authorities,
                            )),
                            allowed,
                        );
                    if !derived_inputs && !subset(&ambient, allowed) {
                        let rule = if matches!(frame.restriction.as_str(), "CONC-5" | "CAP-3") {
                            frame.restriction.clone()
                        } else if transparent {
                            const { RuleId::of("CLO-2") }
                        } else {
                            frame.restriction.clone()
                        };
                        if rule.as_str() == "CONC-5" {
                            self.explained(
                                rule,
                                span,
                                "call authority exceeds the declared uses restriction",
                                Explanation::default()
                                    .provided(Fact::new(Cause::Authority, self.span(callee), &[]))
                                    .required(Fact::new(
                                        Requirement::Uses,
                                        self.sites().owner_span(frame.owner),
                                        &[],
                                    )),
                            );
                        } else {
                            self.error(
                                rule,
                                span,
                                "call authority exceeds the declared uses restriction",
                            );
                        }
                    }
                }
            }
        }
        // Specialize the type contract before applying its argument-derived
        // placement and capture evidence, including callable type actuals.
        let declared_result = if generic_arguments.is_empty() {
            f.result
        } else {
            self.substitute(f.result, &generic_arguments, &generic_authorities)
                .unwrap_or(f.result)
        };
        let result = self.result_at(
            declared_result,
            &allocation_targets,
            &origins,
            &actuals,
            &authority_arguments,
            &capture_arguments,
            &partitions,
        );
        let result = declared_receipt_evidence(self.t, result).expect("declared call result");
        let result = self.mut_content_placement(result, &f, &ordered);
        let result = self.renew_result_brands(declared_result, result);
        self.set_ty(e, result);
        if obligation_valid && task_obligation(self.t, result) {
            self.pending_tasks.insert(e, span);
            self.task_values.insert(e, BTreeSet::from([e]));
        }
        let mut handback_authority = used.clone().join(required.clone());
        if let Uses::Only(uses) = &f.uses {
            let sources: Vec<_> = arguments.iter().map(|a| self.used(a.value)).collect();
            handback_authority = handback_authority.join(
                self.scoped_authority(substitute_authority(uses, &sources, &generic_authorities)),
            );
        }
        // Brand invalidation happens after checking all argument obligations. Container
        // operations that mutate Slots preserve identity; a move/replacement does not.
        for (i, p) in f.parameters.iter().enumerate() {
            if let Some((_, a)) = ordered.iter().find(|(ordinal, _)| *ordinal == i) {
                if matches!(p.passing, PassingMode::Mut | PassingMode::Take) {
                    if self.protected_field(a.value) {
                        self.explained(
                            const { RuleId::of("PROV-5") },
                            span,
                            "a sibling brand owner cannot be moved or passed mut separately from its record",
                            Explanation::default()
                    .provided(Fact::new(Cause::SiblingOwner, self.span(a.value), &[]))
                    .required(Fact::new(Requirement::Owned, span, &[])),
                        );
                    }
                    let advances = p.passing == PassingMode::Mut
                        && f.state_promises.iter().any(|promise| {
                            promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                                && promise.selector.fields.is_empty()
                                && promise.advance_only
                        });
                    if !advances {
                        self.invalidate(a.value, span, p.passing == PassingMode::Mut);
                    }
                    if p.passing == PassingMode::Mut
                        && !f.state_promises.iter().any(|promise| {
                            promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                                && promise.selector.fields.is_empty()
                                && promise.preserve_captures
                        })
                    {
                        self.handback_receipt(a.value, e, handback_authority.clone());
                    }
                }
            }
        }
        // Preserve nominal provider attempts and their failure policy.
        let _ = self.authority(result);
        self.facts.insert(e, used.join(required));
    }
    /// PROV-4, caller side: content moved in through any relation-free `mut`
    /// parameter may come back out through any of them, or as the result. The
    /// arenas and regions each such argument's content is placed in also place
    /// the result and every such argument's storage after the call; their
    /// parts inherit the root's placement (`placed_part_type`).
    fn mut_content_placement(
        &mut self,
        result: TypeId,
        f: &FunctionType,
        ordered: &[(usize, Argument)],
    ) -> TypeId {
        // (placement of the content, its type, the storage it was passed from)
        let mut contents = vec![];
        for (ordinal, argument) in ordered {
            let Some(parameter) = f.parameters.get(*ordinal) else {
                continue;
            };
            let related = self.t.get(parameter.ty).is_some_and(|t| {
                t.modes
                    .relation
                    .reaches()
                    .iter()
                    .any(|r| matches!(r, Reach::Place(_)))
            });
            if parameter.passing != PassingMode::Mut || related {
                continue;
            }
            let Some(actual) = self.ty(argument.value) else {
                continue;
            };
            let mut placement = Relation::global();
            let mut pending = vec![actual];
            let mut seen = std::collections::HashSet::new();
            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) {
                    continue;
                }
                let Some(row) = self.t.get(ty) else {
                    continue;
                };
                if matches!(row.kind, TypeKind::Function(_)) {
                    continue;
                }
                placement = placement.union(&self.placement(&row.modes.relation));
                pending.extend(row.kind.value_parts());
            }
            let mut root = argument.value;
            while let ExprKind::Unary { value, .. } = self.tree.expressions[root.0 as usize].kind {
                root = value;
            }
            let mut holder = None;
            if self.source_rows.places.contains_key(&root) {
                if let SourceRoot::Binding { owner, binding } = &self.source_place(root).root {
                    holder = self.root_binding(*owner, binding).cloned();
                }
            }
            contents.push((placement, actual, holder));
        }
        if contents
            .iter()
            .all(|(placement, _, _)| placement.is_global())
        {
            return result;
        }
        // Content reaches only storage whose type can hold a part of it.
        let store = self.t;
        let memo = self.m;
        let placements = |store: &TypeInterner, this: &mut Self, holder: TypeId| {
            let mut placement = Relation::global();
            for (from, ty, _) in &contents {
                if !from.is_global()
                    && hirsel_type_analysis::representation::may_hold_content(
                        store,
                        memo,
                        holder,
                        *ty,
                        &mut |path| this.read(path),
                    )
                {
                    placement = placement.union(from);
                }
            }
            placement
        };
        let at_result = placements(store, self, result);
        let at_holders: Vec<_> = contents
            .iter()
            .filter_map(|(_, ty, holder)| Some((holder.clone()?, *ty)))
            .map(|(holder, ty)| {
                let placement = placements(store, self, ty);
                (holder, placement)
            })
            .collect();
        let place = |this: &mut Self, ty: TypeId, placement: &Relation| {
            if placement.is_global() || this.property(ty, Bound::Plain) {
                return ty;
            }
            let mut row = this.t.get(ty).unwrap().clone();
            row.modes.relation = row.modes.relation.union(placement);
            this.t.intern(row)
        };
        for (holder, placement) in at_holders {
            let key = binding_key(&holder);
            let held = self.binding_types.get(&key).copied().unwrap_or(holder.ty);
            let placed = place(self, held, &placement);
            self.binding_types.insert(key, placed);
        }
        place(self, result, &at_result)
    }
    /// An unheld literal can choose its result promise at the receiving call.
    /// Keep its executable result and complete bound evidence independently.
    fn receive_literal_result(
        &mut self,
        expression: ExprId,
        actual: TypeId,
        expected: TypeId,
    ) -> TypeId {
        if !matches!(&self.tree.expressions[expression.0 as usize].kind,
            ExprKind::Closure { signature, .. } if signature.result.is_none())
        {
            return actual;
        }
        let mut row = self.t.get(actual).unwrap().clone();
        let (TypeKind::Function(function), TypeKind::Function(promise)) =
            (&mut row.kind, &self.t.get(expected).unwrap().kind)
        else {
            return actual;
        };
        let (result, expected_result) = (function.result, promise.result);
        // The callable's own generics scope this comparison, so it has its
        // own contextual memo.
        let generics = self.declarations().nested(&function.generics);
        let reader = self.reader;
        let mut lookup = |d: DefId| Ok(reader.borrow_mut().read(d).ok());
        let mut memo = ContextMemo::new(self.t, &generics, &mut lookup);
        let compatible = memo.view(expected_result, result, TypeRelation::Value);
        hirsel_modes::proofs::count_memo(0, memo.hits());
        drop(memo);
        self.record_reads();
        let compatible = compatible.unwrap_or_else(|error| self.proof_failure(error, false));
        if !compatible {
            return actual;
        }
        function.result = expected_result;
        function.error = result_error(self.t, expected_result, &self.signature.metadata.roles);
        let received = self.intern(row);
        self.set_ty(expression, received);
        if let Some(mut closure) = self.closures.get_mut(&expression) {
            closure.ty = received;
        }
        received
    }
    /// CLO-1: an inferred local closure result is local to the environment,
    /// which a closure literal binds when it is created, as a call binds the
    /// arguments of an `in x` result. When the body's result can depend on
    /// nothing but the environment (no borrowed non-plain parameter) and every
    /// captured source is global, the closure's result is global.
    fn environment_result(
        &mut self,
        result: TypeId,
        f: &FunctionType,
        captures: &[CapturedName],
        views: &[(BindingKey, TypeId, TypeId)],
    ) -> TypeId {
        let Some(row) = self.t.get(result).cloned() else {
            return result;
        };
        if !self.lifetime(&row.modes.relation).is_environment() {
            return result;
        }
        let borrowed_parameter = f
            .parameters
            .iter()
            .any(|p| p.passing != PassingMode::Take && !self.property(p.ty, Bound::Plain));
        let sources = captures
            .iter()
            .map(|c| {
                views
                    .iter()
                    .find(|(_, view, _)| *view == c.ty)
                    .map_or(c.ty, |(_, _, source)| *source)
            })
            .collect::<Vec<_>>();
        let global = !borrowed_parameter
            && !views.is_empty()
            && sources
                .iter()
                .all(|t| hirsel_types::structurally_global(self.t, *t, &|o| self.arena_place(o)));
        if !global {
            return result;
        }
        let mut row = row;
        row.modes.relation = self.placement(&row.modes.relation);
        self.intern(row)
    }
    /// MEM-5 evidence that the body of an inferred closure literal cannot
    /// allocate, on any path. The proof is by construction over a closed set
    /// of forms: no block is constructed outside an arena (a String or Bytes
    /// literal, a sequence, `box`, an escaping closure), nothing is deep-copied
    /// (`unique`), nothing but a Plain binding is written (no detach), every
    /// call is `@noalloc` after the CLO-2 join, no `for` advances a cursor,
    /// and every value the frame owns is destroyed without allocating. A
    /// body outside this set has no evidence; writing `@noalloc` on the
    /// receiving type asks for the complete audit of that body instead.
    fn noalloc_evidence(&mut self, owner: &NameSite, body: ExprId) -> bool {
        let generics = self.declarations();
        let silent = |row: &Type| match &row.drop_allocation {
            DropAllocation::NoAlloc => true,
            DropAllocation::MayAllocate => false,
            DropAllocation::DependsOn(parameters) => parameters.iter().all(|p| {
                let bounds = generics.bounds(*p);
                bounds.contains(&Bound::NoallocDrop) || bounds.contains(&Bound::Plain)
            }),
        };
        // A closure literal builds its environment in this frame (CLO-1)
        // only where it cannot leave it: called on the spot, or bound by an
        // unannotated `let` whose every use is a call of it.
        let mut scoped = BTreeSet::new();
        let mut callees = BTreeSet::new();
        let mut called_only = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(e) = stack.pop() {
            let row = self.ty(e).and_then(|ty| self.t.get(ty)).cloned();
            let row = row.as_ref();
            match &self.tree.expressions[e.0 as usize].kind {
                // A block placed in an arena is not a heap allocation.
                ExprKind::Literal(
                    hirsel_syntax::Literal::String(_) | hirsel_syntax::Literal::Bytes(_),
                )
                | ExprKind::Sequence(_)
                | ExprKind::Unary {
                    op: UnaryOp::Box, ..
                } if row.is_none_or(|row| self.placement(&row.modes.relation).is_global()) => {
                    return false;
                }
                ExprKind::Unary {
                    op: UnaryOp::Unique,
                    ..
                }
                | ExprKind::Comptime(_)
                | ExprKind::Shape(_)
                | ExprKind::Heap
                | ExprKind::Todo(_)
                | ExprKind::Error => return false,
                ExprKind::Closure { .. } => {
                    // Its body runs in its own frame; only the environment
                    // is built here, and a global one is a counted block.
                    if !scoped.contains(&e)
                        || row.is_none_or(|row| self.lifetime(&row.modes.relation).is_global())
                    {
                        return false;
                    }
                    continue;
                }
                ExprKind::Path(_)
                    if !callees.contains(&e)
                        && self
                            .binding(e)
                            .is_some_and(|binding| called_only.contains(&binding.site)) =>
                {
                    return false;
                }
                ExprKind::Call { callee, .. } => {
                    callees.insert(*callee);
                    scoped.insert(*callee);
                    // A constructor builds its value inline; one that
                    // checks a fact (FACT-2) calls that check.
                    let constructor = self
                        .call_row(e)
                        .is_some_and(|call| matches!(call.target, CallTarget::Constructor(_)))
                        && !row.is_some_and(|row| match &row.kind {
                            TypeKind::Named { definition, .. } => {
                                self.read(*definition).is_none_or(|signature| {
                                    !matches!(&signature.kind,
                                        SignatureKind::Type(d) if d.fact.is_none())
                                })
                            }
                            _ => false,
                        });
                    let noalloc = self
                        .out
                        .call_allocations
                        .iter()
                        .any(|(site, a)| *site == e && *a == Allocation::NoAlloc);
                    if !(constructor || noalloc) {
                        return false;
                    }
                }
                _ => (),
            }
            // Every value computed here is destroyed here on some path (a
            // discarded result, a failing later operand): only a read of a
            // binding's storage, which borrows, and a callee make none.
            let borrows = !callees.contains(&e)
                && self.source_rows.places.get(&e).is_some_and(|i| {
                    matches!(self.out.body.places[*i].root, SourceRoot::Binding { .. })
                        && matches!(
                            self.tree.expressions[e.0 as usize].kind,
                            ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
                        )
                });
            if !callees.contains(&e) && !borrows && !row.is_some_and(silent) {
                return false;
            }
            match &self.tree.expressions[e.0 as usize].kind {
                ExprKind::Block { statements, .. } => {
                    for statement in statements {
                        match self.tree.statements[statement.0 as usize].kind {
                            StmtKind::For { .. } => return false,
                            StmtKind::Assign { place, .. } => {
                                let root = match &self.source_place(place).root {
                                    SourceRoot::Binding { owner, binding } => {
                                        self.root_binding(*owner, binding).map(|b| {
                                            self.binding_types
                                                .get(&binding_key(b))
                                                .copied()
                                                .unwrap_or(b.ty)
                                        })
                                    }
                                    _ => None,
                                };
                                if !root.is_some_and(|ty| self.property(ty, Bound::Plain)) {
                                    return false;
                                }
                            }
                            StmtKind::Let {
                                pattern,
                                ty: None,
                                value,
                                ..
                            } if matches!(
                                self.tree.expressions[value.0 as usize].kind,
                                ExprKind::Closure { .. }
                            ) =>
                            {
                                scoped.insert(value);
                                called_only.insert(NameSite::Pattern(pattern));
                            }
                            _ => (),
                        }
                    }
                }
                _ => (),
            }
            let nested = matches!(
                self.tree.expressions[e.0 as usize].kind,
                ExprKind::Block { .. }
            )
            .then(|| self.local_function_bodies(e))
            .unwrap_or_default();
            stack.extend(
                self.expression_children(e)
                    .into_iter()
                    .filter(|child| !nested.contains(child)),
            );
        }
        let taken = |parameter: ParamId| match owner {
            NameSite::Expression(e) => match &self.tree.expressions[e.0 as usize].kind {
                ExprKind::Closure { signature, .. } => signature
                    .parameters
                    .get(parameter.0 as usize)
                    .is_some_and(|p| p.passing == PassingMode::Take),
                _ => true,
            },
            _ => true,
        };
        self.out.body.bindings.iter().all(|binding| {
            binding.owner != *owner
                || matches!(binding.binding, ResolvedName::Parameter(p) if !taken(p))
                || self
                    .t
                    .get(
                        self.binding_types
                            .get(&binding_key(binding))
                            .copied()
                            .unwrap_or(binding.ty),
                    )
                    .is_some_and(silent)
        })
    }
    /// The bodies of the local functions a block declares: other frames.
    fn local_function_bodies(&self, block: ExprId) -> Vec<ExprId> {
        let ExprKind::Block { statements, .. } = &self.tree.expressions[block.0 as usize].kind
        else {
            return vec![];
        };
        statements
            .iter()
            .filter_map(|s| match self.tree.statements[s.0 as usize].kind {
                StmtKind::Function { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }
    fn generic_obligations(
        &mut self,
        e: ExprId,
        f: &FunctionType,
        args: &[Argument],
        intrinsic: Option<Intrinsic>,
    ) {
        let sharing = sharing_type_parameters(self.t, f);
        let substitutions = self
            .out
            .body
            .calls
            .iter()
            .find(|call| call.expression == e)
            .map(|call| call.generic_arguments.clone())
            .unwrap_or_default();
        let store = self.t;
        let declarations = self.declarations();
        for parameter in sharing {
            if let Some(GenericArgument::Type(actual)) = substitutions.get(parameter.0 as usize) {
                let forbidden =
                    has_owned_crossing_requirement(store, *actual, &declarations, &mut |path| {
                        Ok(self.read(path))
                    })
                    .unwrap_or_else(|error| self.proof_failure(error, true));
                if forbidden {
                    let site = self.span(e);
                    self.explained(const { RuleId::of("MODE-4") }, site, "a generic parameter with copying permission cannot carry this implicit sendable receiving requirement; write sendable on its required occurrences or remove the copying permission",
                        Explanation::default().provided(Fact::new(Cause::NotSendable, site, &[]))
                            .required(Fact::new(Requirement::SendableType, site, &["the receiving type"])));
                    return;
                }
            }
        }
        fn evidence_children(kind: &TypeKind) -> Vec<TypeId> {
            if let TypeKind::Function(f) = kind {
                return f
                    .parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .collect();
            }
            kind.value_parts()
        }
        fn occurrences(
            store: &TypeInterner,
            root: TypeId,
            param: TypeParamId,
            depth: u32,
            active: &mut HashSet<TypeId>,
        ) -> Vec<Vec<usize>> {
            if !active.insert(root) {
                return vec![];
            }
            let Some(t) = store.get(root) else {
                return vec![];
            };
            let result = if t.kind
                == TypeKind::Parameter(TypeParameterRef::Bound {
                    depth,
                    parameter: param,
                }) {
                vec![vec![]]
            } else {
                evidence_children(&t.kind)
                    .iter()
                    .enumerate()
                    .flat_map(|(i, t)| {
                        occurrences(
                            store,
                            *t,
                            param,
                            depth
                                + u32::from(matches!(
                                    store.get(root).unwrap().kind,
                                    TypeKind::Function(_)
                                )),
                            active,
                        )
                        .into_iter()
                        .map(move |mut p| {
                            p.insert(0, i);
                            p
                        })
                    })
                    .collect()
            };
            active.remove(&root);
            result
        }
        for (i, g) in f.generics.iter().enumerate() {
            let GenericParameter::Type { bounds, .. } = g else {
                continue;
            };
            let dependent_arguments: Vec<_> = f
                .parameters
                .iter()
                .zip(args)
                .filter_map(|(p, a)| {
                    (!occurrences(
                        self.t,
                        p.ty,
                        TypeParamId(i as u32),
                        0,
                        &mut HashSet::default(),
                    )
                    .is_empty())
                    .then(|| Label {
                        span: self.span(a.value),
                        message: "generic parameter instantiation failed".into(),
                    })
                })
                .collect();
            for (p, arg) in f.parameters.iter().zip(args) {
                for path in occurrences(
                    self.t,
                    p.ty,
                    TypeParamId(i as u32),
                    0,
                    &mut HashSet::default(),
                ) {
                    let Some(mut actual) = self.ty(arg.value) else {
                        continue;
                    };
                    let primary = if intrinsic == Some(Intrinsic::MutexWith)
                        && matches!(self.t.get(actual).unwrap().kind, TypeKind::Function(_))
                    {
                        self.callback_result_span(e)
                    } else {
                        self.span(e)
                    };
                    actual = self.complete_value_type(actual);
                    // Only the last callback's whole result is the same proof
                    // as static_intrinsic's callback-result bound. A parameter
                    // or a nested field is an independent generic occurrence.
                    let callback_result = args.last().is_some_and(|a| a.value == arg.value)
                        && matches!(&self.t.get(actual).unwrap().kind,
                            TypeKind::Function(f) if path.as_slice() == [f.parameters.len()]);
                    let mut valid = true;
                    for index in &path {
                        if let Some(child) =
                            evidence_children(&self.t.get(actual).unwrap().kind).get(*index)
                        {
                            actual = *child
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if !valid {
                        continue;
                    }
                    if bounds.contains(&Bound::Serializable)
                        && !self.property(actual, Bound::Serializable)
                    {
                        self.error(const { RuleId::of("SER-1") }, self.span(e),
                            "actual generic argument needs a transparent serializable schema without brands or authority");
                        return;
                    }
                    let owner = match intrinsic {
                        Some(Intrinsic::MutexWith | Intrinsic::MutexCreate) => {
                            const { RuleId::of("CONC-5") }
                        }
                        Some(Intrinsic::ChannelOpen | Intrinsic::ChannelSend) => {
                            const { RuleId::of("CONC-6") }
                        }
                        _ => const { RuleId::of("MODE-4") },
                    };
                    // An argument's own root is I's to judge (MODE-2).
                    let sendable = if path.is_empty() {
                        self.sendable_when_unique(actual)
                    } else {
                        self.sendable(actual)
                    };
                    if bounds.contains(&Bound::Sendable) && !sendable {
                        let explanation = if callback_result
                            && matches!(
                                intrinsic,
                                Some(
                                    Intrinsic::TaskAsync
                                        | Intrinsic::TaskIsolate
                                        | Intrinsic::MutexWith
                                )
                            ) {
                            self.callback_sendability_explanation(
                                e,
                                actual,
                                self.span(arg.value),
                                primary,
                            )
                        } else {
                            Explanation::default()
                                .provided(Fact::new(Cause::NotSendable, self.span(arg.value), &[]))
                                .required(Fact::new(Requirement::Sendable, primary, &[]))
                        };
                        self.explained(
                            owner, primary,
                            "each actual generic occurrence must be sendable before parameter weakening",
                            explanation,
                        );
                        if self.out.failure.is_none() && !self.failed_read {
                            let bound = self.out.diagnostics.last_mut().unwrap();
                            bound.secondary.extend(dependent_arguments.clone());
                            bound.obligations.push(Obligation::GenericBound {
                                arguments: dependent_arguments.iter().map(|l| l.span).collect(),
                            });
                        }
                        return;
                    }
                    if bounds.contains(&Bound::Inert)
                        && self.authority(actual) != AuthoritySummary::inert()
                    {
                        self.explained(
                            if intrinsic == Some(Intrinsic::SharedFreeze) {
                                const { RuleId::of("MEM-11") }
                            } else {
                                const { RuleId::of("CAP-2") }
                            },
                            self.span(e),
                            "inert requires an empty declared authority summary",
                            Explanation::default()
                                .provided(Fact::new(Cause::Authority, self.span(arg.value), &[]))
                                .required(Fact::new(Requirement::Inert, self.span(e), &[])),
                        );
                        if self.out.failure.is_none() && !self.failed_read {
                            let bound = self.out.diagnostics.last_mut().unwrap();
                            bound.secondary.extend(dependent_arguments.clone());
                            bound.obligations.push(Obligation::GenericBound {
                                arguments: dependent_arguments.iter().map(|l| l.span).collect(),
                            });
                        }
                        return;
                    }
                }
            }
        }
    }
    fn static_intrinsic(
        &mut self,
        e: ExprId,
        intrinsic: Option<Intrinsic>,
        args: &[TypeId],
        f: &FunctionType,
    ) {
        let span = self.span(e);
        match intrinsic {
            Some(Intrinsic::SharedFreeze) => {
                if let Some(t) = args.first() {
                    if self.contains_resource(*t) {
                        self.explained(
                            const { RuleId::of("MEM-11") },
                            span,
                            "Shared construction cannot hold resources",
                            Explanation::default()
                                .provided(Fact::new(Cause::Resource, span, &[]))
                                .required(Fact::new(Requirement::NoResources, span, &[])),
                        );
                    }
                }
            }
            Some(Intrinsic::ArenaSeal) => {
                if args.get(1).is_some_and(|t| self.contains_resource(*t)) {
                    self.explained(
                        const { RuleId::of("MEM-4") },
                        span,
                        "an arena seal cannot hold resources",
                        Explanation::default()
                            .provided(Fact::new(Cause::Resource, span, &[]))
                            .required(Fact::new(Requirement::NoResources, span, &[])),
                    );
                }
            }
            Some(Intrinsic::MutexCreate) => {
                if let Some(t) = args.get(1) {
                    if !self.sendable_when_unique(*t) {
                        self.explained(
                            const { RuleId::of("CONC-5") },
                            span,
                            "mutex payload must be unique and sendable",
                            self.payload_explanation(
                                e,
                                1,
                                Cause::NotSendable,
                                Requirement::Sendable,
                            ),
                        );
                    }
                }
            }
            Some(Intrinsic::ChannelSend) => {
                if let Some(t) = args.get(2) {
                    if !self.sendable_when_unique(*t) {
                        self.explained(const { RuleId::of("CONC-6") }, span, "channel payload must be sendable; Receivers cross only by task capture",
                            self.payload_explanation(e, 2, Cause::NotSendable, Requirement::Sendable));
                    }
                }
            }
            Some(Intrinsic::ChannelOpen) => {
                let ts = self
                    .out
                    .body
                    .instantiations
                    .iter()
                    .find(|i| {
                        i.expression == e && self.intrinsics.get(&i.callee).copied() == intrinsic
                    })
                    .map(|i| i.arguments.clone())
                    .unwrap_or_default();
                for t in ts {
                    if let GenericArgument::Type(t) = t {
                        if !self.sendable(t) {
                            self.explained(
                                const { RuleId::of("CONC-6") },
                                span,
                                "channel payload type must be sendable; for counted data, write `unique` on the payload type (Sender[unique T] and Receiver[unique T]) or on its counted fields",
                                self.boundary_explanation(
                                    None,
                                    span,
                                    Cause::NotSendable,
                                    Requirement::Sendable,
                                ),
                            );
                            break;
                        }
                    }
                }
            }
            Some(Intrinsic::AtomicCreate) => {
                if let Some(t) = args.get(1) {
                    if !matches!(self.t.get(*t).map(|t| &t.kind), Some(TypeKind::Primitive(p)) if !matches!(p, Primitive::Bytes | Primitive::String))
                    {
                        self.error(
                            const { RuleId::of("CONC-7") },
                            span,
                            "atomic payload must be scalar",
                        );
                    }
                }
            }
            Some(Intrinsic::TaskAsync | Intrinsic::TaskIsolate | Intrinsic::MutexWith) => {
                if let Some(t) = args.last() {
                    if let TypeKind::Function(actual) = self.t.get(*t).unwrap().kind.clone() {
                        let result = self
                            .closures
                            .by_type(*t)
                            .and_then(|c| c.result_evidence)
                            .unwrap_or(actual.result);
                        if !self.sendable(result) {
                            let rule = if intrinsic == Some(Intrinsic::MutexWith) {
                                const { RuleId::of("CONC-5") }
                            } else {
                                const { RuleId::of("MODE-4") }
                            };
                            let result_span = if intrinsic == Some(Intrinsic::MutexWith) {
                                self.callback_result_span(e)
                            } else {
                                span
                            };
                            let explanation =
                                self.callback_sendability_explanation(e, result, result_span, span);
                            self.explained(
                                rule,
                                result_span,
                                "the callback result must be sendable",
                                explanation,
                            );
                        }
                    }
                }
            }
            _ => (),
        }
        let _ = f;
    }
    pub(super) fn syntax_uses(&mut self, site: NameSite, paths: &[hirsel_syntax::Path]) -> Uses {
        let mut authority = AuthoritySummary::inert();
        let resolved: Vec<_> = self
            .out
            .body
            .resolution
            .paths
            .iter()
            .filter(|(s, _)| s == &site)
            .map(|(_, n)| n.clone())
            .collect();
        let scopes: Vec<_> = self
            .out
            .body
            .resolution
            .reference_scopes
            .iter()
            .filter(|(s, _)| *s == site)
            .map(|(_, owner)| *owner)
            .collect();
        let mut scopes = scopes.into_iter();
        for (index, path) in paths.iter().enumerate() {
            let name = resolved.get(index).cloned();
            match name {
                Some(ResolvedName::Definition(d)) => {
                    authority = authority.join(AuthoritySummary::Known(
                        vec![AuthorityTerm::Capability(d)].into(),
                    ));
                }
                Some(n) => {
                    let owner = scopes.next();
                    // A function type has no runtime binding rows for its own
                    // parameters. Resolve the header binder before looking up
                    // captured values in the surrounding frame.
                    if let ResolvedName::Parameter(parameter) = n
                        && owner == Some(site)
                    {
                        let reference = if matches!(site, NameSite::Type(_)) {
                            ParameterRef::bound(parameter)
                        } else {
                            ParameterRef::body(self.lexical.site(site), parameter)
                        };
                        authority = authority.join(AuthoritySummary::Known(
                            vec![AuthorityTerm::Parameter(reference)].into(),
                        ));
                        continue;
                    }
                    if let Some(b) = self
                        .out
                        .body
                        .bindings
                        .iter()
                        .find(|b| {
                            work::count(Counter::AuthoritySourceRows);
                            b.binding == n
                                && owner.map_or(
                                    matches!(n, ResolvedName::Local(_)) || b.owner == site,
                                    |owner| b.owner == owner,
                                )
                        })
                        .cloned()
                    {
                        let selected = match b.binding {
                            ResolvedName::Parameter(parameter) if b.owner == site => {
                                AuthoritySummary::Known(
                                    vec![AuthorityTerm::Parameter(ParameterRef::body(
                                        self.lexical.site(site),
                                        parameter,
                                    ))]
                                    .into(),
                                )
                            }
                            _ => self.authority(b.ty),
                        };
                        authority = authority.join(selected);
                    } else {
                        authority = AuthoritySummary::Unknown;
                    }
                }
                None => {
                    let _ = path;
                    authority = AuthoritySummary::Unknown;
                }
            }
        }
        Uses::Only(authority)
    }
    fn constructor(
        &mut self,
        e: ExprId,
        _callee: ExprId,
        arguments: &[Argument],
        expected: Option<TypeId>,
    ) -> bool {
        let Some(mut root) = self.ty(e) else {
            return false;
        };
        if self.capability_definition(root).is_some_and(|path| self.read(DefId::intern(&path)).is_some_and(|s|
            matches!(&s.kind, SignatureKind::Type(d) if d.capability == Some(CapabilityKind::Scoped))))
        {
            let mut row = self.t.get(root).unwrap().clone();
            // A local seal is bounded by this invocation even when every
            // represented field is physically Plain.
            row.modes.relation = row.modes.relation.union(&Relation::environment());
            root = self.intern(row);
            self.set_ty(e, root);
        }
        // The binding places the destination's relation names: a field is
        // within the owner's relation, so it admits what they admit (v22 R3).
        let owner_places = expected.map(|expected| {
            self.t
                .get(expected)
                .unwrap()
                .modes
                .relation
                .retain(|r| match r {
                    Reach::Place(o) => !self.arena_place(o),
                    Reach::Environment => true,
                    Reach::Region(_) => false,
                })
        });
        if let Some(expected) = expected {
            let goal = self.t.get(expected).unwrap().clone();
            let mut row = self.t.get(root).unwrap().clone();
            // Expected placement is a construction requirement; operand modes
            // and nested generic contracts remain independently supplied.
            let placement = self.placement(&goal.modes.relation);
            if self.placement(&row.modes.relation).is_global() && !placement.is_global() {
                row.modes.relation = row.modes.relation.union(&placement);
            }
            if self.t.get(root) != Some(&row) {
                root = self.intern(row);
                self.set_ty(e, root);
            }
        }
        self.construction_destination(e);
        let Some(mut fields) = self.constructor_fields(e, root) else {
            return false;
        };
        if let Some(goal) = expected.filter(|goal| self.brandless_variant(e, *goal).is_some()) {
            root = self.part_brands(root, goal);
            self.set_ty(e, root);
            fields = self.constructor_fields(e, root).unwrap();
        }
        self.verdicts.last_mut().unwrap().call_prerequisites = Some(true);
        let Some(call) = self.call_row(e) else {
            self.out
                .failure
                .get_or_insert(QueryError::Operational(OperationalFailure {
                    operation: "authority constructor".into(),
                    message: "missing H call mapping".into(),
                }));
            return true;
        };
        let supplied: BTreeMap<FieldId, ExprId> = call
            .arguments
            .iter()
            .filter_map(|a| {
                a.parameter
                    .as_ref()
                    .map(|(i, _)| (FieldId(*i), a.expression))
            })
            .collect();
        let mut used = AuthoritySummary::inert();
        for a in arguments {
            let expected = supplied
                .iter()
                .find(|(_, e)| **e == a.value)
                .and_then(|(f, _)| fields.get(f.0 as usize))
                .map(|f| f.ty);
            self.expr(
                a.value,
                Some((
                    CaptureContext::Escaping,
                    expected,
                    const { RuleId::of("CLO-4") },
                )),
            );
            used = used.join(self.used(a.value));
        }
        // a constructed value holds its parts' loans, so it lives
        // where their arena content lives, and each field is within it. The
        // owner is checked where it is kept (#79), as a tuple is. A
        // destination that names an arena already placed the owner, and its
        // fields are checked against it.
        let holdings = supplied
            .iter()
            .filter_map(|(field, part)| {
                let ty = self.ty(*part)?;
                if self.property(ty, Bound::Plain) {
                    return Some((*field, Relation::global()));
                }
                let relation = &self.t.get(ty).unwrap().modes.relation;
                Some((*field, self.placement(relation)))
            })
            .collect();
        self.part_holdings.insert(e, holdings);
        let mut parts = Relation::global();
        let supplied_types: Vec<_> = supplied.values().filter_map(|p| self.ty(*p)).collect();
        for part in supplied_types {
            if !self.property(part, Bound::Plain) {
                let relation = &self.t.get(part).unwrap().modes.relation;
                parts = parts.union(&self.placement(relation));
            }
        }
        let mut row = self.t.get(root).unwrap().clone();
        let relation = row.modes.relation.union(&parts);
        if relation != row.modes.relation {
            // PROV-4: the destination arena and all held parts bound the
            // constructed value, even when the destination already names one.
            row.modes.relation = relation;
            root = self.intern(row);
            self.set_ty(e, root);
            let Some(placed) = self.constructor_fields(e, root) else {
                return false;
            };
            fields = placed;
        }
        let siblings: BTreeMap<_, _> = supplied
            .iter()
            .filter_map(|(f, e)| self.identity(*e).map(|o| (*f, o)))
            .collect();
        if self.verdicts.last().unwrap().call_prerequisites.unwrap() {
            for (&field, &earg) in &supplied {
                let Some(f) = fields.get(field.0 as usize) else {
                    continue;
                };
                let has_sibling = self
                    .t
                    .get(f.ty)
                    .unwrap()
                    .brand
                    .alternatives
                    .iter()
                    .any(|o| matches!(o.root, OriginRoot::Sibling(_)));
                let mut expected = super::facts::siblings(self.t, f.ty, &siblings);
                if let Some(places) = owner_places.as_ref().filter(|p| !p.is_global()) {
                    let mut row = self.t.get(expected).unwrap().clone();
                    if !self.placement(&row.modes.relation).is_global() {
                        row.modes.relation = row.modes.relation.union(places);
                        expected = self.intern(row);
                    }
                }
                if let Some(actual) = self.ty(earg) {
                    self.kept(
                        actual,
                        expected,
                        self.span(e),
                        earg,
                        has_sibling,
                        Receiving::new("the constructed field", self.span(e)),
                        // A field's locality follows its owner, whose row
                        // carries its parts' placement and is checked where
                        // it is kept.
                        true,
                    );
                }
            }
        }
        self.constructor_argument_brands(e, root);
        self.hold_moved_fields(e, &supplied);

        let parts: Vec<_> = arguments.iter().map(|argument| argument.value).collect();
        self.construction_sendability(e, &parts);
        if let Some(root) = self.ty(e) {
            let evidence = self.t.get(root).unwrap().modes.sendability.clone();
            let received =
                declared_receipt_evidence(self.t, root).expect("checked constructor receipt");
            let mut row = self.t.get(received).unwrap().clone();
            row.modes.sendability = evidence;
            let received = self.intern(row);
            self.set_ty(e, received);
        }
        if arguments
            .iter()
            .all(|argument| self.unique_construction_part(argument.value))
        {
            self.set_unique(e);
        }
        self.arena_construction(e);
        self.carrier_construction(e, &parts);
        // Preserve nominal provider attempts and their failure policy.
        let _ = self.authority(root);
        self.facts.insert(e, used);
        true
    }
    /// A field moved into a fresh value carries its owner identity there
    /// (PROV-2: Take transports existing evidence), so the value holds it
    /// until the value is transferred or written. A Plain or copied field
    /// leaves its source owning its identity.
    fn hold_moved_fields(&mut self, e: ExprId, supplied: &BTreeMap<FieldId, ExprId>) {
        let mut held = BTreeSet::new();
        for (field, part) in supplied {
            let copied = matches!(
                self.tree.expressions[self.transparent_value(*part).0 as usize].kind,
                ExprKind::Unary {
                    op: UnaryOp::Copy | UnaryOp::Unique,
                    ..
                }
            );
            if copied
                || self
                    .ty(*part)
                    .is_none_or(|ty| self.property(ty, Bound::Plain))
            {
                continue;
            }
            let Some(partition) = self.value_partition(*part) else {
                continue;
            };
            let at = |path: &[FieldId]| [&[*field][..], path].concat();
            held.insert((at(&[]), partition.whole.clone()));
            held.extend(partition.parts.iter().map(|(p, o)| (at(p), o.clone())));
            held.extend(partition.held.iter().map(|(p, o)| (at(p), o.clone())));
        }
        if held.is_empty() {
            return;
        }
        if let Some(mut partition) = self.value_partition(e) {
            partition.held.extend(held);
            self.expression_partitions.insert(e, partition);
        }
    }
    // Infer payload brands from supplied fields, using the declaration's exact
    // type-parameter occurrences rather than copying a receiving annotation.
    fn constructor_argument_brands(&mut self, e: ExprId, root: TypeId) {
        let Some(call) = self.call_row(e).cloned() else {
            return;
        };
        let CallTarget::Constructor(ContextualName::Variant { variant, .. }) = call.target else {
            return;
        };
        let mut row = self.t.get(root).unwrap().clone();
        let TypeKind::Named {
            definition,
            arguments,
        } = &mut row.kind
        else {
            return;
        };
        let Some(signature) = self.read(*definition) else {
            return;
        };
        let SignatureKind::Type(declaration) = &signature.kind else {
            return;
        };
        let Some(representation) = declaration.representation else {
            return;
        };
        let Some(TypeKind::Variant(variants)) = self.t.get(representation).map(|t| &t.kind) else {
            return;
        };
        let Some(selected) = variants.get(variant.0 as usize) else {
            return;
        };
        let mut pending = Vec::new();
        for argument in &call.arguments {
            if let (Some((field, _)), Some(actual)) =
                (argument.parameter.as_ref(), self.ty(argument.expression))
            {
                if let Some((_, formal)) = selected.fields.get(*field as usize) {
                    pending.push((*formal, actual, 0));
                }
            }
        }
        let mut seen = HashSet::default();
        let mut occurrences: BTreeMap<u32, Vec<TypeId>> = BTreeMap::new();
        while let Some((formal, actual, depth)) = pending.pop() {
            if !seen.insert((formal, actual, depth)) {
                continue;
            }
            let formal = self.t.get(formal).unwrap();
            let actual_row = self.t.get(actual).unwrap();
            if let TypeKind::Parameter(hirsel_types::TypeParameterRef::Bound {
                depth: d,
                parameter,
            }) = formal.kind
            {
                if d == depth {
                    occurrences.entry(parameter.0).or_default().push(actual);
                }
                continue;
            }
            if std::mem::discriminant(&formal.kind) != std::mem::discriminant(&actual_row.kind) {
                continue;
            }
            let next = depth + u32::from(matches!(formal.kind, TypeKind::Function(_)));
            pending.extend(
                formal
                    .kind
                    .value_parts()
                    .into_iter()
                    .zip(actual_row.kind.value_parts())
                    .map(|(a, b)| (a, b, next)),
            );
        }
        for (parameter, parts) in occurrences {
            if let Some(GenericArgument::Type(actual)) = arguments.get_mut(parameter as usize) {
                let joined = self.join_types(parts[0], &parts);
                let refined = self.part_brands(*actual, joined);
                let mut part = self.t.get(refined).unwrap().clone();
                part.brand = self.t.get(joined).unwrap().brand.clone();
                *actual = self.intern(part);
            }
        }
        let refined = self.intern(row);
        self.set_ty(e, refined);
    }
    /// A constructor's fields placed in its owner: a record's, or the
    /// selected variant's.
    pub(super) fn constructor_fields(&mut self, e: ExprId, root: TypeId) -> Option<Vec<Field>> {
        self.fields(root).or_else(|| {
            let call = self.call_row(e)?;
            let CallTarget::Constructor(ContextualName::Variant { variant, .. }) = &call.target
            else {
                return None;
            };
            let variant = *variant;
            // A Packed construction builds its payload variant (MEM-13).
            let payload = match &self.t.get(root)?.kind {
                TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    arguments,
                } => match arguments.first()? {
                    GenericArgument::Type(t) => *t,
                    _ => return None,
                },
                _ => root,
            };
            let TypeKind::Variant(variants) = self.layout(payload)? else {
                return None;
            };
            Some(
                variants
                    .get(variant.0 as usize)?
                    .fields
                    .iter()
                    .map(|(name, ty)| Field {
                        name: name.unwrap_or_else(|| Symbol::intern("")),
                        ty: self.placed_item(root, *ty),
                        growable: false,
                    })
                    .collect(),
            )
        })
    }
    pub(super) fn expression_children(&self, e: ExprId) -> Vec<ExprId> {
        hirsel_modes::expression_children(self.tree, e)
    }
    /// A static lexical name with no environment can be reconstructed wherever
    /// it is referenced; it is not a borrowed value of the enclosing frame.
    pub(super) fn capture_free_generic(&self, e: ExprId) -> bool {
        let Some(binding) = self.binding(e) else {
            return false;
        };
        if !matches!(self.t.get(binding.ty).map(|row| &row.kind), Some(TypeKind::Function(f)) if !f.generics.is_empty())
        {
            return false;
        }
        let NameSite::Statement(statement) = binding.site else {
            return false;
        };
        let StmtKind::Function { body, .. } = self.tree.statements[statement.0 as usize].kind
        else {
            return false;
        };
        self.closures
            .get(&body)
            .is_some_and(|closure| closure.captures.is_empty())
    }
    fn free_uses(&self, owner: NameSite, body: ExprId) -> Vec<FreeUse> {
        let mut owners = BTreeSet::from([owner.clone()]);
        let mut stack = vec![body];
        let mut all = Vec::new();
        let mut annotations = Vec::new();
        let signature_types =
            |signature: &hirsel_syntax::FunctionSyntax, first, types: &mut Vec<_>| {
                types.extend(signature.parameters.iter().map(|p| (p.ty, first)));
                types.extend(signature.result.map(|ty| (ty, first)));
            };
        match owner {
            NameSite::Expression(e) => {
                if let ExprKind::Closure { signature, .. } =
                    &self.tree.expressions[e.0 as usize].kind
                {
                    signature_types(signature, e, &mut annotations);
                }
            }
            NameSite::Statement(s) => {
                if let StmtKind::Function { signature, .. } =
                    &self.tree.statements[s.0 as usize].kind
                {
                    signature_types(signature, body, &mut annotations);
                }
            }
            _ => {}
        }
        while let Some(e) = stack.pop() {
            if matches!(
                self.tree.expressions[e.0 as usize].kind,
                ExprKind::Closure { .. }
            ) {
                owners.insert(NameSite::Expression(e));
            }
            if let ExprKind::Closure { signature, .. } = &self.tree.expressions[e.0 as usize].kind {
                signature_types(signature, e, &mut annotations);
            }
            if let ExprKind::Block { statements, .. } = &self.tree.expressions[e.0 as usize].kind {
                for s in statements {
                    match &self.tree.statements[s.0 as usize].kind {
                        StmtKind::Let {
                            ty: Some(ty),
                            value,
                            ..
                        } => annotations.push((*ty, *value)),
                        StmtKind::Function {
                            signature, body, ..
                        } => signature_types(signature, *body, &mut annotations),
                        _ => {}
                    }
                    if matches!(
                        self.tree.statements[s.0 as usize].kind,
                        StmtKind::Function { .. }
                    ) {
                        owners.insert(NameSite::Statement(*s));
                    }
                }
            }
            all.push(e);
            let children = self.expression_children(e);
            stack.extend(children.into_iter().rev());
        }
        let mut uses: Vec<FreeUse> = vec![];
        for e in all {
            let (reference, explicit_take) = match self.tree.expressions[e.0 as usize].kind {
                ExprKind::Unary {
                    op: UnaryOp::Take,
                    value,
                } => (self.transparent_value(value), true),
                _ => (e, false),
            };
            if self.elaborations.get(&reference).is_some_and(|i| {
                !self.out.body.expressions[*i]
                    .instantiated_arguments
                    .is_empty()
            }) && self.capture_free_generic(reference)
            {
                continue;
            }
            let Some(mut b) = self.binding(reference) else {
                continue;
            };
            if owners.contains(&b.owner)
                || (b.site == owner && matches!(b.binding, ResolvedName::Local(_)))
            {
                continue;
            }
            b.ty = self
                .binding_types
                .get(&binding_key(&b))
                .copied()
                .unwrap_or(b.ty);
            // The capture mode is chosen from syntax, not the inner elaborated use.
            if let Some(prior) = uses
                .iter_mut()
                .find(|u| u.binding.owner == b.owner && u.binding.binding == b.binding)
            {
                prior.take |= explicit_take;
            } else {
                uses.push(FreeUse {
                    binding: b,
                    first: reference,
                    take: explicit_take,
                });
            }
        }
        // An arena named only by `in a` still supplies a runtime allocation
        // token. Carry it through the ordinary capture table so ownership,
        // lowering and every engine see the same borrowed environment input.
        let mut seen = BTreeSet::new();
        while let Some((syntax, first)) = annotations.pop() {
            if !seen.insert(syntax) {
                continue;
            }
            match &self.tree.types[syntax.0 as usize].kind {
                hirsel_syntax::TypeExprKind::Named { arguments, .. } => {
                    annotations.extend(arguments.iter().filter_map(|a| match a {
                        hirsel_syntax::TypeArgument::Type(ty) => Some((*ty, first)),
                        _ => None,
                    }));
                }
                hirsel_syntax::TypeExprKind::Tuple(parts) => {
                    annotations.extend(parts.iter().map(|ty| (*ty, first)))
                }
                hirsel_syntax::TypeExprKind::Box(ty) => annotations.push((*ty, first)),
                hirsel_syntax::TypeExprKind::Function(signature) => {
                    signature_types(signature, first, &mut annotations)
                }
                hirsel_syntax::TypeExprKind::Qualified {
                    base,
                    brand,
                    relation,
                    ..
                } => {
                    annotations.push((*base, first));
                    let mut index = usize::from(brand.as_ref().is_some_and(|b| !b.sibling));
                    for place in relation.iter().flat_map(|r| &r.places) {
                        if !matches!(place, hirsel_syntax::RelationPlace::Path(_)) {
                            continue;
                        }
                        let origin = self.relation_binding(&NameSite::Type(syntax), index);
                        index += 1;
                        let Some(mut binding) = origin
                            .as_ref()
                            .and_then(|o| self.origin_binding(o))
                            .cloned()
                        else {
                            continue;
                        };
                        if !self.arena_type(binding.ty)
                            || owners.contains(&binding.owner)
                            || (binding.site == owner
                                && matches!(binding.binding, ResolvedName::Local(_)))
                            || uses.iter().any(|u| {
                                u.binding.owner == binding.owner
                                    && u.binding.binding == binding.binding
                            })
                        {
                            continue;
                        }
                        binding.ty = self
                            .binding_types
                            .get(&binding_key(&binding))
                            .copied()
                            .unwrap_or(binding.ty);
                        uses.push(FreeUse {
                            binding,
                            first,
                            take: false,
                        });
                    }
                }
                hirsel_syntax::TypeExprKind::Error => {}
            }
        }
        uses
    }
    pub(super) fn closure(
        &mut self,
        e: ExprId,
        body: ExprId,
        context: Option<(CaptureContext, Option<TypeId>, RuleId)>,
    ) {
        let actual = self.ty(e).unwrap();
        let original = self.t.get(actual).unwrap().clone();
        let syntax = match self.tree.expressions[e.0 as usize].kind.clone() {
            ExprKind::Closure { signature, .. } => Some(signature),
            _ => None,
        };
        self.function_value(e, body, NameSite::Expression(e), original, syntax, context);
    }
    fn set_function_type(&mut self, key: ExprId, owner: &NameSite, ty: TypeId) {
        if matches!(owner, NameSite::Expression(_)) {
            self.set_ty(key, ty);
        } else if let Some(binding) = self.out.body.bindings.iter_mut().find(|b| {
            work::count(Counter::AuthoritySourceRows);
            &b.site == owner && matches!(b.binding, ResolvedName::Local(_))
        }) {
            binding.ty = ty;
            self.binding_types.insert(binding_key(binding), ty);
        }
    }
    pub(super) fn function_value(
        &mut self,
        e: ExprId,
        body: ExprId,
        owner: NameSite,
        original: Type,
        syntax: Option<hirsel_syntax::FunctionSyntax>,
        context: Option<(CaptureContext, Option<TypeId>, RuleId)>,
    ) {
        let TypeKind::Function(mut f) = original.kind.clone() else {
            self.out.failure.get_or_insert(QueryError::NotImplemented);
            return;
        };
        let inferred_result = context
            .as_ref()
            .is_none_or(|(_, expected, _)| expected.is_none())
            && syntax.as_ref().is_some_and(|s| s.result.is_none());
        let scopes: Vec<_> = std::iter::once(owner)
            .chain(self.frames.iter().rev().map(|f| f.owner))
            .map(|site| self.lexical.site(site))
            .collect();
        for p in &mut f.parameters {
            p.ty = bind_parameter_scopes(self.t, p.ty, &scopes)
                .expect("checked lexical parameter scope");
        }
        if !inferred_result {
            f.result = bind_parameter_scopes(self.t, f.result, &scopes)
                .expect("checked lexical parameter scope");
        }
        if let Some(signature) = &syntax {
            for (parameter, syntax) in f.parameters.iter_mut().zip(&signature.parameters) {
                parameter.ty = self.annotation(parameter.ty, syntax.ty);
                parameter.ty = declared_receipt_evidence(self.t, parameter.ty)
                    .expect("declared closure entry");
            }
            // H may erase dependent provenance on a nested binding while the
            // function header retains it. Apply the same written annotation to
            // that binding, preserving its elaborated structure and modes.
            let parameters: Vec<_> = self
                .source_rows
                .parameters
                .get(&owner)
                .into_iter()
                .flatten()
                .copied()
                .filter_map(|index| {
                    work::count(Counter::AuthoritySourceRows);
                    let binding = &self.out.body.bindings[index];
                    let ResolvedName::Parameter(parameter) = binding.binding else {
                        return None;
                    };
                    let syntax = signature.parameters.get(parameter.0 as usize)?;
                    Some((index, binding.ty, syntax.ty))
                })
                .collect();
            for (index, ty, syntax) in parameters {
                let annotated = self.annotation(ty, syntax);
                let annotated = declared_receipt_evidence(self.t, annotated)
                    .expect("declared closure parameter");
                let binding = &mut self.out.body.bindings[index];
                binding.ty = annotated;
                self.binding_types.insert(binding_key(binding), annotated);
            }
            if let Some(result) = signature.result {
                f.result = self.annotation(f.result, result);
            }
            if let Some(uses) = &signature.uses {
                f.uses = self.syntax_uses(owner.clone(), uses);
            }
        }
        let (ctx, expected, restriction) =
            context.unwrap_or((CaptureContext::Scoped, None, const { RuleId::of("CAP-3") }));
        let expected_type = expected.and_then(|t| self.t.get(t)).cloned();
        let expected_function = expected_type.as_ref().and_then(|t| match &t.kind {
            TypeKind::Function(f) => Some(f.clone()),
            _ => None,
        });
        let capture_limit = expected_function
            .as_ref()
            .map(|f| f.uses.clone())
            .unwrap_or_else(|| f.uses.clone());
        let receipts = Frame::parameter_receipts(&capture_limit, self.lexical.site(owner));
        // Own parameters remain substitution selectors. Outer parameters are
        // captured values: publish their actual authority, as the body does.
        if let Uses::Only(summary) = &capture_limit {
            let published = match summary {
                AuthoritySummary::Known(terms) => {
                    terms
                        .iter()
                        .cloned()
                        .fold(AuthoritySummary::inert(), |authority, term| {
                            let own = matches!(term, AuthorityTerm::Parameter(
                            ParameterRef::Body { scope, .. }
                        ) if scope == self.lexical.site(owner));
                            let selected = AuthoritySummary::Known(vec![term].into());
                            authority.join(if own {
                                selected
                            } else {
                                self.scoped_authority(selected)
                            })
                        })
                }
                AuthoritySummary::Unknown => AuthoritySummary::Unknown,
            };
            f.uses = Uses::Only(published);
        }
        let capture_limit = match capture_limit {
            Uses::Only(a) => Uses::Only(self.scoped_authority(a)),
            other => other,
        };
        let free = self.free_uses(owner.clone(), body);
        let mut captures = Vec::new();
        let mut authority = AuthoritySummary::inert();
        let mut modes = original.modes.clone();
        modes.relation = if matches!(ctx, CaptureContext::Scoped | CaptureContext::OwningScoped) {
            Relation::environment()
        } else {
            Relation::global()
        };
        modes.multiplicity = if ctx == CaptureContext::Task {
            Multiplicity::Once
        } else {
            Multiplicity::Many
        };
        let mut sendable = matches!(ctx, CaptureContext::Escaping | CaptureContext::Task)
            || expected_type
                .as_ref()
                .is_some_and(|t| crossing_requirement(t) == Sendability::Sendable);
        // Borrowed captures as the body sees them, with their source rows.
        let mut views = Vec::new();
        for free_use in free {
            let mut b = free_use.binding;
            b.ty = self
                .binding_types
                .get(&binding_key(&b))
                .copied()
                .unwrap_or(b.ty);
            let usage = if matches!(ctx, CaptureContext::Scoped) {
                UseKind::Borrow
            } else if free_use.take {
                UseKind::Take
            } else if ctx == CaptureContext::OwningScoped {
                UseKind::Borrow
            } else {
                UseKind::Copy
            };
            let mut capture_type = self.t.get(b.ty).unwrap().clone();
            let plain = self.property(b.ty, Bound::Plain);
            let flat = self.property(b.ty, Bound::Flat);
            // The environment's locality and uniqueness are the ownership
            // pass's, from current content; these rows feed sendability.
            // A copied Shared root is an independently counted handle (MEM-11);
            // a copied Mutex or Atomic names its run-owned table entry (MODE-4),
            // also inside a record, variant or tuple of handles and plain bits.
            if usage == UseKind::Copy && (plain || flat || self.copy_detaches(b.ty)) {
                capture_type.modes.relation = self.placement(&capture_type.modes.relation);
            }
            if usage == UseKind::Copy && !plain && !flat {
                capture_type.modes.uniqueness = Uniqueness::Aliased;
                capture_type.sendability_requirement = SendabilityRequirement::None;
                let root = self.intern(capture_type.clone());
                capture_type.modes.sendability = self.proof(Sendability::NotSendable, |memo| {
                    hirsel_modes::proofs::sendability_after_sharing(memo, root)
                });
            }
            // CLO-1/OWN-1: a closure body knows a borrowed capture exactly as a
            // function body knows a borrowed parameter of the same type. It is
            // local, and anything non-plain derived from it is local to the
            // environment; placement and brands stay those of the source.
            // The source row is a receiving contract. When a source's current
            // content is local (a finalizer's value, a record holding local
            // content), the ownership pass makes the result local again.
            let view = matches!(usage, UseKind::Borrow | UseKind::Mut) && !plain;
            if view {
                capture_type.modes.relation =
                    Relation::environment().union(&self.placement(&capture_type.modes.relation));
            }
            let capture_type_id = self.intern(capture_type.clone());
            if view {
                views.push((binding_key(&b), capture_type_id, b.ty));
            }
            let a = self.authority(b.ty);
            authority = authority.join(a.clone());
            if let Uses::Only(allowed) = &capture_limit {
                if !subset(&a, allowed) {
                    let rule = if matches!(ctx, CaptureContext::Escaping | CaptureContext::Task) {
                        const { RuleId::of("CLO-4") }
                    } else {
                        restriction.clone()
                    };
                    let provided = if rule.as_str() == "CONC-5" {
                        let (name, site) = self.sites().binding(&b);
                        Fact::new(Cause::AuthorityCapture, site, &[&name])
                    } else {
                        Fact::new(Cause::Authority, self.span(free_use.first), &[])
                    };
                    let seal = self.capability_definition(b.ty);
                    let message = seal.as_ref().map_or_else(
                        || "captured authority exceeds the callback's declared uses".to_owned(),
                        |path| {
                            let limit = match allowed {
                                AuthoritySummary::Known(terms) => format!(
                                    "{{{}}}",
                                    terms
                                        .iter()
                                        .map(|term| match term {
                                            AuthorityTerm::Capability(path) => path.0.name.clone(),
                                            _ => "parameter authority".into(),
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                AuthoritySummary::Unknown => "unknown".into(),
                            };
                            format!("captures {}; allowed {limit}", path.0.name)
                        },
                    );
                    let before = self.out.diagnostics.len();
                    self.explained(
                        rule,
                        self.span(free_use.first),
                        &message,
                        Explanation::default()
                            .provided(provided)
                            .required(Fact::new(Requirement::Uses, self.span(e), &[])),
                    );
                    if self.out.diagnostics.len() > before {
                        if let Some(path) = seal {
                            let declaration = self.reader.borrow_mut().provider().cite_name(&path);
                            let diagnostic = self.out.diagnostics.last_mut().unwrap();
                            diagnostic
                                .notes
                                .push(format!("capability declaration: {path}"));
                            if let Some(span) = declaration {
                                diagnostic.secondary.push(Label {
                                    span,
                                    message: "declared capability".into(),
                                });
                            }
                        }
                    }
                }
            }
            if self.scoped(b.ty) && matches!(ctx, CaptureContext::Escaping | CaptureContext::Task) {
                self.explained(
                    const { RuleId::of("MODE-6") },
                    self.span(e),
                    "a scoped capability cannot enter an escaping capture",
                    Explanation::default()
                        .provided(Fact::new(
                            Cause::ScopedCapability,
                            self.span(free_use.first),
                            &[],
                        ))
                        .required(Fact::new(Requirement::EscapingCapture, self.span(e), &[])),
                );
            } else if ctx == CaptureContext::Task {
                let receiver = matches!(
                    capture_type.kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Receiver,
                        ..
                    }
                );
                if b.mutable
                    || matches!(usage, UseKind::Borrow | UseKind::Mut)
                    || (receiver && usage != UseKind::Take)
                    // A taken capture's current uniqueness is I's (CLO-3).
                    || (!receiver
                        && !if usage == UseKind::Take {
                            self.sendable_when_unique(capture_type_id)
                        } else {
                            self.sendable(capture_type_id)
                        })
                    // The Receiver exception waives sendability, not
                    // locality: a local value cannot cross into a task.
                    || (receiver && !self.lifetime(&capture_type.modes.relation).is_global())
                {
                    let (name, site) = self.sites().binding(&b);
                    let cause = if b.mutable {
                        Cause::MutableCapture
                    } else if matches!(usage, UseKind::Borrow | UseKind::Mut) {
                        Cause::BorrowedCapture
                    } else if receiver {
                        Cause::ReceiverCapture
                    } else {
                        Cause::NotSendable
                    };
                    self.explained(
                        const { RuleId::of("CLO-3") },
                        self.span(e),
                        "task captures must copy or take sendable values; Receivers must be taken",
                        Explanation::default()
                            .provided(Fact::new(cause, site, &[&name]))
                            .required(Fact::new(Requirement::TaskCapture, self.span(e), &[])),
                    );
                }
            }
            if matches!(ctx, CaptureContext::Escaping | CaptureContext::Task)
                && self.retains_scoped_placement(capture_type_id)
            {
                self.explained(
                    const { RuleId::of("PROV-4") },
                    self.span(e),
                    "a closure cannot outlive the storage of its captured value",
                    Explanation::default()
                        .provided(Fact::new(
                            Cause::ScopedPlacement,
                            self.span(free_use.first),
                            &[],
                        ))
                        .required(Fact::new(Requirement::EscapingCapture, self.span(e), &[])),
                );
            }
            if usage == UseKind::Take || capture_type.modes.multiplicity == Multiplicity::Once {
                modes.multiplicity = Multiplicity::Once;
            }
            let captured_sendable = if usage == UseKind::Copy && !plain && !flat {
                // Use the post-share proof just derived above. Re-entering
                // structural proof would consume stale descendant summaries.
                capture_type.modes.sendability == Sendability::Sendable
            } else {
                self.sendable(capture_type_id)
            };
            if !captured_sendable {
                sendable = false;
            }
            captures.push(CapturedName {
                owner: b.owner,
                binding: b.binding,
                usage,
                ty: capture_type_id,
            });
        }
        modes.sendability = if sendable {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        };
        // Spec #58: inferred literal values carry capture-derived uses evidence.
        // An explicit header or receiving function type retains its contract.
        if expected.is_none() && matches!(f.uses, Uses::Unrestricted) {
            f.uses = Uses::Only(authority.clone());
        }
        // Capture authority is distinct from authority passed to the closure itself.
        f.authority = authority.clone();
        let mut ty = original.clone();
        ty.kind = TypeKind::Function(f.clone());
        ty.modes = modes;
        // MEM-5: an erased closure's cleanup summary is fixed where it is
        // built, by the values its environment owns; a borrowed capture
        // destroys nothing. Only an unannotated binding's own literal proves
        // it: a receiving type, written or a constructor's, keeps the
        // summary it states, as it keeps its `uses`.
        if expected_function.is_none()
            && matches!(owner, NameSite::Expression(_))
            && self.tree.statements.iter().any(|statement| {
                matches!(statement.kind,
                    StmtKind::Let { ty: None, value, .. } if value == e)
            })
        {
            ty.drop_allocation = captures
                .iter()
                .filter(|capture| matches!(capture.usage, UseKind::Copy | UseKind::Take))
                .fold(DropAllocation::NoAlloc, |summary, capture| {
                    summary.join(self.t.get(capture.ty).unwrap().drop_allocation.clone())
                });
        }
        let refined = self.intern(ty);
        let refined = abstract_parameter_scope(self.t, refined, self.lexical.site(owner))
            .expect("checked callable binder");
        self.set_function_type(e, &owner, refined);

        self.closures.insert(
            e,
            ClosureInfo {
                context: ctx,
                result_evidence: None,
                expression: e,
                captures: captures
                    .iter()
                    .map(|c| Capture {
                        binding: c.binding.clone(),
                        usage: c.usage,
                        ty: c.ty,
                    })
                    .collect(),
                ty: refined,
            },
        );

        self.captures.insert(
            e,
            CaptureTable {
                expression: e,
                context: ctx,
                captures: captures.clone(),
                authority: authority.clone(),
            },
        );
        // A lexical instantiation has an existing checked static body. Its
        // generated forwarding call needs capture creation and a formal result,
        // but must not walk the name again as a source body.
        if matches!(self.tree.expressions[e.0 as usize].kind, ExprKind::Path(_)) {
            self.forwarding_closure(e);
            self.facts.insert(e, authority);
            return;
        }
        let before_types = self.binding_types.clone();
        let before_views = self.capture_views.clone();
        for (key, view, _) in &views {
            self.binding_types.insert(key.clone(), *view);
            self.capture_views.insert(key.clone());
        }
        let before_invalid = self.invalid.clone();
        let before_pending = std::mem::take(&mut self.pending_tasks);
        let before_task_bindings = self.task_bindings.clone();
        let before_loop_exits = std::mem::take(&mut self.loop_task_exits);
        self.frames.push(Frame {
            owner: owner.clone(),
            allowed: Uses::Unrestricted,
            receipts,
            result: (!inferred_result).then_some(f.result),
            restriction,
        });

        let mut allowed = match capture_limit {
            Uses::Only(a) => Some(a),
            Uses::Unrestricted => None,
        };
        // A restricted callback may use the capabilities it receives as parameters.
        if let Some(a) = &mut allowed {
            for p in &f.parameters {
                *a = a.clone().join(self.authority(p.ty));
            }
        }
        self.frames.last_mut().unwrap().allowed = if self.in_clause {
            Uses::Only(AuthoritySummary::inert())
        } else {
            allowed.map(Uses::Only).unwrap_or(Uses::Unrestricted)
        };
        let temporaries = self.temporaries.len();
        self.expr(
            body,
            Some((
                CaptureContext::Escaping,
                (!inferred_result).then_some(f.result),
                const { RuleId::of("CLO-4") },
            )),
        );
        self.frame_temporaries(body, temporaries);
        self.return_value(body, self.result_span(body));
        let result_evidence = self.ty(body);
        if let Some(mut closure) = self.closures.get_mut(&e) {
            closure.result_evidence = result_evidence;
        }
        let mut frame_result = f.result;
        let infers_result = expected_function.is_none()
            && syntax
                .as_ref()
                .is_some_and(|signature| signature.result.is_none());
        // MEM-5: like its `uses` (#58), an inferred literal's `@noalloc` is
        // evidence read off its body. A receiving function type or a written
        // header is a contract and keeps the allocation it states; contracts
        // are code of their own, which this proof does not read.
        let proves_noalloc = expected_function.is_none()
            && matches!(owner, NameSite::Expression(_))
            && syntax
                .as_ref()
                .is_some_and(|signature| signature.contracts.is_empty())
            && f.allocation == Allocation::MayAllocate
            && self.noalloc_evidence(&owner, body);
        if proves_noalloc {
            f.allocation = Allocation::NoAlloc;
        }
        if infers_result || proves_noalloc {
            if let Some(mut result) = self.ty(body).filter(|_| infers_result) {
                // CLO-1: an inferred result is not a uniqueness promise. A
                // borrowed capture read in the body shares its value (MODE-2),
                // whatever the captured binding's row says, so the frame
                // result and the callable's result claim no uniqueness.
                let mut row = self.t.get(result).unwrap().clone();
                // A literal's inferred result describes a returned value.
                // Its root operand qualifier does not become a written result contract.
                row.sendability_requirement = SendabilityRequirement::None;
                if row.modes.uniqueness == Uniqueness::Unique
                    && !self.property(result, Bound::Plain)
                {
                    row.modes.uniqueness = Uniqueness::Aliased;
                }
                result = self.intern(row);
                let returned = self.transparent_value(body);
                let place = self.source_place(returned);
                let source_binding = match &place.root {
                    SourceRoot::Binding { owner, binding } => {
                        self.root_binding(*owner, binding).cloned()
                    }
                    _ => self.binding(returned),
                };
                let borrowed_capture = source_binding
                    .as_ref()
                    .is_some_and(|binding| self.capture_views.contains(&binding_key(binding)));
                let owning = matches!(place.access, SourceAccess::Take | SourceAccess::Unique)
                    || matches!(place.root, SourceRoot::Value(_)) && place.projections.is_empty()
                    || !borrowed_capture
                        && place.projections.is_empty()
                        && source_binding.is_some_and(|binding| binding.usage == UseKind::Take);
                if place.access == SourceAccess::Copy || !owning {
                    let mut row = self.t.get(result).unwrap().clone();
                    row.sendability_requirement = SendabilityRequirement::None;
                    let root = self.intern(row.clone());
                    row.modes.sendability = self.proof(Sendability::NotSendable, |memo| {
                        hirsel_modes::proofs::sendability_after_sharing(memo, root)
                    });
                    result = self.intern(row);
                    if let Some(mut closure) = self.closures.get_mut(&e) {
                        closure.result_evidence = Some(result);
                    }
                }
                frame_result = result;
                f.result = self.environment_result(result, &f, &captures, &views);
            }
            f.error = result_error(self.t, f.result, &self.signature.metadata.roles);
            let mut row = self.t.get(refined).unwrap().clone();
            row.kind = TypeKind::Function(f.clone());
            let updated = self.intern(row);
            let updated = abstract_parameter_scope(self.t, updated, self.lexical.site(owner))
                .expect("checked inferred callable binder");
            self.set_function_type(e, &owner, updated);
            if let Some(mut c) = self.closures.get_mut(&e) {
                c.ty = updated;
            }
        }
        let owner_site = self.lexical.site(owner);
        let mut owned_brand = false;
        let mut stack = vec![f.result];
        let mut seen = HashSet::default();
        while let Some(t) = stack.pop() {
            if !seen.insert(t) {
                continue;
            }
            let ty = self.t.get(t).unwrap();
            owned_brand |= ty
                .brand
                .alternatives
                .iter()
                .any(|o| matches!(o.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == owner_site))
                || ty.modes.relation.reaches().iter().any(|r| matches!(r, Reach::Place(o) if self.arena_place(o) && matches!(o.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == owner_site)));
            stack.extend(ty.kind.value_parts());
        }
        if owned_brand {
            self.explained(
                const { RuleId::of("PROV-2") },
                self.span(e),
                "a closure result cannot be branded or placed in its own parameters",
                Explanation::default()
                    .provided(Fact::new(Cause::ClosureOwn, self.span(e), &[]))
                    .required(Fact::new(Requirement::NoOwnPlacement, self.span(e), &[])),
            );
        }
        self.check_pending_tasks();
        self.out.result_types.retain(|(site, _)| *site != owner);
        self.out.result_types.push((owner.clone(), frame_result));
        self.pending_tasks = before_pending;
        self.task_bindings = before_task_bindings;
        self.loop_task_exits = before_loop_exits;
        self.frames.pop();
        self.binding_types = before_types;
        self.capture_views = before_views;
        self.invalid = before_invalid;
        self.facts.insert(e, authority);
    }
}

impl Checker<'_> {
    /// An invalid call reports its own error, so the CONC-2 obligations its arguments carry
    /// are not reported again (section 23).
    fn abandon_tasks(&mut self, arguments: &[Argument]) {
        for argument in arguments {
            for task in self.task_handles(argument.value) {
                self.pending_tasks.remove(&task);
            }
        }
    }
}

/// CONC-2: a task handle or task set, each of which must be awaited or detached.
pub(super) fn task_obligation(types: &TypeInterner, ty: TypeId) -> bool {
    match types.get(ty).map(|t| &t.kind) {
        Some(TypeKind::Builtin {
            kind: BuiltinType::Task,
            ..
        }) => true,
        Some(TypeKind::Named { definition, .. }) => {
            *definition.path() == hirsel_syntax::DefPath::core("TaskSet")
        }
        _ => false,
    }
}
