use super::{CaptureContext, CaptureMode, Place, Projection};
use hirsel_span::Span;
use hirsel_syntax::*;
use hirsel_types::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Context {
    Read,
    /// A borrow-only use: a borrowed argument or an operand read through. A
    /// part used this way is a view, not a share (MODE-2/OWN-8, #76).
    View,
    Own,
    /// An owned receiver: return, aggregate field or replacement.
    Store,
    /// Sharing a selected borrowed source into the declared local result.
    ResultShare,
    Copy,
    Take,
    Mut,
    Unique,
    Invoke,
    Address,
}
#[derive(Clone, Debug)]
pub(super) struct Requirement {
    pub ty: TypeId,
    pub parameter: usize,
    pub passing: PassingMode,
    pub modes: Modes,
    pub sendable_bound: bool,
}
#[derive(Clone, Debug)]
pub(super) enum Op {
    Nop,
    ReturnUnit,
    IdentityReady(NameSite),
    IterationLoan {
        source: ExprId,
        begin: bool,
    },
    /// The place an evaluated argument names is reserved (`mut`) or lent
    /// (borrowed) while the later arguments of its call are evaluated
    /// (LANG-3, OWN-3). The call ends the reservation.
    ArgumentLoan {
        argument: ExprId,
        call: ExprId,
    },
    Use {
        expression: ExprId,
        context: Context,
        operation: ExprId,
    },
    Finish {
        expression: ExprId,
        context: Context,
        children: Vec<ExprId>,
        fresh: bool,
    },
    Forward {
        expression: ExprId,
        child: ExprId,
    },
    Bind {
        pattern: PatternId,
        value: ExprId,
        guarded: bool,
        test: bool,
    },
    Write {
        place: ExprId,
        value: ExprId,
        operation: Span,
        compound: bool,
    },
    Call {
        expression: ExprId,
        callee: ExprId,
        arguments: Vec<Argument>,
        requirements: Vec<Option<Requirement>>,
        result: Modes,
        constructor: bool,
        /// A constructor's declared part type for each written argument.
        parts: Vec<Option<TypeId>>,
        /// The arguments whose values the declared result's erased
        /// environment captures, when every captured parameter is written.
        captures: Option<Vec<usize>>,
    },
    Capture {
        expression: ExprId,
        escaping: bool,
        owning: bool,
    },
    Return {
        value: ExprId,
        result: Modes,
    },
    Kill(Vec<usize>),
    /// The block statement's temporaries are released (LANG-3).
    StatementEnd(StmtId),
    LocalFunction(StmtId),
}
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub op: Op,
    pub next: Vec<usize>,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub(super) struct Graph {
    pub nodes: Vec<Node>,
    pub entry: usize,
    pub owner: NameSite,
    pub result_shareable: BTreeSet<TypeParameterRef>,
    pub result: Modes,
    pub result_ty: Option<TypeId>,
    pub expressions: Vec<ExprId>,
    pub scoped: bool,
    pub clause: Option<ExprId>,
    // Identities only: nested graphs read the current published capture row.
    pub captures: Vec<(NameSite, ResolvedName, usize)>,
    pub capture_modes: std::sync::Arc<super::source_rows::CaptureRows>,
    pub lifetimes: std::sync::Arc<LifetimeScopes>,
    /// Block tails that move a binding the block declares out to the consumer.
    pub leaving: BTreeSet<ExprId>,
    /// Block tails that project a part of a binding the block declares,
    /// with that block and its consumer's operation: a consumer that only
    /// borrows the part would read storage the block ends (LANG-11).
    pub ending: BTreeMap<ExprId, (ExprId, ExprId)>,
    pub statements: Statements,
}
/// The block statement whose temporaries hold each evaluated value, and the
/// statement each block statement is nested in. A block's tail belongs to
/// the statement holding the block, as lowering extracts it there.
#[derive(Clone, Debug, Default)]
pub(super) struct Statements {
    pub temporaries: BTreeMap<ExprId, StmtId>,
    pub parents: BTreeMap<StmtId, StmtId>,
}
impl Statements {
    /// Whether `inner` is `outer` or nested in it.
    pub fn within(&self, mut inner: StmtId, outer: StmtId) -> bool {
        loop {
            if inner == outer {
                return true;
            }
            let Some(&parent) = self.parents.get(&inner) else {
                return false;
            };
            inner = parent;
        }
    }
}
/// Lexical source lifetimes, recorded while constructing the ownership graph.
/// Occurrences belong to the enclosing scope; a block creates a child for its
/// contents. This is independent of syntax spans and survives generated syntax.
#[derive(Clone, Debug)]
pub(super) struct LifetimeScopes {
    parents: Vec<Option<usize>>,
    pub expressions: BTreeMap<ExprId, usize>,
    pub bindings: BTreeMap<usize, usize>,
}
impl LifetimeScopes {
    /// The frame itself: the scope of a value that ends with the function.
    pub const FRAME: usize = 0;
    fn new() -> Self {
        Self {
            parents: vec![None],
            expressions: BTreeMap::new(),
            bindings: BTreeMap::new(),
        }
    }
    fn child(&mut self, parent: usize) -> usize {
        let child = self.parents.len();
        self.parents.push(Some(parent));
        child
    }
    pub fn contains(&self, source: usize, mut receiving: usize) -> bool {
        loop {
            if source == receiving {
                return true;
            }
            let Some(parent) = self.parents[receiving] else {
                return false;
            };
            receiving = parent;
        }
    }
}
#[derive(Clone)]
struct Environment {
    owner: NameSite,
    breaks: Option<(usize, usize, usize)>,
    scopes: Vec<Vec<usize>>,
    lifetime: usize,
    result: Modes,
    /// The nearest operation that consumes this expression's value through
    /// transparent blocks, branches and arms (LANG-11).
    consumer: Option<ExprId>,
    /// The block statement whose temporaries this expression's value joins.
    statement: Option<StmtId>,
}
enum Job {
    Expr {
        entry: usize,
        expression: ExprId,
        context: Context,
        operation: ExprId,
        escaping: bool,
        owning: bool,
        next: usize,
        env: Environment,
    },
    Stmt {
        entry: usize,
        statement: StmtId,
        next: usize,
        env: Environment,
    },
}
fn node(nodes: &mut Vec<Node>, op: Op, next: Vec<usize>, span: Span) -> usize {
    let id = nodes.len();
    nodes.push(Node { op, next, span });
    id
}
fn reserve(nodes: &mut Vec<Node>, span: Span) -> usize {
    node(nodes, Op::Nop, vec![], span)
}
pub(super) fn binders(
    rows: &super::source_rows::SourceRows,
    patterns: &[PatternId],
    tree: &SyntaxTree,
) -> Vec<usize> {
    let mut result = vec![];
    let mut todo = patterns.to_vec();
    while let Some(pattern) = todo.pop() {
        result.extend(rows.at(NameSite::Pattern(pattern)));
        match &tree.patterns[pattern.0 as usize].kind {
            PatternKind::Tuple(children) => todo.extend(children),
            PatternKind::Constructor { fields, .. } => {
                todo.extend(fields.iter().map(|f| f.pattern))
            }
            _ => {}
        }
    }
    result
}
pub(super) fn place(
    body: &CheckedBody,
    expression: ExprId,
    rows: &super::source_rows::SourceRows,
) -> Option<Place> {
    let source = &body.places[rows.source_place(expression).expect("H source place")];
    let SourceRoot::Binding {
        owner,
        binding: name,
    } = &source.root
    else {
        return None;
    };
    let binding = rows
        .binding(body, Some(*owner), name)
        .expect("checked place binding");
    let projections = source
        .projections
        .iter()
        .map(|p| match p.kind {
            SourceProjectionKind::Field(field) => Projection::Field(field.0),
            SourceProjectionKind::VariantField { variant, field } => Projection::VariantField {
                variant: variant.0,
                field: field.0,
            },
            SourceProjectionKind::Index(_) => Projection::Elements,
        })
        .collect();
    Some(Place {
        binding,
        projections,
    })
}

pub(super) fn build(
    tree: &SyntaxTree,
    item: ItemId,
    own: &Signature,
    body: &mut CheckedBody,
    capture_modes: &[CaptureMode],
    clauses: &[(ExprId, TypeId)],
    results: &[(NameSite, TypeId)],
    rows: &super::source_rows::SourceRows,
) -> Result<Vec<Graph>, QueryError> {
    let root = match &tree.items[item.0 as usize].kind {
        ItemKind::Function {
            body: Some(root), ..
        } => *root,
        ItemKind::Const { value, .. } => *value,
        ItemKind::Type {
            drop: Some(drop), ..
        } => drop.body,
        _ => return Ok(vec![]),
    };
    let capture_modes = std::sync::Arc::new(super::source_rows::CaptureRows::new(capture_modes));
    // Flow tracks lifetimes: a result's own Arena parameters place it (PROV-4).
    let arenas = crate::arenas::Arenas::new(body, own);
    let root_modes = match &own.kind {
        SignatureKind::Function(f) => {
            let row = own.types.get(f.result).unwrap();
            let mut modes = row.modes.clone();
            modes.sendability = crossing_requirement(row);
            if let Some(arena) = &arenas.definition {
                modes.relation = modes.relation.retain(|r| {
                    !matches!(r, Reach::Place(o) if arena_parameter(&own.types, f, o, 0, arena))
                });
            }
            modes
        }
        _ => Modes::global(),
    };
    let mut todo = vec![(root, NameSite::Item(item), root_modes, false, vec![])];
    todo.extend(clauses.iter().map(|(root, _)| {
        (
            *root,
            NameSite::Expression(*root),
            Modes::global(),
            false,
            vec![],
        )
    }));
    let mut clause_scopes: BTreeMap<_, _> = clauses
        .iter()
        .map(|(root, _)| (NameSite::Expression(*root), *root))
        .collect();
    let mut graphs = vec![];
    while let Some((root, owner, mut result, scoped, captures)) = todo.pop() {
        result.relation = arenas.lifetime(&result.relation);
        let clause = clause_scopes.get(&owner).copied();
        let result_ty = results
            .iter()
            .find(|(site, _)| *site == owner)
            .map(|(_, ty)| *ty)
            .or_else(|| {
                let root = match &own.kind {
                    SignatureKind::Function(f) if owner == NameSite::Item(item) => f.result,
                    SignatureKind::Const(root) if owner == NameSite::Item(item) => *root,
                    _ => return None,
                };
                import_type(&own.types, root, &mut body.types)
                    .ok()
                    .and_then(|root| bind_parameter_scopes(&mut body.types, root, &[owner]).ok())
            });
        let result_shareable = if owner == NameSite::Item(item) {
            match &own.kind {
                SignatureKind::Function(f) => local_result_permissions(&own.types, f, owner),
                _ => BTreeSet::new(),
            }
        } else {
            callable_result_permissions(body, owner, rows)
        };
        let mut expressions = vec![];
        let mut lifetimes = LifetimeScopes::new();
        let mut nodes = vec![];
        let exit = node(
            &mut nodes,
            Op::Nop,
            vec![],
            tree.expressions[root.0 as usize].origin.span,
        );
        let ret = node(
            &mut nodes,
            Op::Return {
                value: root,
                result: result.clone(),
            },
            vec![exit],
            tree.expressions[root.0 as usize].origin.span,
        );
        let mut entry = reserve(&mut nodes, tree.expressions[root.0 as usize].origin.span);
        let env = Environment {
            owner: owner.clone(),
            breaks: None,
            scopes: vec![],
            lifetime: 0,
            result: result.clone(),
            consumer: None,
            statement: None,
        };
        // Places in the value position of a block that declares their binding:
        // the value leaves the block and the binding does not, so a read moves
        // an owned local out instead of borrowing storage about to end.
        let mut leaving: BTreeSet<ExprId> = BTreeSet::new();
        let mut ending: BTreeMap<ExprId, (ExprId, ExprId)> = BTreeMap::new();
        let mut statements = Statements::default();
        let mut jobs = vec![Job::Expr {
            entry,
            expression: root,
            context: if result.relation.is_global() {
                Context::Own
            } else {
                Context::ResultShare
            },
            operation: root,
            escaping: true,
            owning: false,
            next: ret,
            env: env.clone(),
        }];
        // H keeps snapshot children in the declaring frame, while the clause
        // reads only their plain saved results. Check those children in the
        // frame's entry state, before the body can weaken parameter modes.
        for snapshot in entry_snapshots(tree, owner).into_iter().rev() {
            let next = entry;
            entry = reserve(
                &mut nodes,
                tree.expressions[snapshot.0 as usize].origin.span,
            );
            jobs.push(Job::Expr {
                entry,
                expression: snapshot,
                context: Context::Read,
                operation: snapshot,
                escaping: false,
                owning: false,
                next,
                env: env.clone(),
            });
        }
        while let Some(job) = jobs.pop() {
            match job {
                Job::Expr {
                    entry,
                    expression,
                    context,
                    operation,
                    escaping,
                    owning,
                    next,
                    mut env,
                } => {
                    if operation != expression {
                        env.consumer = Some(operation);
                    }
                    expressions.push(expression);
                    lifetimes.expressions.insert(expression, env.lifetime);
                    if let Some(statement) = env.statement {
                        statements.temporaries.insert(expression, statement);
                    }
                    let expr = &tree.expressions[expression.0 as usize];
                    nodes[entry].span = expr.origin.span;
                    if place(body, expression, rows).is_some()
                        && !matches!(expr.kind, ExprKind::Unary { .. })
                    {
                        let context = if matches!(context, Context::Read | Context::View)
                            && leaving.contains(&expression)
                        {
                            Context::Own
                        } else {
                            context
                        };
                        nodes[entry].op = Op::Use {
                            expression,
                            context,
                            operation,
                        };
                        nodes[entry].next = vec![next];
                        // An index expression is evaluated before using the element place.
                        let indices: Vec<_> = body
                            .places
                            .iter()
                            .find(|p| p.expression == expression)
                            .expect("H source place")
                            .projections
                            .iter()
                            .rev()
                            .filter_map(|p| match p.kind {
                                SourceProjectionKind::Index(index) => Some(index),
                                _ => None,
                            })
                            .collect();
                        if !indices.is_empty() {
                            let use_op = nodes[entry].op.clone();
                            let use_node = node(&mut nodes, use_op, vec![next], expr.origin.span);
                            nodes[entry].op = Op::Nop;
                            let mut child_next = use_node;
                            for index in indices {
                                let child = reserve(
                                    &mut nodes,
                                    tree.expressions[index.0 as usize].origin.span,
                                );
                                jobs.push(Job::Expr {
                                    entry: child,
                                    expression: index,
                                    context: Context::Read,
                                    operation,
                                    escaping: false,
                                    owning: false,
                                    next: child_next,
                                    env: env.clone(),
                                });
                                child_next = child;
                            }
                            nodes[entry].next = vec![child_next];
                        }
                        continue;
                    }
                    let mut children: Vec<(ExprId, Context, bool, bool)> = vec![];
                    let op = match &expr.kind {
                        ExprKind::Unary { op, value } => {
                            let context = match body
                                .places
                                .iter()
                                .find(|p| p.expression == expression)
                                .expect("H source place")
                                .access
                            {
                                SourceAccess::Take => Context::Take,
                                SourceAccess::Copy => Context::Copy,
                                SourceAccess::Mut => Context::Mut,
                                SourceAccess::Unique => Context::Unique,
                                SourceAccess::Read => Context::Read,
                            };
                            let child_context = if *op == UnaryOp::Box {
                                Context::Store
                            } else {
                                context
                            };
                            children.push((*value, child_context, escaping, owning));
                            Op::Finish {
                                expression,
                                context,
                                children: vec![*value],
                                fresh: *op == UnaryOp::Box,
                            }
                        }
                        ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                            children.extend(values.iter().map(|v| {
                                (
                                    *v,
                                    if context == Context::ResultShare {
                                        context
                                    } else {
                                        Context::Store
                                    },
                                    true,
                                    false,
                                )
                            }));
                            Op::Finish {
                                expression,
                                context,
                                children: values.clone(),
                                fresh: true,
                            }
                        }
                        ExprKind::Binary {
                            op: BinaryOp::And | BinaryOp::Or,
                            left,
                            right,
                        } => {
                            let finish = node(
                                &mut nodes,
                                Op::Finish {
                                    expression,
                                    context,
                                    children: vec![*left],
                                    fresh: false,
                                },
                                vec![next],
                                expr.origin.span,
                            );
                            let right_finish = node(
                                &mut nodes,
                                Op::Finish {
                                    expression,
                                    context,
                                    children: vec![*left, *right],
                                    fresh: false,
                                },
                                vec![next],
                                expr.origin.span,
                            );
                            let right_entry = reserve(&mut nodes, expr.origin.span);
                            jobs.push(Job::Expr {
                                entry: right_entry,
                                expression: *right,
                                context: Context::Read,
                                operation,
                                escaping: false,
                                owning: false,
                                next: right_finish,
                                env: env.clone(),
                            });
                            let branch = node(
                                &mut nodes,
                                Op::Nop,
                                vec![finish, right_entry],
                                expr.origin.span,
                            );
                            jobs.push(Job::Expr {
                                entry,
                                expression: *left,
                                context: Context::Read,
                                operation,
                                escaping: false,
                                owning: false,
                                next: branch,
                                env,
                            });
                            continue;
                        }
                        ExprKind::Binary { left, right, .. } => {
                            children.extend([
                                (*left, Context::View, false, false),
                                (*right, Context::View, false, false),
                            ]);
                            Op::Finish {
                                expression,
                                context,
                                children: vec![*left, *right],
                                fresh: true,
                            }
                        }
                        ExprKind::Before(_)
                            if clauses
                                .iter()
                                .any(|(e, _)| owner == NameSite::Expression(*e)) =>
                        {
                            Op::Finish {
                                expression,
                                context,
                                children: vec![],
                                fresh: true,
                            }
                        }
                        ExprKind::Before(value) | ExprKind::Comptime(value) => {
                            children.push((*value, context, escaping, owning));
                            Op::Forward {
                                expression,
                                child: *value,
                            }
                        }
                        ExprKind::Field { base, .. } => {
                            children.push((*base, Context::Read, false, false));
                            Op::Finish {
                                expression,
                                context,
                                children: vec![*base],
                                fresh: false,
                            }
                        }
                        ExprKind::Index { base, index } => {
                            children.extend([
                                (*base, Context::Read, false, false),
                                (*index, Context::Read, false, false),
                            ]);
                            Op::Finish {
                                expression,
                                context,
                                children: vec![*base],
                                fresh: false,
                            }
                        }
                        ExprKind::Block {
                            statements: block_statements,
                            tail,
                        } => {
                            let patterns: Vec<_> = block_statements
                                .iter()
                                .filter_map(|s| match tree.statements[s.0 as usize].kind {
                                    StmtKind::Let { pattern, .. } => Some(pattern),
                                    _ => None,
                                })
                                .collect();
                            let mut locals = binders(rows, &patterns, tree);
                            let mut statement_bindings: Vec<_> = block_statements
                                .iter()
                                .flat_map(|s| rows.at(NameSite::Statement(*s)))
                                .collect();
                            statement_bindings.sort_unstable_by(|a, b| {
                                hirsel_intern::work::count(
                                    hirsel_intern::work::Counter::OwnershipSourceRows,
                                );
                                a.cmp(b)
                            });
                            statement_bindings.dedup();
                            locals.extend(statement_bindings);
                            env.lifetime = lifetimes.child(env.lifetime);
                            lifetimes
                                .bindings
                                .extend(locals.iter().map(|index| (*index, env.lifetime)));
                            let end = node(
                                &mut nodes,
                                Op::Kill(locals.clone()),
                                vec![next],
                                expr.origin.span,
                            );
                            let mut positions: Vec<ExprId> = tail.iter().copied().collect();
                            while let Some(position) = positions.pop() {
                                match &tree.expressions[position.0 as usize].kind {
                                    ExprKind::Block {
                                        tail: Some(inner), ..
                                    } => positions.push(*inner),
                                    ExprKind::If {
                                        then_branch,
                                        else_branch,
                                        ..
                                    } => {
                                        positions.push(*then_branch);
                                        positions.extend(else_branch);
                                    }
                                    ExprKind::Match { arms, .. } => {
                                        positions.extend(arms.iter().map(|arm| arm.body))
                                    }
                                    ExprKind::Unary { .. } => {}
                                    _ => {
                                        let local = rows
                                            .source_place(position)
                                            .and_then(|_| place(body, position, rows))
                                            .filter(|p| locals.contains(&p.binding));
                                        match local {
                                            Some(p) if p.projections.is_empty() => {
                                                leaving.insert(position);
                                            }
                                            Some(_) => {
                                                ending.insert(
                                                    position,
                                                    (expression, env.consumer.unwrap_or(operation)),
                                                );
                                            }
                                            None => {}
                                        }
                                    }
                                }
                            }
                            env.scopes.push(locals);
                            let mut current = if let Some(tail) = tail {
                                let forward = node(
                                    &mut nodes,
                                    Op::Forward {
                                        expression,
                                        child: *tail,
                                    },
                                    vec![end],
                                    expr.origin.span,
                                );
                                let child = reserve(&mut nodes, expr.origin.span);
                                jobs.push(Job::Expr {
                                    entry: child,
                                    expression: *tail,
                                    context,
                                    operation: *tail,
                                    escaping,
                                    owning,
                                    next: forward,
                                    env: env.clone(),
                                });
                                child
                            } else {
                                node(
                                    &mut nodes,
                                    Op::Finish {
                                        expression,
                                        context,
                                        children: vec![],
                                        fresh: true,
                                    },
                                    vec![end],
                                    expr.origin.span,
                                )
                            };
                            for statement in block_statements.iter().rev() {
                                let span = tree.statements[statement.0 as usize].origin.span;
                                let child = reserve(&mut nodes, span);
                                let end = node(
                                    &mut nodes,
                                    Op::StatementEnd(*statement),
                                    vec![current],
                                    span,
                                );
                                if let Some(parent) = env.statement {
                                    statements.parents.insert(*statement, parent);
                                }
                                let mut env = env.clone();
                                env.statement = Some(*statement);
                                jobs.push(Job::Stmt {
                                    entry: child,
                                    statement: *statement,
                                    next: end,
                                    env,
                                });
                                current = child;
                            }
                            nodes[entry].next = vec![current];
                            continue;
                        }
                        ExprKind::If {
                            condition,
                            then_branch,
                            else_branch,
                        } => {
                            let join = node(
                                &mut nodes,
                                Op::IdentityReady(NameSite::Expression(expression)),
                                vec![next],
                                expr.origin.span,
                            );
                            let mut branches = vec![];
                            for branch in std::iter::once(Some(*then_branch))
                                .chain(std::iter::once(*else_branch))
                            {
                                if let Some(branch) = branch {
                                    let finish = node(
                                        &mut nodes,
                                        Op::Forward {
                                            expression,
                                            child: branch,
                                        },
                                        vec![join],
                                        expr.origin.span,
                                    );
                                    let child = reserve(&mut nodes, expr.origin.span);
                                    jobs.push(Job::Expr {
                                        entry: child,
                                        expression: branch,
                                        context,
                                        operation: branch,
                                        escaping,
                                        owning,
                                        next: finish,
                                        env: env.clone(),
                                    });
                                    branches.push(child);
                                } else {
                                    branches.push(node(
                                        &mut nodes,
                                        Op::Finish {
                                            expression,
                                            context,
                                            children: vec![],
                                            fresh: true,
                                        },
                                        vec![join],
                                        expr.origin.span,
                                    ));
                                }
                            }
                            let branch = node(&mut nodes, Op::Nop, branches, expr.origin.span);
                            jobs.push(Job::Expr {
                                entry,
                                expression: *condition,
                                context: Context::Read,
                                operation: *condition,
                                escaping: false,
                                owning: false,
                                next: branch,
                                env,
                            });
                            continue;
                        }
                        ExprKind::Match { value, arms } => {
                            let join = node(
                                &mut nodes,
                                Op::IdentityReady(NameSite::Expression(expression)),
                                vec![next],
                                expr.origin.span,
                            );
                            let mut residual = exit;
                            for arm in arms.iter().rev() {
                                let bindings = binders(rows, &[arm.pattern], tree);
                                let kill = node(
                                    &mut nodes,
                                    Op::Kill(bindings.clone()),
                                    vec![join],
                                    expr.origin.span,
                                );
                                let finish = node(
                                    &mut nodes,
                                    Op::Forward {
                                        expression,
                                        child: arm.body,
                                    },
                                    vec![kill],
                                    expr.origin.span,
                                );
                                let arm_entry = reserve(&mut nodes, expr.origin.span);
                                let mut arm_env = env.clone();
                                arm_env.scopes.push(bindings.clone());
                                arm_env.lifetime = lifetimes.child(env.lifetime);
                                lifetimes.bindings.extend(
                                    bindings.iter().map(|index| (*index, arm_env.lifetime)),
                                );
                                jobs.push(Job::Expr {
                                    entry: arm_entry,
                                    expression: arm.body,
                                    context,
                                    operation: arm.body,
                                    escaping,
                                    owning,
                                    next: finish,
                                    env: arm_env.clone(),
                                });
                                let commit = node(
                                    &mut nodes,
                                    Op::Bind {
                                        pattern: arm.pattern,
                                        value: *value,
                                        guarded: arm.guard.is_some(),
                                        test: false,
                                    },
                                    vec![arm_entry],
                                    expr.origin.span,
                                );
                                let selected = if let Some(guard) = arm.guard {
                                    let reject = node(
                                        &mut nodes,
                                        Op::Kill(bindings),
                                        vec![residual],
                                        expr.origin.span,
                                    );
                                    let branch = node(
                                        &mut nodes,
                                        Op::Nop,
                                        vec![commit, reject],
                                        expr.origin.span,
                                    );
                                    let guard_entry = reserve(&mut nodes, expr.origin.span);
                                    jobs.push(Job::Expr {
                                        entry: guard_entry,
                                        expression: guard,
                                        context: Context::Read,
                                        operation: guard,
                                        escaping: false,
                                        owning: false,
                                        next: branch,
                                        env: arm_env,
                                    });
                                    node(
                                        &mut nodes,
                                        Op::Bind {
                                            pattern: arm.pattern,
                                            value: *value,
                                            guarded: true,
                                            test: true,
                                        },
                                        vec![guard_entry],
                                        expr.origin.span,
                                    )
                                } else {
                                    commit
                                };
                                residual = node(
                                    &mut nodes,
                                    Op::Nop,
                                    vec![selected, residual],
                                    expr.origin.span,
                                );
                            }
                            jobs.push(Job::Expr {
                                entry,
                                expression: *value,
                                context: Context::Read,
                                operation: *value,
                                escaping: false,
                                owning: false,
                                next: residual,
                                env,
                            });
                            continue;
                        }
                        ExprKind::Try { value, .. } => {
                            let finish = node(
                                &mut nodes,
                                Op::Forward {
                                    expression,
                                    child: *value,
                                },
                                vec![next],
                                expr.origin.span,
                            );
                            let kill = node(
                                &mut nodes,
                                Op::Kill(env.scopes.iter().flatten().copied().collect()),
                                vec![exit],
                                expr.origin.span,
                            );
                            let branch =
                                node(&mut nodes, Op::Nop, vec![finish, kill], expr.origin.span);
                            jobs.push(Job::Expr {
                                entry,
                                expression: *value,
                                context: Context::Store,
                                operation,
                                escaping,
                                owning,
                                next: branch,
                                env,
                            });
                            continue;
                        }
                        ExprKind::Return(value) => {
                            let kill = node(
                                &mut nodes,
                                Op::Kill(env.scopes.iter().flatten().copied().collect()),
                                vec![exit],
                                expr.origin.span,
                            );
                            if let Some(value) = value {
                                let ret = node(
                                    &mut nodes,
                                    Op::Return {
                                        value: *value,
                                        result: env.result.clone(),
                                    },
                                    vec![kill],
                                    expr.origin.span,
                                );
                                jobs.push(Job::Expr {
                                    entry,
                                    expression: *value,
                                    context: if env.result.relation.is_global() {
                                        Context::Own
                                    } else {
                                        Context::ResultShare
                                    },
                                    operation: *value,
                                    escaping: true,
                                    owning: false,
                                    next: ret,
                                    env,
                                });
                            } else {
                                nodes[entry].op = Op::ReturnUnit;
                                nodes[entry].next = vec![kill];
                            }
                            continue;
                        }
                        ExprKind::Break | ExprKind::Continue => {
                            if let Some((break_to, continue_to, depth)) = env.breaks {
                                nodes[entry].op = Op::Kill(
                                    env.scopes.iter().skip(depth).flatten().copied().collect(),
                                );
                                nodes[entry].next = vec![if matches!(expr.kind, ExprKind::Break) {
                                    break_to
                                } else {
                                    continue_to
                                }];
                            }
                            continue;
                        }
                        ExprKind::Closure {
                            body: closure_body, ..
                        } => {
                            let captures = rows
                                .closure(expression)
                                .map(|i| &body.closures[i])
                                .map(|c| c.captures.clone())
                                .unwrap_or_default();
                            // The body is checked against its frame result; an
                            // inferred local result may be specialised on the
                            // closure's type by its captured sources (CLO-1).
                            let frame = results
                                .iter()
                                .find(|(site, _)| *site == NameSite::Expression(expression))
                                .and_then(|(_, ty)| body.types.get(*ty));
                            let result = frame
                                .or_else(|| {
                                    expression_type(body, expression, rows)
                                        .and_then(|ty| body.types.get(ty))
                                        .and_then(|ty| match &ty.kind {
                                            TypeKind::Function(f) => body.types.get(f.result),
                                            _ => None,
                                        })
                                })
                                .map_or_else(Modes::global, |t| {
                                    let mut modes = t.modes.clone();
                                    modes.sendability = crossing_requirement(t);
                                    modes
                                });
                            let context = capture_modes.first(expression).map(|c| c.context);
                            let escaping = context.map_or(escaping, |c| {
                                matches!(c, CaptureContext::Escaping | CaptureContext::Task)
                            });
                            let owning =
                                context.map_or(owning, |c| c == CaptureContext::OwningScoped);
                            let captures = captures
                                .into_iter()
                                .enumerate()
                                .map(|(ordinal, capture)| {
                                    let row = capture_modes.get(expression, ordinal as u32);
                                    (
                                        row.map_or_else(|| env.owner.clone(), |r| r.owner.clone()),
                                        capture.binding,
                                        ordinal,
                                    )
                                })
                                .collect();
                            if let Some(root) = clause {
                                clause_scopes.insert(NameSite::Expression(expression), root);
                            }
                            todo.push((
                                *closure_body,
                                NameSite::Expression(expression),
                                result,
                                !escaping,
                                captures,
                            ));
                            Op::Capture {
                                expression,
                                escaping,
                                owning,
                            }
                        }
                        ExprKind::Call { callee, arguments } => {
                            let call =
                                rows.call(expression)
                                    .map(|i| &body.calls[i])
                                    .ok_or_else(|| {
                                        QueryError::Operational(OperationalFailure {
                                            operation: "ownership checked call".into(),
                                            message: "missing H call mapping".into(),
                                        })
                                    })?;
                            let constructor = matches!(call.target, CallTarget::Constructor(_));
                            // The callee's own Arena parameters place its result
                            // and parameters; flow tracks lifetimes (PROV-4).
                            let arena_parameters: Vec<u32> = call
                                .arguments
                                .iter()
                                .filter_map(|a| a.parameter.as_ref())
                                .filter(|(_, p)| {
                                    arenas.definition.as_ref().is_some_and(|arena| {
                                        is_arena_type(&body.types, p.ty, arena)
                                    })
                                })
                                .map(|(ordinal, _)| *ordinal)
                                .collect();
                            let lifetime = |relation: &Relation| {
                                arenas.lifetime(relation).retain(|r| {
                                    !matches!(r, Reach::Place(hirsel_types::Origin {
                                        root: OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter }),
                                        fields,
                                    }) if fields.is_empty() && arena_parameters.contains(&parameter.0))
                                })
                            };
                            let mut result = body.types.get(call.result).unwrap().modes.clone();
                            result.relation = lifetime(&result.relation);
                            let requirements: Vec<_> = call
                                .arguments
                                .iter()
                                .map(|a| {
                                    if call.state != CallState::Complete || constructor {
                                        return None;
                                    }
                                    a.parameter.as_ref().map(|(ordinal, p)| Requirement {
                                        ty: p.ty,
                                        parameter: *ordinal as usize,
                                        passing: p.passing,
                                        modes: {
                                            let mut modes =
                                                body.types.get(p.ty).unwrap().modes.clone();
                                            modes.sendability =
                                                crossing_requirement(body.types.get(p.ty).unwrap());
                                            modes.relation = lifetime(&modes.relation);
                                            modes
                                        },
                                        sendable_bound: sendable_parameter(
                                            body, *callee, *ordinal, rows,
                                        ),
                                    })
                                })
                                .collect();
                            // J's instantiated result includes construction-site
                            // sendability (notably erased iterator state). The
                            // declaration alone cannot recover that evidence;
                            // flow derives it again from `captures`.
                            if let Some(ty) = expression_type(body, expression, rows) {
                                result.sendability =
                                    body.types.get(ty).unwrap().modes.sendability.clone();
                            }
                            children.push((*callee, Context::Invoke, false, false));
                            for (index, arg) in arguments.iter().enumerate() {
                                let req = requirements.get(index).and_then(Option::as_ref);
                                children.push((
                                    arg.value,
                                    if constructor {
                                        if context == Context::ResultShare {
                                            context
                                        } else {
                                            Context::Store
                                        }
                                    } else if req.is_some_and(|r| r.passing == PassingMode::Borrow)
                                    {
                                        Context::View
                                    } else {
                                        Context::Read
                                    },
                                    constructor
                                        || req.is_some_and(|r| r.modes.relation.is_global()),
                                    req.is_some_and(|r| r.passing == PassingMode::Take),
                                ));
                            }
                            let parts = if constructor {
                                call.arguments
                                    .iter()
                                    .map(|a| a.parameter.as_ref().map(|(_, p)| p.ty))
                                    .collect()
                            } else {
                                vec![]
                            };
                            let captures =
                                match &body.types.get(call.result).unwrap().erased_captures {
                                    Some(ErasedCaptures::FromArguments(parameters))
                                        if call.state == CallState::Complete && !constructor =>
                                    {
                                        parameters
                                            .iter()
                                            .map(|p| match p.parameter {
                                                ParameterRef::Bound {
                                                    depth: 0,
                                                    parameter,
                                                } if p.fields.is_empty() => {
                                                    call.arguments.iter().position(|a| {
                                                        a.parameter
                                                            .as_ref()
                                                            .is_some_and(|(o, _)| *o == parameter.0)
                                                    })
                                                }
                                                _ => None,
                                            })
                                            .collect()
                                    }
                                    _ => None,
                                };
                            Op::Call {
                                expression,
                                callee: *callee,
                                arguments: arguments.clone(),
                                requirements,
                                result,
                                constructor,
                                parts,
                                captures,
                            }
                        }
                        _ => Op::Finish {
                            expression,
                            context,
                            children: vec![],
                            fresh: matches!(expr.kind, ExprKind::Literal(_) | ExprKind::Heap),
                        },
                    };
                    if children.is_empty() {
                        nodes[entry].op = op;
                        nodes[entry].next = vec![next];
                    } else {
                        let lends = matches!(
                            op,
                            Op::Call {
                                constructor: false,
                                ..
                            }
                        );
                        let finish = node(&mut nodes, op, vec![next], expr.origin.span);
                        let mut current = finish;
                        let last = children.len() - 1;
                        for (index, (child, ctx, esc, own_capture)) in
                            children.into_iter().enumerate().rev()
                        {
                            // The callee is child zero; the last argument is
                            // followed by the call itself.
                            if lends && index != 0 && index != last {
                                current = node(
                                    &mut nodes,
                                    Op::ArgumentLoan {
                                        argument: child,
                                        call: expression,
                                    },
                                    vec![current],
                                    tree.expressions[child.0 as usize].origin.span,
                                );
                            }
                            let child_entry =
                                reserve(&mut nodes, tree.expressions[child.0 as usize].origin.span);
                            jobs.push(Job::Expr {
                                entry: child_entry,
                                expression: child,
                                context: ctx,
                                operation: if ctx == Context::Unique
                                    || matches!(expr.kind, ExprKind::Call { .. })
                                {
                                    expression
                                } else {
                                    operation
                                },
                                escaping: esc,
                                owning: own_capture,
                                next: current,
                                env: env.clone(),
                            });
                            current = child_entry;
                        }
                        nodes[entry].next = vec![current];
                    }
                }
                Job::Stmt {
                    entry,
                    statement,
                    next,
                    mut env,
                } => {
                    let stmt = &tree.statements[statement.0 as usize];
                    match &stmt.kind {
                        StmtKind::Expr(expression) => jobs.push(Job::Expr {
                            entry,
                            expression: *expression,
                            context: Context::Read,
                            operation: *expression,
                            escaping: false,
                            owning: false,
                            next,
                            env,
                        }),
                        StmtKind::Let {
                            pattern, value, ty, ..
                        } => {
                            let bind = node(
                                &mut nodes,
                                Op::Bind {
                                    pattern: *pattern,
                                    value: *value,
                                    guarded: false,
                                    test: false,
                                },
                                vec![next],
                                stmt.origin.span,
                            );
                            jobs.push(Job::Expr {
                                entry,
                                expression: *value,
                                context: Context::Own,
                                operation: *value,
                                escaping: ty.is_some(),
                                owning: false,
                                next: bind,
                                env,
                            });
                        }
                        StmtKind::Assign { place, value, op } => {
                            let write = node(
                                &mut nodes,
                                Op::Write {
                                    place: *place,
                                    value: *value,
                                    operation: stmt.origin.span,
                                    compound: op.0.is_some(),
                                },
                                vec![next],
                                stmt.origin.span,
                            );
                            let rhs = reserve(&mut nodes, stmt.origin.span);
                            jobs.push(Job::Expr {
                                entry: rhs,
                                expression: *value,
                                context: Context::Store,
                                operation: *value,
                                escaping: true,
                                owning: false,
                                next: write,
                                env: env.clone(),
                            });
                            jobs.push(Job::Expr {
                                entry,
                                expression: *place,
                                context: Context::Address,
                                operation: *place,
                                escaping: false,
                                owning: false,
                                next: rhs,
                                env,
                            });
                        }
                        StmtKind::While {
                            condition,
                            body: loop_body,
                        } => {
                            env.lifetime = lifetimes.child(env.lifetime);
                            let condition_entry = reserve(&mut nodes, stmt.origin.span);
                            nodes[entry].op = Op::IdentityReady(NameSite::Statement(statement));
                            nodes[entry].next = vec![condition_entry];
                            let branch = node(&mut nodes, Op::Nop, vec![], stmt.origin.span);
                            let loop_entry = reserve(&mut nodes, stmt.origin.span);
                            env.breaks = Some((next, entry, env.scopes.len()));
                            jobs.push(Job::Expr {
                                entry: loop_entry,
                                expression: *loop_body,
                                context: Context::Read,
                                operation: *loop_body,
                                escaping: false,
                                owning: false,
                                next: entry,
                                env: env.clone(),
                            });
                            nodes[branch].next = vec![loop_entry, next];
                            jobs.push(Job::Expr {
                                entry: condition_entry,
                                expression: *condition,
                                context: Context::Read,
                                operation: *condition,
                                escaping: false,
                                owning: false,
                                next: branch,
                                env,
                            });
                        }
                        StmtKind::For {
                            pattern,
                            source,
                            body: loop_body,
                        } => {
                            // Collection preparation outlives individual yielded items.
                            env.lifetime = lifetimes.child(env.lifetime);
                            let preparation = env.clone();
                            env.lifetime = lifetimes.child(env.lifetime);
                            lifetimes.bindings.extend(
                                binders(rows, &[*pattern], tree)
                                    .into_iter()
                                    .map(|index| (index, env.lifetime)),
                            );
                            // A borrowed loop over a place reads its source at every
                            // advance (SEM-4), so the source stays lent while the
                            // loop may advance again, whatever the body uses.
                            let borrowed_place = !matches!(
                                tree.expressions[source.0 as usize].kind,
                                ExprKind::Unary {
                                    op: UnaryOp::Take | UnaryOp::Mut,
                                    ..
                                }
                            ) && rows.source_place(*source).is_some()
                                && body
                                    .expressions
                                    .iter()
                                    .find(|e| e.expression == *source)
                                    .is_some_and(|e| {
                                        !matches!(
                                            body.types.get(e.ty).unwrap().kind,
                                            TypeKind::Builtin {
                                                kind: BuiltinType::Iter,
                                                ..
                                            }
                                        )
                                    });
                            let loop_exit = if borrowed_place {
                                node(
                                    &mut nodes,
                                    Op::IterationLoan {
                                        source: *source,
                                        begin: false,
                                    },
                                    vec![next],
                                    stmt.origin.span,
                                )
                            } else {
                                next
                            };
                            let header = node(
                                &mut nodes,
                                Op::IdentityReady(NameSite::Statement(statement)),
                                vec![],
                                stmt.origin.span,
                            );
                            let kill = node(
                                &mut nodes,
                                Op::Kill(binders(rows, &[*pattern], tree)),
                                vec![header],
                                stmt.origin.span,
                            );
                            let loop_entry = reserve(&mut nodes, stmt.origin.span);
                            env.breaks = Some((loop_exit, kill, env.scopes.len()));
                            jobs.push(Job::Expr {
                                entry: loop_entry,
                                expression: *loop_body,
                                context: Context::Read,
                                operation: *loop_body,
                                escaping: false,
                                owning: false,
                                next: kill,
                                env: env.clone(),
                            });
                            let bind = node(
                                &mut nodes,
                                Op::Bind {
                                    pattern: *pattern,
                                    value: *source,
                                    guarded: false,
                                    test: false,
                                },
                                vec![loop_entry],
                                stmt.origin.span,
                            );
                            nodes[header].next = vec![bind, loop_exit];
                            let ready = if borrowed_place {
                                node(
                                    &mut nodes,
                                    Op::IterationLoan {
                                        source: *source,
                                        begin: true,
                                    },
                                    vec![header],
                                    stmt.origin.span,
                                )
                            } else {
                                header
                            };
                            jobs.push(Job::Expr {
                                entry,
                                expression: *source,
                                context: Context::Read,
                                operation: *source,
                                escaping: false,
                                owning: false,
                                next: ready,
                                env: preparation,
                            });
                        }
                        StmtKind::Function { body: fn_body, .. } => {
                            let closure = rows.closure(*fn_body).map(|i| &body.closures[i]);
                            let result = closure
                                .and_then(|c| body.types.get(c.ty))
                                .and_then(|t| match &t.kind {
                                    TypeKind::Function(f) => body.types.get(f.result),
                                    _ => None,
                                })
                                .map_or_else(Modes::global, |t| {
                                    let mut modes = t.modes.clone();
                                    modes.sendability = crossing_requirement(t);
                                    modes
                                });
                            let captures = closure
                                .map(|c| {
                                    c.captures
                                        .iter()
                                        .enumerate()
                                        .map(|(ordinal, capture)| {
                                            let row = capture_modes.get(*fn_body, ordinal as u32);
                                            (
                                                row.map_or_else(
                                                    || env.owner.clone(),
                                                    |r| r.owner.clone(),
                                                ),
                                                capture.binding.clone(),
                                                ordinal,
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            if let Some(root) = clause {
                                clause_scopes.insert(NameSite::Statement(statement), root);
                            }
                            todo.push((
                                *fn_body,
                                NameSite::Statement(statement),
                                result,
                                true,
                                captures,
                            ));
                            nodes[entry].op = Op::LocalFunction(statement);
                            nodes[entry].next = vec![next];
                        }
                    }
                }
            }
        }
        graphs.push(Graph {
            nodes,
            entry,
            owner,
            result_shareable,
            result,
            result_ty,
            expressions,
            scoped,
            clause,
            captures,
            capture_modes: capture_modes.clone(),
            lifetimes: std::sync::Arc::new(lifetimes),
            leaving,
            ending,
            statements,
        });
    }
    Ok(graphs)
}

fn entry_snapshots(tree: &SyntaxTree, owner: NameSite) -> Vec<ExprId> {
    let signature = match owner {
        NameSite::Item(id) => match &tree.items[id.0 as usize].kind {
            ItemKind::Function { signature, .. } => Some(signature),
            _ => None,
        },
        NameSite::Statement(id) => match &tree.statements[id.0 as usize].kind {
            StmtKind::Function { signature, .. } => Some(signature),
            _ => None,
        },
        NameSite::Expression(id) => match &tree.expressions[id.0 as usize].kind {
            ExprKind::Closure { signature, .. } => Some(signature),
            _ => None,
        },
        _ => None,
    };
    let mut pending: Vec<_> = signature
        .into_iter()
        .flat_map(|s| &s.contracts)
        .map(|contract| contract.expression)
        .collect();
    let mut seen = BTreeSet::new();
    let mut snapshots = vec![];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        match &tree.expressions[id.0 as usize].kind {
            ExprKind::Before(value) => snapshots.push(*value),
            ExprKind::Tuple(values) | ExprKind::Sequence(values) => pending.extend(values),
            ExprKind::Call { callee, arguments } => {
                pending.push(*callee);
                pending.extend(arguments.iter().map(|a| a.value));
            }
            ExprKind::Field { base, .. } => pending.push(*base),
            ExprKind::Index { base, index } => pending.extend([*base, *index]),
            ExprKind::Unary { value, .. }
            | ExprKind::Try { value, .. }
            | ExprKind::Comptime(value) => pending.push(*value),
            ExprKind::Binary { left, right, .. } => pending.extend([*left, *right]),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                pending.extend([*condition, *then_branch]);
                pending.extend(else_branch);
            }
            ExprKind::Match { value, arms } => {
                pending.push(*value);
                pending.extend(
                    arms.iter()
                        .flat_map(|a| a.guard.iter().chain(std::iter::once(&a.body))),
                );
            }
            ExprKind::Return(value) => pending.extend(value),
            ExprKind::Block { statements, tail } => {
                pending.extend(tail);
                for statement in statements {
                    match &tree.statements[statement.0 as usize].kind {
                        StmtKind::Let { value, .. } | StmtKind::Expr(value) => pending.push(*value),
                        StmtKind::Assign { place, value, .. } => pending.extend([*place, *value]),
                        StmtKind::While { condition, body } => pending.extend([*condition, *body]),
                        StmtKind::For { source, body, .. } => pending.extend([*source, *body]),
                        _ => (),
                    }
                }
            }
            _ => (), // Nested callable bodies have their own entry snapshots.
        }
    }
    snapshots.sort_by_key(|id| tree.expressions[id.0 as usize].origin.span.start);
    snapshots
}
// J proves structural bounds before I discovers sharing. A bare sendable
// parameter also needs the argument's current root evidence at the call.
fn sendable_parameter(
    body: &CheckedBody,
    callee: ExprId,
    ordinal: u32,
    rows: &super::source_rows::SourceRows,
) -> bool {
    let Some(TypeKind::Function(function)) = expression_type(body, callee, rows)
        .and_then(|ty| body.types.get(ty))
        .map(|ty| &ty.kind)
    else {
        return false;
    };
    let Some(TypeKind::Parameter(TypeParameterRef::Bound {
        depth: 0,
        parameter,
    })) = function
        .parameters
        .get(ordinal as usize)
        .and_then(|parameter| body.types.get(parameter.ty))
        .map(|ty| &ty.kind)
    else {
        return false;
    };
    matches!(function.generics.get(parameter.0 as usize),
        Some(GenericParameter::Type { bounds, .. }) if bounds.contains(&Bound::Sendable))
}

fn callable_result_permissions(
    body: &CheckedBody,
    owner: NameSite,
    rows: &super::source_rows::SourceRows,
) -> BTreeSet<TypeParameterRef> {
    let ty = match owner {
        NameSite::Expression(e) => expression_type(body, e, rows),
        _ => rows
            .at(owner)
            .map(|i| &body.bindings[i])
            .find(|b| matches!(b.binding, ResolvedName::Local(_)))
            .map(|b| b.ty),
    };
    match ty.and_then(|ty| body.types.get(ty)).map(|ty| &ty.kind) {
        Some(TypeKind::Function(f)) => local_result_permissions(&body.types, f, owner),
        _ => BTreeSet::new(),
    }
}

pub(super) fn expression_type(
    body: &CheckedBody,
    expression: ExprId,
    rows: &super::source_rows::SourceRows,
) -> Option<TypeId> {
    rows.expression(expression).map(|i| body.expressions[i].ty)
}

/// All checked ordinary operands, including contracts and finalizers outside the CFG.
pub(super) fn call_arguments(
    body: &CheckedBody,
    properties: &[super::properties::Properties],
    rows: &super::source_rows::SourceRows,
) -> Vec<(ExprId, ExprId, UseKind)> {
    body.calls
        .iter()
        .filter(|c| {
            !matches!(c.target, CallTarget::Constructor(_)) && c.state == CallState::Complete
        })
        .flat_map(|call| {
            call.arguments.iter().filter_map(|argument| {
                let (_, parameter) = argument.parameter.as_ref()?;
                let plain = expression_type(body, argument.expression, rows)
                    .and_then(|ty| properties.get(ty.index()))
                    .is_some_and(|p| p.plain);
                let usage = match parameter.passing {
                    PassingMode::Borrow if plain => UseKind::Copy,
                    PassingMode::Borrow => UseKind::Borrow,
                    PassingMode::Take => UseKind::Take,
                    PassingMode::Mut => UseKind::Mut,
                };
                Some((call.expression, argument.expression, usage))
            })
        })
        .collect()
}
