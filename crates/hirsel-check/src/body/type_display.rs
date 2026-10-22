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
