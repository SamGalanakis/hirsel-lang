//! Check-time lowering under a `LoweringPolicy`.
use super::*;
use hirsel_syntax::ExprKind;
use hirsel_types::ResolvedName;

impl QueryEngine {
    /// Whether a file check under `policy` lowers this mode-checked definition.
    /// A file's own reachable closure is every body it declares.
    ///
    /// Lowering reports source diagnostics only through compile-time
    /// evaluation (`prepare_comptime`): a constant initializer, a `comptime`
    /// expression, or a read of a source constant. Otherwise `LowerAdapter`
    /// fails only with invariants and V only with operational violations, so
    /// deferring those bodies leaves every diagnostic and its order unchanged.
    /// Registry dictionary constants materialize without evaluation.
    pub(crate) fn lowers_at_check(&self, definition: &DefPath, policy: LoweringPolicy) -> bool {
        if policy == LoweringPolicy::Reachable {
            return true;
        }
        let id = hirsel_syntax::DefId::intern(definition);
        let Some(Ok(analyzed)) = self.modes.get(&id).map(|e| e.memo.value.as_ref()) else {
            return true;
        };
        let typed = &analyzed.value.typed;
        let tree = &typed.syntax.tree;
        if matches!(typed.syntax.item.kind, ItemKind::Const { .. }) {
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
        match self.items.get(path).and_then(|i| i.first()) {
            Some(item) => matches!(item.item.kind, ItemKind::Const { .. }),
            None => self.image_signature(path).is_some_and(|signature| {
                matches!(
                    signature.output.value.kind,
                    hirsel_types::SignatureKind::Const { .. }
                )
            }),
        }
    }

    /// Whether every body a file check under `policy` lowers is lowered.
    pub(crate) fn package_lowered(
        &self,
        files: &[(FileId, FileContext)],
        policy: LoweringPolicy,
    ) -> bool {
        files.iter().all(|(f, _)| {
            let Some((_, _, Ok(parsed))) = self.parsed.get(f) else {
                return false;
            };
            parsed.value.items.iter().all(|syntax| {
                let item = &syntax.item;
                !generation::executable(syntax)
                    || self
                        .lowered
                        .contains_key(&hirsel_syntax::DefId::intern(&item.definition))
                    || !self.lowers_at_check(&item.definition, policy)
            })
        })
    }

    /// Every top-level declaration of an indexed file, in source order.
    pub(crate) fn file_definitions(&self, file: FileId) -> Vec<DefPath> {
        if let Some(definitions) = self.image_file_definitions(file) {
            return definitions;
        }
        let Some((_, _, Ok(parsed))) = self.parsed.get(&file) else {
            return vec![];
        };
        parsed
            .value
            .items
            .iter()
            .map(|syntax| syntax.item.definition.clone())
            .collect()
    }

    /// Every function of an indexed file, and whether it is an `extern`.
    pub(crate) fn file_functions(&self, file: FileId) -> Vec<(DefPath, bool)> {
        let Some((_, _, Ok(parsed))) = self.parsed.get(&file) else {
            return vec![];
        };
        parsed
            .value
            .items
            .iter()
            .filter_map(|syntax| {
                let item = &syntax.item;
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
        let _timing = timings::interval(Interval::Parallel);
        self.semantic_program(roots)?;
        self.prepare_staged_instances()
    }
}
