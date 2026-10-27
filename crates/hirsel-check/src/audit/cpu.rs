use super::*;

impl Audit<'_, '_> {
    pub(super) fn cpu_callbacks(&mut self) -> Result<(), QueryError> {
        let calls = self.input.checked.calls.clone();
        for (expression, name) in self.input.checked.resolution.expressions.clone() {
            if !matches!(name, ResolvedName::Definition(ref path) if path.is_core("cpu.map_scoped"))
            {
                continue;
            }
            let span = self.input.syntax.tree.expressions[expression.0 as usize]
                .origin
                .span;
            let direct = calls.iter().any(|call| {
                if !matches!(&call.target, CallTarget::Definition(path) if path.is_core("cpu.map_scoped")) {
                    return false;
                }
                let ast::ExprKind::Call { callee, .. } = self.input.syntax.tree.expressions[call.expression.0 as usize].kind else { return false; };
                let at = self.input.syntax.tree.expressions[callee.0 as usize].origin.span;
                at.start <= span.start && at.end >= span.end
            });
            if !direct {
                self.report(
                    const { RuleId::of("CAP-3") },
                    span,
                    "cpu.map_scoped requires a direct call so its closed callbacks can be proved",
                );
            }
        }
        for call in calls {
            if !matches!(&call.target, CallTarget::Definition(path) if path.is_core("cpu.map_scoped") || (path.0.package.package_is("std", "1") && path.0.modules == ["cpu"] && path.0.name == "map_scoped"))
            {
                continue;
            }
            for argument in &call.arguments {
                if !argument
                    .parameter
                    .as_ref()
                    .is_some_and(|(ordinal, _)| matches!(ordinal, 2 | 3))
                {
                    continue;
                }
                let expression = argument.expression;
                let ty = self.expression_type(expression).unwrap();
                let closed = matches!(&self.types.row(ty).kind,
                    TypeKind::Function(f) if f.foreign.is_none()
                        && matches!(&f.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty()));
                let mut values_closed = closed;
                if let TypeKind::Function(f) = self.types.row(ty).kind.clone() {
                    for value in f.parameters.iter().map(|p| p.ty).chain([f.result]) {
                        values_closed &= self.cpu_value(value)?;
                    }
                }
                if !values_closed {
                    self.report(
                        const { RuleId::of("CAP-3") },
                        self.input.syntax.tree.expressions[expression.0 as usize]
                            .origin
                            .span,
                        "a CPU callback must be closed, authority-free and have uses {}",
                    );
                }
                let root = self
                    .source_place(expression)
                    .and_then(|place| match place.root {
                        SourceRoot::Value(root) => Some(root),
                        _ => None,
                    })
                    .unwrap_or(expression);
                let closures = self.input.checked.closures.clone();
                if !closures.iter().any(|c| c.expression == root) {
                    self.report(const { RuleId::of("CAP-3") },
                        self.input.syntax.tree.expressions[expression.0 as usize].origin.span,
                        "CPU callbacks require a locally visible closure so its closed calls can be proved");
                }
                for closure in closures.iter().filter(|closure| closure.expression == root) {
                    for capture in &closure.captures {
                        if !self.cpu_value(capture.ty)? {
                            self.report(const { RuleId::of("CAP-3") }, self.input.syntax.tree.expressions[root.0 as usize].origin.span,
                                "CPU callbacks cannot capture authority, synchronization cells or foreign handles");
                        }
                    }
                    let span = self.input.syntax.tree.expressions[root.0 as usize]
                        .origin
                        .span;
                    for (expression, name) in self.input.checked.resolution.expressions.clone() {
                        let at = self.input.syntax.tree.expressions[expression.0 as usize]
                            .origin
                            .span;
                        if at.start < span.start || at.end > span.end {
                            continue;
                        }
                        if let ResolvedName::Definition(path) = name
                            && !path.0.package.package_is("core", "1")
                            && matches!(
                                &self.snapshot(&(&path).into())?.kind,
                                SignatureKind::Function(_)
                            )
                        {
                            self.report(
                                const { RuleId::of("CAP-3") },
                                at,
                                "CPU callbacks cannot hide an external helper in a function value",
                            );
                        }
                    }
                    for nested in &self.input.checked.calls {
                        let at = self.input.syntax.tree.expressions[nested.expression.0 as usize]
                            .origin
                            .span;
                        if at.start < span.start || at.end > span.end {
                            continue;
                        }
                        if matches!(&nested.target, CallTarget::Constructor(_))
                            && let Some(ty) = self.expression_type(nested.expression)
                            && !self.cpu_value(ty)?
                        {
                            self.report(
                                const { RuleId::of("CAP-3") },
                                at,
                                "CPU callbacks cannot construct resources with unproved finalizers",
                            );
                        }
                        if let CallTarget::Definition(path) = &nested.target
                            && !path.0.package.package_is("core", "1")
                        {
                            self.report(const { RuleId::of("CAP-3") }, at,
                                "CPU callbacks require locally visible calls; inline this helper in the callback");
                        }
                        if let CallTarget::Value(callee) = nested.target {
                            let allowed = self.expression_type(callee).is_some_and(|ty| {
                                matches!(&self.types.row(ty).kind, TypeKind::Function(f)
                                    if f.foreign.is_none() && matches!(&f.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty()))
                            });
                            if !allowed {
                                self.report(const { RuleId::of("CAP-3") }, at,
                                    "an unresolved indirect CPU call requires an empty uses {} function type");
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn cpu_value(&mut self, ty: TypeId) -> Result<bool, QueryError> {
        let generics = GenericDeclarations::signature(self.input.signature.generics());
        let roles = self.input.signature.metadata.roles.clone();
        let types = self.types;
        let authority = structural_authority(types, ty, &roles, &generics, &mut |_, path| {
            path.map(|path| self.snapshot(&path)).transpose()
        })?;
        if !matches!(authority, AuthoritySummary::Known(terms) if terms.is_empty()) {
            return Ok(false);
        }
        let mut pending = vec![ty];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match types.row(ty).kind.clone() {
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let signature = self.snapshot(&definition)?;
                    if matches!(&signature.kind, SignatureKind::Type(t) if t.resource || t.capability.is_some())
                    {
                        return Ok(false);
                    }
                    let (representation, _) = self.representation(ty)?;
                    if representation != ty {
                        pending.push(representation);
                    }
                    pending.extend(arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                }
                TypeKind::Builtin { kind, arguments } => {
                    if matches!(
                        kind,
                        BuiltinType::Task
                            | BuiltinType::Atomic
                            | BuiltinType::Mutex
                            | BuiltinType::Sender
                            | BuiltinType::Receiver
                            | BuiltinType::Ptr
                            | BuiltinType::FnPtr
                    ) {
                        return Ok(false);
                    }
                    pending.extend(arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                }
                TypeKind::Function(_) => return Ok(false),
                TypeKind::Tuple(parts) => pending.extend(parts.iter().copied()),
                TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
                TypeKind::Variant(variants) => pending.extend(
                    variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|(_, t)| *t)),
                ),
                TypeKind::Parameter(_) => return Ok(false),
                TypeKind::Box(t) => pending.push(t),
                _ => (),
            }
        }
        Ok(true)
    }
}
