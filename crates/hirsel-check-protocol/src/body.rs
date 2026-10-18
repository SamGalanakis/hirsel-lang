//! H-produced plans consumed after mode analysis by audits and TIR lowering.
use hirsel_syntax as ast;
use hirsel_types::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TryPlan {
    pub expression: ast::ExprId,
    pub result: ast::DefPath,
    pub mapper: Option<CallTarget>,
    pub mapper_passing: Option<ast::PassingMode>,
    pub success_variant: VariantId,
    pub error_variant: VariantId,
    pub propagated_error_variant: VariantId,
    pub error_type: TypeId,
    pub propagated_result: TypeId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionPlan {
    pub owner: NameSite,
    pub result: TypeId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForPlan {
    pub protocol: ForProtocol,
    pub kind: ForPlanKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoopRead {
    /// SEM-4 sequence indexing; no library function is called.
    Index { result: TypeId },
}
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Plans {
    /// Finalizer-resource patterns and their complete physical field types.
    pub resource_patterns: Vec<(ast::PatternId, Vec<TypeId>)>,
    /// Ordered H-selected record, tuple, and variant payload projections.
    pub pattern_literals: Vec<(ast::PatternId, ast::Literal)>,
    pub tries: Vec<TryPlan>,
    pub loops: Vec<ForPlan>,
    pub functions: Vec<FunctionPlan>,
    pub shapes: Vec<(ast::ExprId, ast::TypeExprId, TypeId)>,
    pub contracts: Vec<ContractPlan>,
    pub facts: Vec<FactPlan>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactPlan {
    pub expression: ast::ExprId,
    pub predicate: ast::DefPath,
    pub arguments: Vec<GenericArgument>,
    pub result: TypeId,
    pub usage: UseKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractPlan {
    pub owner: NameSite,
    pub expression: ast::ExprId,
    pub kind: ast::ContractKind,
    pub snapshots: Vec<ast::ExprId>,
    /// H-owned capture-free clause convention, in a distinct lexical frame.
    pub function: TypeId,
    pub parameters: Vec<ContractParameter>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractParameter {
    Parameter(ParamId),
    Snapshot(ast::ExprId),
    Result,
}

/// The lexical size inputs needed to stage one checked expression. Sizes
/// declared inside the expression belong to its ordinary nested calls instead.
/// Result types count too: an empty Inline can receive its size from context.
pub fn comptime_inputs(
    tree: &ast::SyntaxTree,
    checked: &CheckedBody,
    expression: ast::ExprId,
) -> Vec<SizeParameterRef> {
    use std::collections::BTreeSet;
    let region = tree.expressions[expression.0 as usize].origin.span;
    let contains = |site: NameSite| {
        let span = match site {
            NameSite::Item(id) => tree.items[id.0 as usize].origin.span,
            NameSite::Statement(id) => tree.statements[id.0 as usize].origin.span,
            NameSite::Expression(id) => tree.expressions[id.0 as usize].origin.span,
            NameSite::Pattern(id) => tree.patterns[id.0 as usize].origin.span,
            NameSite::Type(id) => tree.types[id.0 as usize].origin.span,
        };
        span.start >= region.start && span.end <= region.end
    };
    let outer = |parameter: SizeParameterRef| match parameter.binder() {
        GenericBinder::Body(scope) => !contains(scope),
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
    let mut pending: Vec<_> = checked.expressions.iter()
        .filter(|row| contains(NameSite::Expression(row.expression)))
        .map(|row| row.ty).collect();
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
    let mut seen = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) { continue; }
        let row = checked.types.get(ty).unwrap();
        if let TypeKind::Builtin { arguments, .. } | TypeKind::Named { arguments, .. } = &row.kind {
            for argument in arguments.iter() {
                if let GenericArgument::Size(Size::Parameter(parameter)) = argument {
                    if outer(*parameter) { inputs.insert(*parameter); }
                }
            }
        }
        pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
    }
    inputs.into_iter().collect()
}

/// Private constant declaration owned by an inline staged expression.
pub fn comptime_path(owner: &ast::DefPath, expression: ast::ExprId) -> ast::DefPath {
    let mut path = owner.clone();
    path.0.name = format!("$comptime:{}:{}", owner.0.name, expression.0);
    path
}
