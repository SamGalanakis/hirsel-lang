//! H-produced plans consumed after mode analysis by audits and TIR lowering.
use hirsel_syntax as ast;
use hirsel_types::*;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TryPlan {
    pub expression: ast::ExprId,
    pub result: ast::DefPath,
    pub mapper: Option<CallTarget>,
    pub mapper_passing: Option<ast::PassingMode>,
    /// The instantiated declared mapper result, before the enclosing error
    /// payload receives it. A weaker receiver cannot change a call receipt.
    #[serde(default)]
    pub mapper_result: Option<TypeId>,
    pub success_variant: VariantId,
    pub error_variant: VariantId,
    pub propagated_error_variant: VariantId,
    pub error_type: TypeId,
    pub propagated_result: TypeId,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FunctionPlan {
    pub owner: NameSite,
    pub result: TypeId,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ForPlan {
    pub protocol: ForProtocol,
    pub kind: ForPlanKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ForPlanKind {
    Cursor {
        cursor: CursorPlan,
    },
    Direct {
        statement: ast::StmtId,
        execution: ForExecution,
    },
}
impl ForPlan {
    pub fn statement(&self) -> ast::StmtId {
        match &self.kind {
            ForPlanKind::Cursor { cursor } => cursor.statement,
            ForPlanKind::Direct { statement, .. } => *statement,
        }
    }
    pub fn cursor(&self) -> Option<&CursorPlan> {
        match &self.kind {
            ForPlanKind::Cursor { cursor } => Some(cursor),
            ForPlanKind::Direct { .. } => None,
        }
    }
}
/// Checked execution protocol, independent of implementation bodies or lookups.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ForExecution {
    Range,
    Indexed {
        length: LoopLength,
        read: LoopRead,
    },
    Advance {
        operation: ast::DefPath,
        step: OptionLayout,
    },
    Recovery,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum LoopLength {
    Definition {
        definition: ast::DefPath,
        result: TypeId,
    },
    Inline {
        size: Size,
        result: TypeId,
    },
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum LoopRead {
    /// SEM-4 sequence indexing; no library function is called.
    Index { result: TypeId },
    /// Insertion-ordered Map storage can contain holes. Advance visits each
    /// physical slot once, while the source item remains an ordinary Index.
    MapIndex {
        result: TypeId,
        advance: ast::DefPath,
    },
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ForProtocol {
    Range { inclusive: bool },
    Sequence(BuiltinType),
    Bytes,
    ConsumingSequence(BuiltinType),
    Map(BuiltinType),
    Set,
    Iterator,
    BorrowedSource(ast::DefPath),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct Plans {
    /// Finalizer-resource patterns and their complete physical field types.
    pub resource_patterns: Vec<(ast::PatternId, Vec<TypeId>)>,
    /// Ordered H-selected record, tuple, and variant payload projections.
    pub pattern_literals: Vec<(ast::PatternId, ast::Literal)>,
    /// Catch-all arms covered by earlier unguarded patterns. Their bodies still check,
    /// but have no executable path and must not be lowered.
    pub unreachable_arms: Vec<ast::PatternId>,
    pub tries: Vec<TryPlan>,
    pub loops: Vec<ForPlan>,
    pub functions: Vec<FunctionPlan>,
    pub shapes: Vec<(ast::ExprId, ast::TypeExprId, TypeId)>,
    pub contracts: Vec<ContractPlan>,
    pub facts: Vec<FactPlan>,
    /// Plain `?` sites with no arm for their error (LANG-8), each with the
    /// arm its diagnostic proposes adding to the enclosing error's declaration.
    pub arm_fixes: Vec<ArmFix>,
}
/// The "no arm" repair of a plain `?` (LANG-8): append `arm` to the variant
/// declaration of `owner`. The checker sees no declaration source, so the
/// engine places the edit and attaches it to the diagnostic at `site`.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ArmFix {
    pub site: hirsel_span::Span,
    pub owner: ast::DefPath,
    /// The arm text, such as `File(FileError)`.
    pub arm: String,
    /// The owner as the diagnostic spells it.
    pub target: String,
    /// The owner is `@serializable`, so the arm changes its schema (SER-2, STORE-7).
    pub serializable: bool,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FactPlan {
    pub expression: ast::ExprId,
    pub predicate: ast::DefPath,
    pub arguments: Vec<GenericArgument>,
    pub result: TypeId,
    pub usage: UseKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ContractPlan {
    pub owner: NameSite,
    pub expression: ast::ExprId,
    pub kind: ast::ContractKind,
    pub snapshots: Vec<ast::ExprId>,
    /// H-owned capture-free clause convention, in a distinct lexical frame.
    pub function: TypeId,
    pub parameters: Vec<ContractParameter>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ContractParameter {
    Parameter(ParamId),
    Snapshot(ast::ExprId),
    Result,
}

/// The lexical size inputs needed to stage one checked expression. Sizes
/// declared inside the expression belong to its ordinary nested calls instead.
/// Result types count too: an empty Inline can receive its size from context.
pub fn comptime_inputs(
    t: &TypeInterner,
    syntax: &ast::ItemSyntax,
    checked: &CheckedBody,
    expression: ast::ExprId,
) -> Vec<SizeParameterRef> {
    use std::collections::BTreeSet;
    let tree = &syntax.tree;
    let region = tree.expressions[expression.0 as usize].origin.span;
    let contains = |site: NameSite| {
        let span = match site {
            NameSite::Item => syntax.item.origin.span,
            NameSite::Statement(id) => tree.statements[id.0 as usize].origin.span,
            NameSite::Expression(id) => tree.expressions[id.0 as usize].origin.span,
            NameSite::Pattern(id) => tree.patterns[id.0 as usize].origin.span,
            NameSite::Type(id) => tree.types[id.0 as usize].origin.span,
        };
        span.start >= region.start && span.end <= region.end
    };
    let owner = syntax.owner();
    let outer = |parameter: SizeParameterRef| match parameter.binder() {
        GenericBinder::Body(scope) => scope.owner != owner || !contains(scope.site),
        GenericBinder::Bound(_) => false,
    };
    let mut inputs = BTreeSet::new();
    for (id, name) in &checked.resolution.expressions {
        if contains(NameSite::Expression(*id)) {
            if let ResolvedName::Size(parameter) = name {
                if outer(*parameter) {
                    inputs.insert(*parameter);
                }
            }
        }
    }
    let mut pending: Vec<_> = checked
        .expressions
        .iter()
        .filter(|row| contains(NameSite::Expression(row.expression)))
        .map(|row| row.ty)
        .collect();
    // Inferred generic call actuals may mention a size absent from the result.
    for call in &checked.calls {
        if contains(NameSite::Expression(call.expression)) {
            for argument in &call.generic_arguments {
                match argument {
                    GenericArgument::Size(Size::Parameter(parameter)) if outer(*parameter) => {
                        inputs.insert(*parameter);
                    }
                    GenericArgument::Type(ty) => pending.push(*ty),
                    _ => (),
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        let row = t.row(ty);
        if let TypeKind::Builtin { arguments, .. } | TypeKind::Named { arguments, .. } = &row.kind {
            for argument in arguments.iter() {
                if let GenericArgument::Size(Size::Parameter(parameter)) = argument {
                    if outer(*parameter) {
                        inputs.insert(*parameter);
                    }
                }
            }
        }
        pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
    }
    inputs.into_iter().collect()
}

/// Private constant declaration owned by an inline staged expression.
pub fn comptime_path(
    owner: &ast::DefPath,
    checked: &CheckedBody,
    expression: ast::ExprId,
) -> ast::DefPath {
    let mut path = owner.clone();
    // Expression ids are file-wide; unrelated definitions must not rename this constant.
    let ordinal = checked
        .expressions
        .iter()
        .filter(|row| row.expression < expression)
        .count();
    path.0.name = format!("$comptime:{}:{}", owner.0.name, ordinal);
    path
}
