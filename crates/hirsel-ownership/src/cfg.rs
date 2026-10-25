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
    Replacement(usize),
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
    /// The failure edge of `value ? mapper` hands the error payload to
    /// `mapper` (`expression` is the propagation).
    MapError {
        expression: ExprId,
        value: ExprId,
    },
    /// Bind `pattern` from `value`, taking the alternatives `choice` names.
    Bind {
        pattern: PatternId,
        value: ExprId,
        guarded: bool,
        test: bool,
        choice: std::sync::Arc<PatternChoice>,
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
    /// The failure edge of `value ?` returns the (mapped) error payload of
    /// `value` as `return Error(e)` would (`expression` is the propagation).
    Propagate {
        expression: ExprId,
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
    /// Each labelled block's break operands: with its tail, the value
    /// positions of the block (LANG-11).
    pub block_breaks: BTreeMap<ExprId, Vec<ExprId>>,
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
    /// Active lexical loops: break/continue targets, scope depth and statement.
    breaks: Vec<(usize, usize, usize, StmtId)>,
    scopes: Vec<Vec<usize>>,
    lifetime: usize,
    result: Modes,
    /// The nearest operation that consumes this expression's value through
    /// transparent blocks, branches and arms (LANG-11).
    consumer: Option<ExprId>,
    /// The block statement whose temporaries this expression's value joins.
    statement: Option<StmtId>,
    /// Enclosing labelled blocks, innermost last (LANG-11).
    blocks: Vec<BlockTarget>,
}
/// A labelled block a break can leave: the break's operand is evaluated as
/// the block's tail is, then the exited scopes end before the join (LANG-11).
#[derive(Clone)]
struct BlockTarget {
    label: hirsel_intern::Symbol,
    block: ExprId,
    join: usize,
    /// The scope depth outside the block and the statement holding it.
    depth: usize,
    statement: Option<StmtId>,
    context: Context,
    escaping: bool,
    owning: bool,
    consumer: Option<ExprId>,
    operation: ExprId,
}
/// The places in the value positions of `value` (through transparent block
/// tails, branches, arms and a labelled block's breaks) that name one of
/// `locals`, with whether the place is the whole binding (LANG-11).
fn local_value_places(
    tree: &SyntaxTree,
    body: &CheckedBody,
    rows: &super::source_rows::SourceRows,
    block_breaks: &BTreeMap<ExprId, Vec<ExprId>>,
    value: ExprId,
    locals: &[usize],
) -> Vec<(ExprId, bool)> {
    let mut found = vec![];
    let mut positions = vec![value];
    while let Some(position) = positions.pop() {
        match &tree.expressions[position.0 as usize].kind {
            ExprKind::Block { tail, .. } => {
                positions.extend(tail);
                positions.extend(block_breaks.get(&position).into_iter().flatten());
            }
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                positions.push(*then_branch);
                positions.extend(else_branch);
            }
            ExprKind::Match { arms, .. } => positions.extend(arms.iter().map(|arm| arm.body)),
            ExprKind::Unary { .. } => {}
            _ => {
                if let Some(p) = rows
                    .source_place(position)
                    .and_then(|_| place(body, position, rows))
                    .filter(|p| locals.contains(&p.binding))
                {
                    found.push((position, p.projections.is_empty()));
                }
            }
        }
    }
    found
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
/// An early exit (`break`, `continue`, `return`, `?`) ends every statement it
/// leaves, innermost first, up to `until` (a loop the exit stays in), so their
/// temporaries are released on that edge too (MODE-2).
/// Bind nodes for every way `pattern`'s alternatives match, each continuing
/// at `next`; one node when it has no alternatives (LANG-13).
#[allow(clippy::too_many_arguments)]
fn bind(
    nodes: &mut Vec<Node>,
    tree: &SyntaxTree,
    pattern: PatternId,
    value: ExprId,
    guarded: bool,
    test: bool,
    next: usize,
    span: Span,
) -> usize {
    let choices = tree
        .pattern_choices(pattern)
        .unwrap_or_else(|| vec![PatternChoice::new()]);
    let binds: Vec<usize> = choices
        .into_iter()
        .map(|choice| {
            node(
                nodes,
                Op::Bind {
                    pattern,
                    value,
                    guarded,
                    test,
                    choice: std::sync::Arc::new(choice),
                },
                vec![next],
                span,
            )
        })
        .collect();
    if let [one] = binds[..] {
        one
    } else {
        node(nodes, Op::Nop, binds, span)
    }
}
fn exit_statements(
    nodes: &mut Vec<Node>,
    statements: &Statements,
    from: Option<StmtId>,
    until: Option<StmtId>,
    next: usize,
    span: Span,
) -> usize {
    let mut left = vec![];
    let mut current = from;
    while let Some(statement) = current.filter(|s| Some(*s) != until) {
        left.push(statement);
        current = statements.parents.get(&statement).copied();
    }
    left.into_iter().rev().fold(next, |next, statement| {
        node(nodes, Op::StatementEnd(statement), vec![next], span)
    })
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
        todo.extend(tree.pattern_children(pattern));
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
    t: &TypeInterner,
    syntax: &ItemSyntax,
    own: &Signature,
    body: &mut CheckedBody,
    capture_modes: &[CaptureMode],
    clauses: &[(ExprId, TypeId)],
    results: &[(NameSite, TypeId)],
    rows: &super::source_rows::SourceRows,
) -> Result<Vec<Graph>, QueryError> {
    let tree = &syntax.tree;
    let lexical = syntax.owner();
    let root = match &syntax.item.kind {
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
    let arenas = crate::arenas::Arenas::new(t, body, own);
    let root_modes = match &own.kind {
        SignatureKind::Function(f) => {
            let row = t.get(f.result).unwrap();
            let mut modes = row.modes.clone();
            modes.sendability = crossing_requirement(row);
            if let Some(arena) = &arenas.definition {
                modes.relation = modes.relation.retain(
                    |r| !matches!(r, Reach::Place(o) if arena_parameter(t, f, o, 0, arena)),
                );
            }
            modes
        }
        _ => Modes::global(),
    };
    let mut todo = vec![(root, NameSite::Item, root_modes, false, vec![])];
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
                    SignatureKind::Function(f) if owner == NameSite::Item => f.result,
                    SignatureKind::Const { ty: root, .. } if owner == NameSite::Item => *root,
                    _ => return None,
                };
                bind_parameter_scopes(t, root, &[lexical.site(owner)]).ok()
            });
        let result_shareable = if owner == NameSite::Item {
            match &own.kind {
                SignatureKind::Function(f) => local_result_permissions(t, f, lexical.site(owner)),
                _ => BTreeSet::new(),
            }
        } else {
            callable_result_permissions(t, body, owner, rows)
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
        let mut block_breaks: BTreeMap<ExprId, Vec<ExprId>> = BTreeMap::new();
        for (exit, block) in tree.block_exits(root) {
            if let ExprKind::Break {
                value: Some(value), ..
            } = tree.expressions[exit.0 as usize].kind
            {
                block_breaks.entry(block).or_default().push(value);
            }
        }
        let env = Environment {
            owner: owner.clone(),
            blocks: vec![],
            breaks: vec![],
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
        // A bare nullary variant is a fresh construction, as a literal is.
        let nullary: BTreeSet<ExprId> = body
            .resolution
            .contextual
            .iter()
            .filter_map(|(site, name)| match (site, name) {
                (NameSite::Expression(id), ContextualName::Variant { .. }) => Some(*id),
                _ => None,
            })
            .collect();
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
        for snapshot in entry_snapshots(syntax, owner).into_iter().rev() {
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
                    let lexical_value = matches!(expr.kind, ExprKind::Path(_))
                        && rows.expression(expression).is_some_and(|i| {
                            !body.expressions[i].instantiated_arguments.is_empty()
                        });
                    if !lexical_value
                        && place(body, expression, rows).is_some()
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
                            label,
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
                            // A labelled block's breaks rejoin its tail here (LANG-11).
                            let join = if let Some(label) = label {
                                let join = node(
                                    &mut nodes,
                                    Op::IdentityReady(NameSite::Expression(expression)),
                                    vec![next],
                                    expr.origin.span,
                                );
                                env.blocks.push(BlockTarget {
                                    label: *label,
                                    block: expression,
                                    join,
                                    depth: env.scopes.len(),
                                    statement: env.statement,
                                    context,
                                    escaping,
                                    owning,
                                    consumer: env.consumer,
                                    operation,
                                });
                                join
                            } else {
                                next
                            };
                            let end = node(
                                &mut nodes,
                                Op::Kill(locals.clone()),
                                vec![join],
                                expr.origin.span,
                            );
                            for (position, whole) in local_value_places(
                                tree,
                                body,
                                rows,
                                &block_breaks,
                                expression,
                                &locals,
                            ) {
                                if whole {
                                    leaving.insert(position);
                                } else {
                                    ending.insert(
                                        position,
                                        (expression, env.consumer.unwrap_or(operation)),
                                    );
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
                                let commit = bind(
                                    &mut nodes,
                                    tree,
                                    arm.pattern,
                                    *value,
                                    arm.guard.is_some(),
                                    false,
                                    arm_entry,
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
                                    bind(
                                        &mut nodes,
                                        tree,
                                        arm.pattern,
                                        *value,
                                        true,
                                        true,
                                        guard_entry,
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
                        ExprKind::Try { value, map_error } => {
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
                            let mut kill = exit_statements(
                                &mut nodes,
                                &statements,
                                env.statement,
                                None,
                                kill,
                                expr.origin.span,
                            );
                            kill = node(
                                &mut nodes,
                                Op::Propagate {
                                    expression,
                                    value: *value,
                                    result: env.result.clone(),
                                },
                                vec![kill],
                                expr.origin.span,
                            );
                            if map_error.is_some() {
                                kill = node(
                                    &mut nodes,
                                    Op::MapError {
                                        expression,
                                        value: *value,
                                    },
                                    vec![kill],
                                    expr.origin.span,
                                );
                            }
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
                            let kill = exit_statements(
                                &mut nodes,
                                &statements,
                                env.statement,
                                None,
                                kill,
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
                        ExprKind::Break {
                            label: Some(label),
                            value,
                        } if let Some(target) =
                            env.blocks.iter().rev().find(|b| b.label == *label).cloned() =>
                        {
                            let exited: Vec<usize> = env
                                .scopes
                                .iter()
                                .skip(target.depth)
                                .flatten()
                                .copied()
                                .collect();
                            let exits = exit_statements(
                                &mut nodes,
                                &statements,
                                env.statement,
                                target.statement,
                                target.join,
                                expr.origin.span,
                            );
                            let kill = node(
                                &mut nodes,
                                Op::Kill(exited.clone()),
                                vec![exits],
                                expr.origin.span,
                            );
                            let Some(value) = value else {
                                nodes[entry].op = Op::Finish {
                                    expression: target.block,
                                    context: target.context,
                                    children: vec![],
                                    fresh: true,
                                };
                                nodes[entry].next = vec![kill];
                                continue;
                            };
                            let consumer = target.consumer.unwrap_or(target.operation);
                            for (position, whole) in
                                local_value_places(tree, body, rows, &block_breaks, *value, &exited)
                            {
                                if whole {
                                    leaving.insert(position);
                                } else {
                                    ending.insert(position, (target.block, consumer));
                                }
                            }
                            let forward = node(
                                &mut nodes,
                                Op::Forward {
                                    expression: target.block,
                                    child: *value,
                                },
                                vec![kill],
                                expr.origin.span,
                            );
                            let mut env = env;
                            env.consumer = target.consumer;
                            jobs.push(Job::Expr {
                                entry,
                                expression: *value,
                                context: target.context,
                                operation: *value,
                                escaping: target.escaping,
                                owning: target.owning,
                                next: forward,
                                env,
                            });
                            continue;
                        }
                        ExprKind::Break { label, .. } | ExprKind::Continue(label) => {
                            if let Some(&(break_to, continue_to, depth, loop_statement)) =
                                env.breaks.iter().rev().find(|(_, _, _, statement)| {
                                    label.is_none()
                                        || tree.statements[statement.0 as usize].kind.loop_label()
                                            == *label
                                })
                            {
                                let target = if matches!(expr.kind, ExprKind::Break { .. }) {
                                    break_to
                                } else {
                                    continue_to
                                };
                                nodes[entry].op = Op::Kill(
                                    env.scopes.iter().skip(depth).flatten().copied().collect(),
                                );
                                nodes[entry].next = vec![exit_statements(
                                    &mut nodes,
                                    &statements,
                                    env.statement,
                                    Some(loop_statement),
                                    target,
                                    expr.origin.span,
                                )];
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
                                .and_then(|(_, ty)| t.get(*ty));
                            let result = frame
                                .or_else(|| {
                                    expression_type(body, expression, rows)
                                        .and_then(|ty| t.get(ty))
                                        .and_then(|ty| match &ty.kind {
                                            TypeKind::Function(f) => t.get(f.result),
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
                        ExprKind::Path(_) if lexical_value => {
                            let captures = rows
                                .closure(expression)
                                .map(|i| body.closures[i].captures.as_slice())
                                .unwrap_or_default();
                            let context = capture_modes.first(expression).map(|c| c.context);
                            Op::Capture {
                                expression,
                                escaping: captures.is_empty()
                                    || context.map_or(escaping, |c| {
                                        matches!(c, CaptureContext::Escaping | CaptureContext::Task)
                                    }),
                                owning: context
                                    .map_or(owning, |c| c == CaptureContext::OwningScoped),
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
                                    arenas
                                        .definition
                                        .as_ref()
                                        .is_some_and(|arena| is_arena_type(t, p.ty, arena))
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
                            let mut result = t.get(call.result).unwrap().modes.clone();
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
                                            let mut modes = t.get(p.ty).unwrap().modes.clone();
                                            modes.sendability =
                                                crossing_requirement(t.get(p.ty).unwrap());
                                            modes.relation = lifetime(&modes.relation);
                                            modes
                                        },
                                        sendable_bound: sendable_parameter(
                                            t, body, *callee, *ordinal, rows,
                                        ),
                                    })
                                })
                                .collect();
                            // J's instantiated result includes construction-site
                            // sendability (notably erased iterator state). The
                            // declaration alone cannot recover that evidence;
                            // flow derives it again from `captures`.
                            if let Some(ty) = expression_type(body, expression, rows) {
                                result.sendability = t.get(ty).unwrap().modes.sendability.clone();
                                // GEN-1: T may instantiate to a once callable.
                                // The unspecialized result's Many default is
                                // not the instantiated call's permission.
                                result.multiplicity = t.get(ty).unwrap().modes.multiplicity;
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
                            let captures = match &t.get(call.result).unwrap().erased_captures {
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
                            fresh: matches!(expr.kind, ExprKind::Literal(_) | ExprKind::Heap)
                                || matches!(expr.kind, ExprKind::Path(_))
                                    && nullary.contains(&expression),
                        },
                    };
                    // A call whose result is Never has no normal successor
                    // (TIR v3): its failure site is the call itself, and a
                    // match arm it ends contributes no value to the join.
                    let successors = if matches!(op, Op::Call { .. })
                        && expression_type(body, expression, rows).is_some_and(|ty| {
                            matches!(
                                t.get(ty).unwrap().kind,
                                TypeKind::Primitive(Primitive::Never)
                            )
                        }) {
                        vec![]
                    } else {
                        vec![next]
                    };
                    if children.is_empty() {
                        nodes[entry].op = op;
                        nodes[entry].next = successors;
                    } else {
                        let lends = matches!(
                            op,
                            Op::Call {
                                constructor: false,
                                ..
                            }
                        );
                        let finish = node(&mut nodes, op, successors, expr.origin.span);
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
                            pattern,
                            value,
                            ty,
                            otherwise,
                            ..
                        } => {
                            let bind = bind(
                                &mut nodes,
                                tree,
                                *pattern,
                                *value,
                                false,
                                false,
                                next,
                                stmt.origin.span,
                            );
                            // LANG-13: a value the pattern misses runs the
                            // else block, which never reaches the bindings.
                            let matched = if let Some(otherwise) = otherwise {
                                let refused = reserve(&mut nodes, stmt.origin.span);
                                jobs.push(Job::Expr {
                                    entry: refused,
                                    expression: *otherwise,
                                    context: Context::Read,
                                    operation: *otherwise,
                                    escaping: false,
                                    owning: false,
                                    next: exit,
                                    env: env.clone(),
                                });
                                node(&mut nodes, Op::Nop, vec![bind, refused], stmt.origin.span)
                            } else {
                                bind
                            };
                            jobs.push(Job::Expr {
                                entry,
                                expression: *value,
                                // A let-else reads its scrutinee as match does.
                                context: if otherwise.is_some() {
                                    Context::Read
                                } else {
                                    Context::Own
                                },
                                operation: *value,
                                escaping: ty.is_some(),
                                owning: false,
                                next: matched,
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
                                context: if op.0.is_none() {
                                    self::place(body, *place, rows)
                                        .filter(|p| p.projections.is_empty())
                                        .map_or(Context::Store, |p| Context::Replacement(p.binding))
                                } else {
                                    Context::Store
                                },
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
                            ..
                        } => {
                            env.lifetime = lifetimes.child(env.lifetime);
                            let condition_entry = reserve(&mut nodes, stmt.origin.span);
                            nodes[entry].op = Op::IdentityReady(NameSite::Statement(statement));
                            nodes[entry].next = vec![condition_entry];
                            let branch = node(&mut nodes, Op::Nop, vec![], stmt.origin.span);
                            let loop_entry = reserve(&mut nodes, stmt.origin.span);
                            env.breaks.push((next, entry, env.scopes.len(), statement));
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
                            ..
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
                                            t.get(e.ty).unwrap().kind,
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
                            env.breaks
                                .push((loop_exit, kill, env.scopes.len(), statement));
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
                            let bind = bind(
                                &mut nodes,
                                tree,
                                *pattern,
                                *source,
                                false,
                                false,
                                loop_entry,
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
                            // The lent source is a view of its place, a field or
                            // element path as much as a binding (OWN-8): no count.
                            jobs.push(Job::Expr {
                                entry,
                                expression: *source,
                                context: if borrowed_place
                                    && rows.source_place(*source).is_some_and(|i| {
                                        matches!(body.places[i].root, SourceRoot::Binding { .. })
                                            && body.places[i].access == SourceAccess::Read
                                    }) {
                                    Context::View
                                } else {
                                    Context::Read
                                },
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
                                .and_then(|c| t.get(c.ty))
                                .and_then(|row| match &row.kind {
                                    TypeKind::Function(f) => t.get(f.result),
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
            block_breaks,
        });
    }
    Ok(graphs)
}

fn entry_snapshots(syntax: &ItemSyntax, owner: NameSite) -> Vec<ExprId> {
    let tree = &syntax.tree;
    let signature = match owner {
        NameSite::Item => match &syntax.item.kind {
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
            ExprKind::Return(value) | ExprKind::Break { value, .. } => pending.extend(value),
            ExprKind::Block {
                statements, tail, ..
            } => {
                pending.extend(tail);
                for statement in statements {
                    match &tree.statements[statement.0 as usize].kind {
                        StmtKind::Let {
                            value, otherwise, ..
                        } => pending.extend(std::iter::once(*value).chain(*otherwise)),
                        StmtKind::Expr(value) => pending.push(*value),
                        StmtKind::Assign { place, value, .. } => pending.extend([*place, *value]),
                        StmtKind::While {
                            condition, body, ..
                        } => pending.extend([*condition, *body]),
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
    t: &TypeInterner,
    body: &CheckedBody,
    callee: ExprId,
    ordinal: u32,
    rows: &super::source_rows::SourceRows,
) -> bool {
    let Some(TypeKind::Function(function)) = expression_type(body, callee, rows)
        .and_then(|ty| t.get(ty))
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
        .and_then(|parameter| t.get(parameter.ty))
        .map(|ty| &ty.kind)
    else {
        return false;
    };
    matches!(function.generics.get(parameter.0 as usize),
        Some(GenericParameter::Type { bounds, .. }) if bounds.contains(&Bound::Sendable))
}

fn callable_result_permissions(
    t: &TypeInterner,
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
    match ty.and_then(|ty| t.get(ty)).map(|ty| &ty.kind) {
        Some(TypeKind::Function(f)) => local_result_permissions(t, f, body.lexical.site(owner)),
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
/// The rows the graphs name: frame results, call requirements and the
/// declared parts constructors write.
pub(super) fn graph_types(graphs: &[Graph]) -> Vec<TypeId> {
    let mut out = vec![];
    for graph in graphs {
        out.extend(graph.result_ty);
        for node in &graph.nodes {
            if let Op::Call {
                requirements,
                parts,
                ..
            } = &node.op
            {
                out.extend(requirements.iter().flatten().map(|r| r.ty));
                out.extend(parts.iter().flatten().copied());
            }
        }
    }
    out
}
pub(super) fn call_arguments(
    body: &CheckedBody,
    properties: &super::properties::PropertyTable,
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
                    .and_then(|ty| properties.get(ty))
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
