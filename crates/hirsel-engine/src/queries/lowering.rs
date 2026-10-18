//! Check-time lowering: complete for the checked package, reachable for its dependencies.
use super::*;
use hirsel_syntax::ExprKind;
use hirsel_types::ResolvedName;

/// Which executable bodies a file check lowers and validates.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Lowering {
    /// A dependency: only bodies whose lowering evaluates at compile time.
    /// Every other body lowers when the program reaches it.
    CompileTime,
    /// The checked program's own package: every executable body.
    All,
}

impl QueryEngine {
    /// Whether a file check under `policy` lowers this mode-checked definition.
    ///
    /// Lowering reports source diagnostics only through compile-time
    /// evaluation (`prepare_comptime`): a constant initializer, a `comptime`
    /// expression, or a read of a source constant. Otherwise `LowerAdapter`
    /// fails only with invariants and V only with operational violations, so
    /// deferring those bodies leaves every diagnostic and its order unchanged.
    /// Registry dictionary constants materialize without evaluation.
    pub(crate) fn lowers_at_check(&self, definition: &DefPath, policy: Lowering) -> bool {
        if policy == Lowering::All {
            return true;
        }
        let Some(Ok(analyzed)) = self.modes.get(definition).map(|c| c.result.as_ref()) else {
            return true;
        };
        let typed = &analyzed.value.typed;
        let tree = &typed.file.tree;
        if matches!(
            tree.items[typed.item.0 as usize].kind,
            ItemKind::Const { .. }
        ) {
            return true;
        }
        typed.checked.expressions.iter().any(|row| {
            matches!(
                tree.expressions[row.expression.0 as usize].kind,
                ExprKind::Comptime(_)
            )
        }) || typed
            .checked
            .resolution
            .expressions
            .iter()
            .any(|(_, name)| matches!(name, ResolvedName::Definition(path) if self.source_constant(path)))
    }

    fn source_constant(&self, path: &DefPath) -> bool {
        self.items
            .get(path)
            .and_then(|i| i.first())
            .is_some_and(|item| {
                matches!(
                    item.file.tree.items[item.item.0 as usize].kind,
                    ItemKind::Const { .. }
                )
            })
    }

    /// Whether every body a file check under `policy` lowers is lowered.
    pub(crate) fn package_lowered(
        &self,
        files: &[(FileId, FileContext)],
        policy: Lowering,
    ) -> bool {
        files.iter().all(|(f, _)| {
            let Some((_, _, Ok(parsed))) = self.parsed.get(f) else {
                return false;
            };
            parsed.value.roots.iter().all(|id| {
                let item = &parsed.value.tree.items[id.0 as usize];
                !generation::executable(&parsed.value, *id)
                    || self.lowered.contains_key(&item.definition)
                    || !self.lowers_at_check(&item.definition, policy)
            })
        })
    }

    /// Every top-level declaration of an indexed file, in source order.
    pub(crate) fn file_definitions(&self, file: FileId) -> Vec<DefPath> {
        let Some((_, _, Ok(parsed))) = self.parsed.get(&file) else {
            return vec![];
        };
        let tree = &parsed.value.tree;
        parsed
            .value
            .roots
            .iter()
            .map(|id| tree.items[id.0 as usize].definition.clone())
            .collect()
    }

    /// Every function of an indexed file, and whether it is an `extern`.
    pub(crate) fn file_functions(&self, file: FileId) -> Vec<(DefPath, bool)> {
        let Some((_, _, Ok(parsed))) = self.parsed.get(&file) else {
            return vec![];
        };
        let tree = &parsed.value.tree;
        parsed
            .value
            .roots
            .iter()
            .filter_map(|id| {
                let item = &tree.items[id.0 as usize];
                match &item.kind {
                    ItemKind::Function { abi, .. } => {
                        Some((item.definition.clone(), abi.is_some()))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Lower and validate every executable body reachable from `roots`. This is
    /// the closure the semantic program revision encodes: calls, generic
    /// dictionary and function references, closures (bodies of their owner),
    /// type finalizers, constants and loop protocol reads all arrive as checked
    /// signature, resolver, mode or lowering reads.
    pub(crate) fn lower_reachable(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<(), QueryError> {
        self.semantic_program(roots)?;
        self.prepare_staged_instances()
    }
}
