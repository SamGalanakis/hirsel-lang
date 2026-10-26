//! Mode-table-driven lowering. Missing judgments are compiler contract errors.
//! Synthetic Take operands relocate compiler-owned temporaries; they do not
//! authorize a source move. Every source use comes from CheckedBody.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_intern::work::{self, Counter};
mod bindings;
mod borrow_cursors;
mod builder;
mod checked;
mod control;
mod decisions;
mod expression;
mod iteration;
mod last_use;
mod lexical;
mod literal;
mod live;
mod loans;
mod pattern;
pub mod print;
mod stability;

use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefPath, ExprId, ItemSyntax, StmtId, TypeExprId};
use hirsel_tir::{Constant, Constructor, Definition, Materialized};
use hirsel_types::{CheckedBody, NameSite, OwnershipFacts, TypeId};

/// Producer-authored control, layout and materialization decisions.
/// H/J supply checked decisions; N/O supply contracts/constants. Lowering
/// transports these rows without reconstructing their type or mode judgments.
#[derive(Clone, Debug, Default)]
pub struct Plans {
    pub resource_patterns: Vec<(hirsel_syntax::PatternId, Vec<TypeId>)>,
    /// O evaluates a closed zero-input entry, retaining ordinary nested frames.
    pub evaluation_frame: bool,
    pub materialized: Vec<(ExprId, Materialized)>,
    pub staged: Vec<(ExprId, TypeId)>,
    /// Nominal variant cardinalities proved by H for constructor patterns.
    pub pattern_literals: Vec<(hirsel_syntax::PatternId, hirsel_syntax::Literal)>,
    /// H-proved catch-all arms covered by earlier unguarded patterns.
    pub unreachable_arms: Vec<hirsel_syntax::PatternId>,
    pub entry_checks: Vec<(NameSite, ExprId, hirsel_diag::RuleId)>,
    pub contracts: Vec<hirsel_check_protocol::body::ContractPlan>,
    pub facts: Vec<hirsel_check_protocol::body::FactPlan>,
    pub propagation: Vec<(ExprId, TryPlan)>,
    pub shapes: Vec<(TypeExprId, TypeId)>,
    pub constants: Vec<(ExprId, Constant)>,
    pub iterations: Vec<(StmtId, RangePlan)>,
    pub protocols: Vec<(StmtId, hirsel_check_protocol::body::ForExecution)>,
    /// Shared checked implicit calls, including their declaration identities.
    pub cursors: Vec<hirsel_types::CursorPlan>,
}
/// H selects this only for a checked integer range source (SEM-4).
#[derive(Clone, Debug)]
pub struct RangePlan {
    pub start: ExprId,
    pub end: ExprId,
    pub inclusive: bool,
}
/// Nominal Result identities/ordinals come from H/M, never from name spelling.
#[derive(Clone, Debug)]
pub struct TryPlan {
    pub success_variant: u32,
    pub error_variant: u32,
    /// The source Result's own payload arguments. The projected places read
    /// them through the Result, keeping its bounds (OWN-8), and the success
    /// payload is then received at the `?` expression's checked type (MODE-2
    /// retained uniqueness).
    pub success_type: TypeId,
    pub error_type: TypeId,
    pub result_constructor: Constructor,
    pub mapper: Option<ErrorMapper>,
}
#[derive(Clone, Debug)]
pub enum ErrorMapper {
    Definition {
        path: DefPath,
        result: TypeId,
        passing: hirsel_syntax::PassingMode,
    },
    Constructor {
        constructor: Constructor,
        result: TypeId,
    },
}
pub struct Input<'a> {
    /// The session interner: checked rows are read and lowered rows interned here.
    pub types: &'a hirsel_types::TypeInterner,
    pub track_caller: bool,
    pub syntax: &'a ItemSyntax,
    pub checked: &'a CheckedBody,
    pub plans: &'a Plans,
    pub ownership: &'a OwnershipFacts,
    /// J's formal frame results in the body-origin namespace, one per source frame.
    pub result_types: &'a [(NameSite, TypeId)],
    pub owner: NameSite,
    pub expression: ExprId,
    pub path: DefPath,
    pub signature_hash: ContentHash,
    pub unit: TypeId,
    pub boolean: TypeId,
    /// The heap role's Arena type; a relation place whose binding has this
    /// type places blocks rather than bounding a lifetime.
    pub arena: Option<DefPath>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LowerError {
    pub site: Span,
    pub message: String,
}
impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lowering contract: {} at {:?}", self.message, self.site)
    }
}
impl std::error::Error for LowerError {}
type Result<T> = std::result::Result<T, LowerError>;

/// The outer frame is reserved before nested closure/contract bodies are appended.
/// Driver metadata can name this layout without forcing an implementation query.
pub fn entry_body() -> hirsel_tir::BodyId {
    hirsel_tir::BodyId(0)
}

/// Called only after driver mode-stage guards. No source-rule diagnostics are
/// manufactured here. The returned Definition's type table is the closure of its bodies' rows.
pub fn lower(input: Input<'_>) -> Result<Definition> {
    let decisions = decisions::Decisions::new(&input);
    let mut bodies = vec![None];
    let mut builder = builder::Builder::new(
        &input,
        &decisions,
        input.owner.clone(),
        decisions.formal_result(&input.owner, input.types)?,
    )?;
    let span = builder.span(input.expression)?;
    builder.entry_checks(&mut bodies)?;
    if let Some(value) = builder.expression(input.expression, &mut bodies)? {
        builder.return_value(value, span, &mut bodies)?;
    }
    bodies[0] = Some(builder.finish()?);
    let mut contracts = vec![];
    let mut caller_contracts = vec![];
    for clause in &input.plans.contracts {
        let mut found = false;
        for (index, body) in bodies.iter().enumerate() {
            if body.as_ref().is_some_and(|body| {
                let mut scope = body.parameter_scope;
                while let Some(original) = decisions.cloned_scopes.borrow().get(&scope) {
                    scope = *original;
                }
                scope
                    == decisions
                        .lexical
                        .site(NameSite::Expression(clause.expression))
            }) {
                found = true;
                let body = hirsel_tir::BodyId(index as u32);
                contracts.push((contracts.len() as u32, body));
                if clause.owner == input.owner {
                    caller_contracts.push(body);
                }
            }
        }
        if !found {
            return Err(LowerError {
                site: Span::default(),
                message: "missing executable clause body".into(),
            });
        }
    }
    let mut protocol_associations = Vec::new();
    for cursor in &input.plans.cursors {
        let Some(association) = &cursor.association else {
            continue;
        };
        let before = protocol_associations.len();
        let span = input.syntax.tree.statements[cursor.statement.0 as usize]
            .origin
            .span;
        for (body_index, body) in bodies.iter().enumerate() {
            let body = body.as_ref().unwrap();
            let call = |path: &DefPath| {
                body.blocks.iter().enumerate().find_map(|(i,b)| {
                (b.span == span && matches!(&b.terminator, hirsel_tir::Terminator::Call { callee: hirsel_tir::Callee::Definition { path: p, .. }, .. } if p == path)).then_some(hirsel_tir::BlockId(i as u32))
            })
            };
            if let (Some(open), Some(next)) = (call(&cursor.open), call(&cursor.next)) {
                protocol_associations.push(hirsel_tir::ProtocolAssociationReceipt {
                    association: association.clone(),
                    body: hirsel_tir::BodyId(body_index as u32),
                    open,
                    next,
                });
            }
        }
        if protocol_associations.len() == before {
            return Err(LowerError {
                site: span,
                message: "missing borrowed protocol call receipt".into(),
            });
        }
    }
    let mut definition = Definition {
        caller_bodies: if input.track_caller {
            let mut bodies = vec![entry_body()];
            bodies.extend(caller_contracts);
            bodies.sort_unstable_by_key(|body| body.0);
            bodies.dedup();
            bodies
        } else {
            vec![]
        },
        protocol_associations,
        types: std::sync::Arc::new([]),
        path: hirsel_syntax::DefId::intern(&input.path),
        inferred_unique: Default::default(),
        signature_hash: input.signature_hash,
        bodies: bodies.into_iter().map(Option::unwrap).collect(),
        entry: entry_body(),
        contracts,
    };
    definition.types = hirsel_tir::definition_types(input.types, &definition);
    Ok(definition)
}

#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod cursor_tests;
