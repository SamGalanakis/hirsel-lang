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
