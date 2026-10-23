//! Source-shaped type names for shape diagnostics. This never queries a body.
use super::*;

impl Checker<'_, '_> {
    pub(super) fn type_spelling(&self, root: TypeId) -> String {
        hirsel_type_analysis::spelling::type_spelling(
            self.types,
            root,
            &self.body.resolution.generic_declarations,
            &self.input.signature.definition.path().0.package,
            false,
        )
    }
    /// A partial Result expectation is owned by the `?` that introduced its
    /// fresh error binder. Recognize only its actual operand, and only when
    /// rewrapping the known payload fits the required registered Result.
    pub(super) fn result_tail_context(
        &mut self,
        operand: ast::ExprId,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<Option<(ast::ExprId, TypeId)>, QueryError> {
        let Some((_, required)) = self.partial_result(expected) else {
            return Ok(None);
        };
        let Some((family, wanted, _)) = self.result_parts(required)? else {
            return Ok(None);
        };
        let Some((actual_family, payload, _)) = self.result_parts(actual)? else {
            return Ok(None);
        };
        if family != actual_family || !self.types.shape_eq(payload, wanted).unwrap_or(false) {
            return Ok(None);
        }
        let TypeKind::Named { arguments, .. } = self.kind(expected) else {
            return Ok(None);
        };
        let [_, GenericArgument::Type(error)] = arguments.as_slice() else {
            return Ok(None);
        };
        let TypeKind::Parameter(TypeParameterRef::Body { scope, .. }) = self.kind(*error) else {
            return Ok(None);
        };
        if scope.owner == self.lexical
            && let NameSite::Expression(propagation) = scope.site
            && matches!(self.input.syntax.tree.expressions[propagation.0 as usize].kind,
                ast::ExprKind::Try { value, .. } if value == operand)
        {
            Ok(Some((propagation, required)))
        } else {
            Ok(None)
        }
    }
    pub(super) fn shared_payload(&self, ty: TypeId) -> Option<TypeId> {
        match self.kind(ty) {
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(payload)] => Some(*payload),
                _ => None,
            },
            _ => None,
        }
    }
    pub(super) fn shared_access_error(
        &mut self,
        rule: RuleId,
        span: Span,
        message: &str,
        payload: Option<TypeId>,
    ) {
        let mut diagnostic = Diagnostic::error(rule, span, message);
        if let Some(payload) = payload {
            diagnostic = diagnostic.with_shared_read_help(&self.type_spelling(payload));
        }
        self.diagnostics.push(diagnostic);
    }
    pub(super) fn type_mismatch(
        &mut self,
        rule: RuleId,
        span: Span,
        message: &str,
        actual: TypeId,
        expected: TypeId,
    ) {
        // Binary RHS checking can reject before `binary` sees both rows. The
        // expected site's whole-expression span identifies its operator, while
        // the checked left row and actual RHS row establish operand identity.
        // This error-only lookup also covers a block's propagated tail context.
        let addition_left = self
            .input
            .syntax
            .tree
            .expressions
            .iter()
            .find_map(|expression| match expression.kind {
                ast::ExprKind::Binary {
                    op: ast::BinaryOp::Add,
                    left,
                    ..
                } if expression.origin.span == span => self.values.get(&left).copied(),
                _ => None,
            });
        if let Some(left) = addition_left
            && self.string_addition_error(rule.clone(), span, left, actual)
        {
            return;
        }
        let payload = self
            .shared_payload(actual)
            .filter(|payload| shape_equal(self.types, *payload, expected).unwrap_or(false));
        let [expected, actual] = hirsel_type_analysis::spelling::type_spellings_apart(
            self.types,
            [expected, actual],
            &self.body.resolution.generic_declarations,
            &self.input.signature.definition.path().0.package,
            false,
        );
        self.shared_access_error(
            rule,
            span,
            &format!("{message}: expected `{expected}`, found `{actual}`"),
            payload,
        );
    }
}
