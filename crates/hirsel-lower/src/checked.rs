//! Adapter for H's shared producer plans, without importing another phase crate.
use super::*;
use hirsel_check_protocol::body::ForProtocol;
use hirsel_syntax::{ExprKind, StmtKind, SyntaxTree, UnaryOp};
use hirsel_tir::Constructor;
use hirsel_types::{CallTarget, ContextualName, GenericArgument, TypeKind};
impl Plans {
    /// Convert H's plans after I/J have enriched the body. The declared function
    /// result is a session row. Unsupported protocols need additional
    /// producer details; no registry identities are guessed here.
    pub fn from_body(
        types: &hirsel_types::TypeInterner,
        tree: &SyntaxTree,
        checked: &CheckedBody,
        source: &hirsel_check_protocol::body::Plans,
        expression: ExprId,
    ) -> Result<Self> {
        let fail = |id: ExprId, message: &str| LowerError {
            site: tree
                .expressions
                .get(id.0 as usize)
                .map(|e| e.origin.span)
                .unwrap_or_default(),
            message: message.into(),
        };
        let expressions = decisions::Table::new(&checked.expressions, |row| row.expression);
        let patterns = decisions::Table::new(&checked.patterns, |row| row.pattern);
        let row = |id| {
            expressions
                .get(&id)
                .first()
                .copied()
                .ok_or_else(|| fail(id, "missing H expression decision"))
        };
        let mut output = Plans::default();
        output.resource_patterns = source.resource_patterns.clone();
        output.contracts = source.contracts.clone();
        output.facts = source.facts.clone();
        output.pattern_literals = source.pattern_literals.clone();
        output.unreachable_arms = source.unreachable_arms.clone();
        for propagation in &source.tries {
            let id = propagation.expression;
            let ExprKind::Try { value, .. } = tree.expressions[id.0 as usize].kind else {
                return Err(fail(id, "H propagation plan refers to non-try"));
            };
            let TypeKind::Named {
                definition,
                arguments: source_arguments,
            } = &types.get(row(value)?.ty).unwrap().kind
            else {
                return Err(fail(id, "H Result source is not nominal"));
            };
            if *definition.path() != propagation.result {
                return Err(fail(id, "H Result identity changed"));
            }
            let [
                GenericArgument::Type(source_success),
                GenericArgument::Type(source_error),
            ] = source_arguments.as_slice()
            else {
                return Err(fail(id, "source Result argument layout is incomplete"));
            };
            let own_result = propagation.propagated_result;
            let TypeKind::Named {
                definition: target,
                arguments,
            } = &types.get(own_result).unwrap().kind
            else {
                return Err(fail(id, "enclosing result is not nominal"));
            };
            let [GenericArgument::Type(_), GenericArgument::Type(mapped)] = arguments.as_slice()
            else {
                return Err(fail(id, "enclosing Result argument layout is incomplete"));
            };
            let mapper = match &propagation.mapper {
                None => None,
                Some(CallTarget::Definition(path)) => Some(ErrorMapper::Definition {
                    path: path.clone(),
                    result: *mapped,
                    passing: propagation
                        .mapper_passing
                        .ok_or_else(|| fail(id, "missing error mapper passing mode"))?,
                }),
                Some(CallTarget::Constructor(name)) => Some(ErrorMapper::Constructor {
                    constructor: constructor(name)
                        .ok_or_else(|| fail(id, "invalid error constructor"))?,
                    result: *mapped,
                }),
                Some(CallTarget::Value(_)) => {
                    return Err(fail(id, "indirect error mapper needs its evaluated plan"));
                }
            };
            output.propagation.push((
                id,
                TryPlan {
                    success_variant: propagation.success_variant.0,
                    error_variant: propagation.error_variant.0,
                    success_type: *source_success,
                    error_type: *source_error,
                    result_constructor: Constructor::Variant {
                        definition: target.path().clone(),
                        variant: propagation.propagated_error_variant.0,
                    },
                    mapper,
                },
            ));
        }
        for plan in &source.loops {
            let StmtKind::For {
                source, pattern, ..
            } = tree.statements[plan.statement().0 as usize].kind
            else {
                return Err(fail(expression, "H loop plan refers to non-for"));
            };
            let execution = match &plan.kind {
                hirsel_check_protocol::body::ForPlanKind::Cursor { cursor } => {
                    output.cursors.push(cursor.clone());
                    continue;
                }
                hirsel_check_protocol::body::ForPlanKind::Direct { execution, .. } => {
                    execution.clone()
                }
            };
            match plan.protocol {
                ForProtocol::Range { inclusive } => {
                    let source = match tree.expressions[source.0 as usize].kind {
                        ExprKind::Unary {
                            op: UnaryOp::Take,
                            value,
                        } => value,
                        _ => source,
                    };
                    let ExprKind::Binary {
                        left: start,
                        right: end,
                        ..
                    } = tree.expressions[source.0 as usize].kind
                    else {
                        return Err(fail(source, "H range lacks endpoints"));
                    };
                    output.iterations.push((
                        plan.statement(),
                        RangePlan {
                            start,
                            end,
                            inclusive,
                        },
                    ));
                }
                _ => match &execution {
                    hirsel_check_protocol::body::ForExecution::Indexed { length, read } => {
                        let mut patterns = patterns.get(&pattern).iter().copied();
                        let Some(item) = patterns.next() else {
                            return Err(fail(source, "indexed loop lacks a checked pattern type"));
                        };
                        if patterns.next().is_some() || types.get(item.ty).is_none() {
                            return Err(fail(
                                source,
                                "indexed loop has an invalid checked pattern type",
                            ));
                        }
                        // I/J refine the complete item judgment in this row.
                        // H's original protocol identifies the operation only.
                        output.protocols.push((
                            plan.statement(),
                            hirsel_check_protocol::body::ForExecution::Indexed {
                                length: length.clone(),
                                read: match read {
                                    hirsel_check_protocol::body::LoopRead::Index { .. } => {
                                        hirsel_check_protocol::body::LoopRead::Index {
                                            result: item.ty,
                                        }
                                    }
                                    hirsel_check_protocol::body::LoopRead::MapIndex {
                                        advance,
                                        ..
                                    } => hirsel_check_protocol::body::LoopRead::MapIndex {
                                        result: item.ty,
                                        advance: advance.clone(),
                                    },
                                },
                            },
                        ));
                    }
                    hirsel_check_protocol::body::ForExecution::Advance { .. } => {
                        output.protocols.push((plan.statement(), execution.clone()));
                    }
                    _ => return Err(fail(source, "loop lacks executable H protocol")),
                },
            }
        }
        output
            .shapes
            .extend(source.shapes.iter().map(|(_, syntax, ty)| (*syntax, *ty)));
        Ok(output)
    }
}
pub(super) fn constructor(name: &ContextualName) -> Option<Constructor> {
    match name {
        ContextualName::Constructor(path) => Some(Constructor::Record(path.clone())),
        ContextualName::Variant { owner, variant } => Some(Constructor::Variant {
            definition: owner.clone(),
            variant: variant.0,
        }),
        ContextualName::Field { .. } => None,
    }
}
