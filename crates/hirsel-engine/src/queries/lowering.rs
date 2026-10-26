//! Check-time lowering under a `LoweringPolicy`.
use super::*;
use hirsel_syntax::ExprKind;
use hirsel_types::ResolvedName;

/// Compact classification of the final mode body's check-time lowering needs.
/// Referenced declarations are interpreted against current source/image kind;
/// membership is never inferred from a producer fingerprint.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LoweringUse {
    constant: bool,
    comptime: bool,
    referenced: Vec<DefId>,
}
impl LoweringUse {
    pub(crate) fn capture(typed: &TypedBody) -> Self {
        let tree = &typed.syntax.tree;
        Self {
            constant: matches!(typed.syntax.item.kind, ItemKind::Const { .. }),
            comptime: typed.checked.expressions.iter().any(|row| {
                matches!(
                    tree.expressions[row.expression.0 as usize].kind,
                    ExprKind::Comptime(_)
                )
            }),
            referenced: typed
                .checked
                .resolution
                .expressions
                .iter()
                .filter_map(|(_, name)| {
                    if let ResolvedName::Definition(path) = name {
                        Some(DefId::intern(path))
                    } else {
                        None
                    }
                })
                .collect(),
        }
    }
    /// Conservative capacity bound of capture's filtered Vec. Its iterator
    /// grows geometrically from at least four elements; DefIds are interned.
    pub(crate) fn capture_bytes(typed: &TypedBody) -> usize {
        let count = typed
            .checked
            .resolution
            .expressions
            .iter()
            .filter(|(_, name)| matches!(name, ResolvedName::Definition(_)))
            .count();
        if count == 0 {
            0
        } else {
            count
                .max(4)
                .saturating_mul(2)
                .saturating_mul(std::mem::size_of::<DefId>())
        }
    }
    fn needed(&self, engine: &QueryEngine) -> bool {
        self.needed_with(|id| engine.source_constant(id.path()))
    }
    pub(super) fn needed_with(&self, mut constant: impl FnMut(DefId) -> bool) -> bool {
        self.constant || self.comptime || self.referenced.iter().copied().any(&mut constant)
    }
}

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
        if let Some(inputs) = self.front_products.lowering_inputs.get(&id)
            && self
                .summaries
                .get(&id)
                .is_some_and(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
        {
            return inputs.needed(self);
        }
        let Some(Ok(analyzed)) = self.modes.get(&id).map(|e| e.memo.value.as_ref()) else {
            return true;
        };
        let typed = &analyzed.value.typed;
        matches!(typed.syntax.item.kind, ItemKind::Const { .. })
            || typed.checked.expressions.iter().any(|row| {
                matches!(typed.syntax.tree.expressions[row.expression.0 as usize].kind, ExprKind::Comptime(_))
            }) || typed.checked.resolution.expressions.iter().any(|(_, name)| {
                matches!(name, ResolvedName::Definition(path) if self.source_constant(path))
            })
    }

    fn source_constant(&self, path: &DefPath) -> bool {
        match self.items.get(path).and_then(|i| i.first()) {
            Some(item) => matches!(item.item.kind, ItemKind::Const { .. }),
            None if self.catalog.body(path).is_some() => {
                self.catalog.facts(path).is_some_and(|facts| facts.constant)
            }
            None => false,
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
                    || self
                        .lower_receipts
                        .contains_key(&hirsel_syntax::DefId::intern(&item.definition))
                    || !self.lowers_at_check(&item.definition, policy)
            })
        })
    }

    /// Every top-level declaration of an indexed file, in source order.
    pub(crate) fn file_definitions(&self, file: FileId) -> Vec<DefPath> {
        if let Some(definitions) = self.catalog.file_definitions(file) {
            return definitions;
        }
        self.source_items(file)
            .into_iter()
            .map(|item| item.definition)
            .collect()
    }

    /// Every function of an indexed file, and whether it is an `extern`.
    pub(crate) fn file_functions(&self, file: FileId) -> Vec<(DefPath, bool)> {
        let syntax: Vec<_> = match self.parsed.get(&file) {
            Some((_, _, Ok(parsed))) => parsed.value.items.iter().collect(),
            _ => self.retained_source_syntax(file),
        };
        syntax
            .into_iter()
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
