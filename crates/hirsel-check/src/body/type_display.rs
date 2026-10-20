//! Source-shaped type names for shape diagnostics. This never queries a body.
use super::*;

impl Checker<'_, '_> {
    pub(super) fn type_spelling(&self, root: TypeId) -> String {
        hirsel_type_analysis::spelling::type_spelling(
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
        let [expected, actual] = hirsel_type_analysis::spelling::type_spellings_apart(
            &self.body.types,
            [expected, actual],
            &self.body.resolution.generic_declarations,
            &self.input.signature.definition.0.package,
            false,
        );
        self.report(
            rule,
            span,
            &format!("{message}: expected `{expected}`, found `{actual}`"),
        );
    }
}
