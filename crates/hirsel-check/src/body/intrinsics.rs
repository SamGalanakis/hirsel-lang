use super::*;
impl Checker<'_, '_> {
    pub(super) fn forget_call(
        &mut self,
        id: ast::ExprId,
        target: &CallTarget,
        arguments: &[GenericArgument],
    ) -> Result<bool, QueryError> {
        let CallTarget::Definition(path) = target else {
            return Ok(true);
        };
        if !path.package_is("core", "1") || !path.0.modules.is_empty() || path.0.name != "forget" {
            return Ok(true);
        }
        let resource = match arguments.first() {
            Some(GenericArgument::Type(ty)) => match self.kind(*ty) {
                TypeKind::Named { definition, .. } => Some(definition),
                _ => None,
            },
            _ => None,
        };
        let valid = if let Some(definition) = resource {
            let declaration = self.snapshot(&definition)?;
            matches!(&declaration.kind, SignatureKind::Type(td) if td.resource)
                && definition.0.package == self.input.signature.definition.0.package
                && definition.0.modules == self.input.signature.definition.0.modules
        } else {
            false
        };
        if !valid {
            self.report(
                const { RuleId::of("DROP-4") },
                self.span(id),
                "forget accepts a resource only in its defining module",
            );
        }
        Ok(valid)
    }
    /// Intrinsic restrictions are attached to the registry's canonical definition
    /// (#36). Same-spelled user functions have only their declared signatures.
    pub(super) fn intrinsic_call(
        &mut self,
        id: ast::ExprId,
        target: &CallTarget,
        args: &[GenericArgument],
    ) -> Result<bool, QueryError> {
        let CallTarget::Definition(path) = target else {
            return Ok(true);
        };
        let Some(signature) = self.snapshots.get(path) else {
            return Ok(true);
        };
        let schema = signature.metadata.schema.clone();
        self.schema_arguments(self.span(id), &schema, args)
    }
    pub(super) fn schema_arguments(
        &mut self,
        span: hirsel_span::Span,
        schema: &[SchemaConstraint],
        args: &[GenericArgument],
    ) -> Result<bool, QueryError> {
        let mut proofs = BTreeMap::new();
        for constraint in schema {
            if let SchemaConstraint::Bound {
                parameter, bound, ..
            } = constraint
            {
                if let Some(GenericArgument::Type(ty)) = (parameter.binder()
                    == GenericBinder::Bound(0))
                .then(|| args.get(parameter.parameter().0 as usize))
                .flatten()
                {
                    proofs.insert((*ty, *bound), self.bound(*ty, *bound)?);
                }
            }
        }
        if let Some((rule, message)) =
            schema_failure(&self.body.types, args, schema, |ty, bound| {
                proofs.get(&(ty, bound)).copied().unwrap_or(false)
            })
        {
            self.report(rule, span, message);
            return Ok(false);
        }
        Ok(true)
    }
}

impl Checker<'_, '_> {
    pub(super) fn table_key_arguments(
        &mut self,
        id: ast::ExprId,
        path: &ast::DefPath,
        arguments: &[GenericArgument],
    ) -> Result<bool, QueryError> {
        for &position in core_roles::table_key_parameters(path) {
            let Some(GenericArgument::Type(actual)) = arguments.get(position) else {
                continue;
            };
            let declarations = self.body.resolution.generic_declarations.clone();
            let schema = serializable_schema(
                &self.body.types.clone(),
                *actual,
                &declarations,
                &mut |path| self.snapshot(path).map(Some),
            )?;
            if schema.is_some_and(|schema| {
                schema.nodes.iter().any(|node| {
                    matches!(
                        node,
                        serialization::SchemaNode::Builtin {
                            kind: BuiltinType::Map | BuiltinType::Set | BuiltinType::Simd,
                            ..
                        }
                    )
                })
            }) {
                self.report(
                    const { RuleId::of("STORE-6") },
                    self.span(id),
                    "table key schemas cannot contain Map, Set or Simd",
                );
                return Ok(false);
            }
        }
        Ok(true)
    }
}
