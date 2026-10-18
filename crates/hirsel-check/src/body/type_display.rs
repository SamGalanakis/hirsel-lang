//! Source-shaped type names for shape diagnostics. This never queries a body.
use super::*;

impl Checker<'_, '_> {
    pub(super) fn type_spelling(&self, root: TypeId) -> String {
        hirsel_types::type_spelling(
            &self.body.types,
            root,
            &self.body.resolution.generic_declarations,
            &self.input.signature.definition.0.package,
            false,
        )
    }
    pub(super) fn type_mismatch(
        &mut self,
        rule: RuleId,
        span: Span,
        message: &str,
        actual: TypeId,
        expected: TypeId,
    ) {
        self.report(
            rule,
            span,
            &format!(
                "{message}: expected `{}`, found `{}`",
                self.type_spelling(expected),
                self.type_spelling(actual)
            ),
        );
    }
}
