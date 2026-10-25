//! Source tail eligibility belongs to H's declaration and scoped-bound facts.
//! Constructor-tail sites (LANG-10) follow OCaml 4.14's `choice_makeblock`
//! (lambda/tmc.ml:741): a self call in the hole of a constructor chain, here
//! restricted to the last source-evaluated field at every level.
use super::*;
/// A constructor-chain position: the declaration field ordinals from the
/// returned constructor down to here, and the earlier fields, which become
/// part of the partial result rather than pending frame obligations.
#[derive(Clone)]
struct Hole {
    path: Vec<u32>,
    fields: Vec<ast::ExprId>,
    /// The innermost level is `box`: the call is the box's payload, so every
    /// hole is a box payload of the function's result type.
    boxed: bool,
}
#[derive(Clone, Default)]
struct State {
    live: BTreeSet<usize>,
    temporaries: BTreeSet<ast::ExprId>,
    terminated: bool,
}
struct Branch {
    expression: Option<ast::ExprId>,
    guard: Option<ast::ExprId>,
    pattern: Option<ast::PatternId>,
    borrowed_pattern: bool,
    tail: bool,
    barrier: bool,
}
impl Branch {
    fn expression(id: ast::ExprId, tail: bool, barrier: bool) -> Self {
        Self {
            expression: Some(id),
            guard: None,
            pattern: None,
            borrowed_pattern: false,
            tail,
            barrier,
        }
    }
    fn unchanged() -> Self {
        Self {
            expression: None,
            guard: None,
            pattern: None,
            borrowed_pattern: false,
            tail: false,
            barrier: false,
        }
    }
}
enum Visit {
    Expression(ast::ExprId, bool, bool, usize),
    Statement(ast::StmtId, bool, usize),
    Introduce(ast::PatternId, bool, usize),
    Hold(ast::ExprId, usize),
    Consume(ast::ExprId, usize),
    /// An earlier constructor field moved into the partial result.
    ConsumeField(ast::ExprId, ast::ExprId, usize),
    ReleaseTemporary(ast::ExprId, usize),
    Assign(ast::ExprId, usize),
    Call(ast::ExprId, bool, bool, usize),
    Exit(BTreeSet<usize>, BTreeSet<ast::ExprId>, usize),
    Fork(Vec<Branch>, usize),
    Join(Vec<usize>, usize),
    Terminate(usize),
    /// A break leaving a labelled block: its state rejoins at the block's end.
    BreakTo(ast::ExprId, usize),
    BlockEnd(ast::ExprId, usize),
}
impl Checker<'_, '_> {
    pub(super) fn publish_tails(&mut self) -> Result<(), QueryError> {
        let ast::ItemKind::Function {
            body: Some(root),
            signature,
            ..
        } = &self.input.syntax.item.kind
        else {
            return Ok(());
        };
        if signature
            .contracts
            .iter()
            .any(|c| c.kind == ast::ContractKind::Ensures)
        {
            return Ok(());
        }
        if !self.body.calls.iter().any(|c|matches!(&c.target,CallTarget::Definition(p) if p == self.input.signature.definition.path())) {return Ok(());}
        let constructor_tail = self.constructor_tail_function()?;
        let mut holes: BTreeMap<ast::ExprId, Hole> = BTreeMap::new();
        let owner = NameSite::Item;
        let initial = State {
            live: self
                .body
                .bindings
                .iter()
                .enumerate()
                .filter_map(|(i, b)| {
                    (b.owner == owner && matches!(b.binding, ResolvedName::Parameter(_)))
                        .then_some(i)
                })
                .collect(),
            ..State::default()
        };
        let mut states = vec![initial];
        let mut borrowed_bindings = BTreeSet::new();
        let exits = self.input.syntax.tree.block_exits(*root);
        let mut breaks: BTreeMap<ast::ExprId, Vec<State>> = BTreeMap::new();
        let mut todo = vec![Visit::Expression(*root, true, false, 0)];
        while let Some(visit) = todo.pop() {
            match visit {
                Visit::Expression(id, tail, barrier, state) => {
                    if states[state].terminated {
                        continue;
                    }
                    use ast::ExprKind as E;
                    match self.input.syntax.tree.expressions[id.0 as usize]
                        .kind
                        .clone()
                    {
                        E::Block {
                            label,
                            statements,
                            tail: value,
                        } => {
                            todo.push(Visit::Exit(
                                states[state].live.clone(),
                                states[state].temporaries.clone(),
                                state,
                            ));
                            // A labelled block's result joins its breaks: none
                            // of its parts is a tail (LANG-11).
                            let tail = tail && label.is_none();
                            if label.is_some() {
                                todo.push(Visit::BlockEnd(id, state));
                            }
                            if let Some(value) = value {
                                todo.push(Visit::Expression(value, tail, barrier, state));
                            }
                            todo.extend(
                                statements
                                    .into_iter()
                                    .rev()
                                    .map(|s| Visit::Statement(s, barrier, state)),
                            );
                        }
                        E::If {
                            condition,
                            then_branch,
                            else_branch,
                        } => {
                            let mut branches = vec![Branch::expression(then_branch, tail, barrier)];
                            branches.push(else_branch.map_or_else(Branch::unchanged, |no| {
                                Branch::expression(no, tail, barrier)
                            }));
                            todo.push(Visit::Fork(branches, state));
                            todo.push(Visit::Expression(condition, false, barrier, state));
                        }
                        E::Match { value, arms } => {
                            // A match of a borrowed parameter introduces views,
                            // not owners whose cleanup must survive the call.
                            let borrowed_pattern = !self.tail_owned_expression(value)
                                && self.source_place(value).is_some_and(|place| {
                                    let SourceRoot::Binding { owner, binding } = &place.root else {
                                        return false;
                                    };
                                    place.projections.is_empty()
                                        && matches!(binding, ResolvedName::Parameter(_))
                                        && self.body.bindings.iter().any(|b| {
                                            b.owner == *owner
                                                && b.binding == *binding
                                                && b.usage == UseKind::Borrow
                                        })
                                });
                            let branches = arms
                                .into_iter()
                                .map(|a| Branch {
                                    expression: Some(a.body),
                                    guard: a.guard,
                                    pattern: Some(a.pattern),
                                    borrowed_pattern,
                                    tail,
                                    barrier,
                                })
                                .collect();
                            todo.push(Visit::ReleaseTemporary(value, state));
                            todo.push(Visit::Fork(branches, state));
                            todo.push(Visit::Hold(value, state));
                            todo.push(Visit::Expression(value, false, barrier, state));
                        }
                        E::Return(value) => {
                            todo.push(Visit::Terminate(state));
                            if let Some(value) = value {
                                todo.push(Visit::Expression(value, true, barrier, state));
                            }
                        }
                        E::Call { callee, arguments } => {
                            todo.push(Visit::Call(id, tail, barrier, state));
                            let hole = match (tail, holes.get(&id)) {
                                (_, Some(h)) => Some(h.clone()),
                                (true, None) => Some(Hole {
                                    path: vec![],
                                    fields: vec![],
                                    boxed: false,
                                }),
                                _ => None,
                            };
                            let mut consumed = vec![];
                            if constructor_tail
                                && let Some(hole) = hole
                                && let Some(last) = arguments.last()
                                && let Some(ordinal) = self.constructor_hole(id, last.value)?
                            {
                                let mut path = hole.path;
                                path.push(ordinal);
                                let mut fields = hole.fields;
                                fields.extend(arguments.iter().rev().skip(1).map(|a| a.value));
                                holes.insert(
                                    last.value,
                                    Hole {
                                        path,
                                        fields,
                                        boxed: false,
                                    },
                                );
                                consumed =
                                    arguments.iter().rev().skip(1).map(|a| a.value).collect();
                            }
                            for arg in arguments.into_iter().rev() {
                                if consumed.contains(&arg.value) {
                                    todo.push(Visit::ConsumeField(id, arg.value, state));
                                }
                                todo.push(Visit::Hold(arg.value, state));
                                todo.push(Visit::Expression(arg.value, false, barrier, state));
                            }
                            todo.push(Visit::Expression(callee, false, barrier, state));
                        }
                        E::Unary {
                            op: ast::UnaryOp::Box,
                            value,
                        } if constructor_tail && (tail || holes.contains_key(&id)) => {
                            let mut hole = holes.get(&id).cloned().unwrap_or(Hole {
                                path: vec![],
                                fields: vec![],
                                boxed: false,
                            });
                            hole.path.push(0);
                            hole.boxed = true;
                            holes.insert(value, hole);
                            todo.push(Visit::Expression(value, false, barrier, state));
                        }
                        E::Tuple(values)
                            if constructor_tail
                                && (tail || holes.contains_key(&id))
                                && !values.is_empty() =>
                        {
                            let mut hole = holes.get(&id).cloned().unwrap_or(Hole {
                                path: vec![],
                                fields: vec![],
                                boxed: false,
                            });
                            hole.path.push(values.len() as u32 - 1);
                            hole.boxed = false;
                            hole.fields.extend(values.iter().rev().skip(1).copied());
                            holes.insert(*values.last().unwrap(), hole);
                            todo.push(Visit::Exit(
                                states[state].live.clone(),
                                states[state].temporaries.clone(),
                                state,
                            ));
                            for value in values.into_iter().rev() {
                                todo.push(Visit::Hold(value, state));
                                todo.push(Visit::Expression(value, false, barrier, state));
                            }
                        }
                        E::Tuple(values) | E::Sequence(values) => {
                            todo.push(Visit::Exit(
                                states[state].live.clone(),
                                states[state].temporaries.clone(),
                                state,
                            ));
                            for value in values.into_iter().rev() {
                                todo.push(Visit::Hold(value, state));
                                todo.push(Visit::Expression(value, false, barrier, state));
                            }
                        }
                        E::Unary {
                            op: ast::UnaryOp::Take,
                            value,
                        } => {
                            todo.push(Visit::Consume(id, state));
                            todo.push(Visit::Expression(value, false, barrier, state));
                        }
                        E::Unary { value, .. }
                        | E::Field { base: value, .. }
                        | E::Try { value, .. }
                        | E::Before(value)
                        | E::Comptime(value) => {
                            todo.push(Visit::Expression(value, false, barrier, state))
                        }
                        E::Index { base, index } => {
                            todo.push(Visit::Expression(index, false, barrier, state));
                            todo.push(Visit::Expression(base, false, barrier, state));
                        }
                        E::Binary {
                            op: ast::BinaryOp::And | ast::BinaryOp::Or,
                            left,
                            right,
                        } => {
                            todo.push(Visit::Fork(
                                vec![
                                    // Only the selected RHS supplies the enclosing result.
                                    Branch::expression(right, tail, barrier),
                                    Branch::unchanged(),
                                ],
                                state,
                            ));
                            todo.push(Visit::Expression(left, false, barrier, state));
                        }
                        E::Binary { left, right, .. } => {
                            todo.push(Visit::Expression(right, false, barrier, state));
                            todo.push(Visit::Expression(left, false, barrier, state));
                        }
                        E::Closure { .. }
                        | E::Literal(_)
                        | E::Path(_)
                        | E::With { .. }
                        | E::Formatter(_)
                        | E::Shape(_)
                        | E::Heap
                        | E::Result
                        | E::Todo(_)
                        | E::Error => (),
                        E::Break { value, .. } if exits.contains_key(&id) => {
                            todo.push(Visit::BreakTo(exits[&id], state));
                            if let Some(value) = value {
                                todo.push(Visit::Expression(value, false, barrier, state));
                            }
                        }
                        E::Break { .. } | E::Continue(_) => states[state].terminated = true,
                    }
                }
                Visit::Statement(id, barrier, state) => {
                    if states[state].terminated {
                        continue;
                    }
                    match self.input.syntax.tree.statements[id.0 as usize]
                        .kind
                        .clone()
                    {
                        ast::StmtKind::Let {
                            pattern,
                            value,
                            otherwise,
                            ..
                        } => {
                            todo.push(Visit::Introduce(pattern, false, state));
                            if let Some(otherwise) = otherwise {
                                // The refused initializer remains owned until the
                                // else block exits, just as a match scrutinee does.
                                // Its cleanup can outlive a recursive return.
                                todo.push(Visit::ReleaseTemporary(value, state));
                                todo.push(Visit::Fork(
                                    vec![
                                        Branch::expression(otherwise, false, barrier),
                                        Branch::unchanged(),
                                    ],
                                    state,
                                ));
                                todo.push(Visit::Hold(value, state));
                            }
                            todo.push(Visit::Expression(value, false, barrier, state));
                        }
                        ast::StmtKind::Expr(value) => {
                            todo.push(Visit::Expression(value, false, barrier, state))
                        }
                        ast::StmtKind::Assign { place, value, .. } => {
                            todo.push(Visit::Assign(place, state));
                            todo.push(Visit::Expression(value, false, barrier, state));
                            todo.push(Visit::Expression(place, false, barrier, state));
                        }
                        ast::StmtKind::While {
                            condition, body, ..
                        } => {
                            // A zero-iteration path keeps all incoming owners live.
                            todo.push(Visit::Fork(
                                vec![
                                    Branch::expression(body, false, barrier),
                                    Branch::unchanged(),
                                ],
                                state,
                            ));
                            todo.push(Visit::Expression(condition, false, barrier, state));
                        }
                        ast::StmtKind::For { source, body, .. } => {
                            let cursor = !self.plans.loops.iter().any(|p| {
                                p.statement() == id
                                    && matches!(p.protocol, ForProtocol::Range { .. })
                            });
                            todo.push(Visit::Fork(
                                vec![
                                    Branch::expression(body, false, barrier || cursor),
                                    Branch::unchanged(),
                                ],
                                state,
                            ));
                            todo.push(Visit::Expression(source, false, barrier, state));
                        }
                        ast::StmtKind::Function { .. } => states[state].live.extend(
                            self.body.bindings.iter().enumerate().filter_map(|(i, b)| {
                                (b.owner == owner && b.site == NameSite::Statement(id)).then_some(i)
                            }),
                        ),
                    }
                }
                Visit::Introduce(pattern, borrowed, state) => {
                    let mut patterns = vec![pattern];
                    while let Some(pattern) = patterns.pop() {
                        for (i, b) in self.body.bindings.iter().enumerate() {
                            if b.owner == owner && b.site == NameSite::Pattern(pattern) {
                                states[state].live.insert(i);
                                if borrowed && b.usage == UseKind::Borrow {
                                    borrowed_bindings.insert(i);
                                }
                            }
                        }
                        if let Some(p) = self.body.patterns.iter().find(|p| p.pattern == pattern) {
                            patterns.extend(p.subpatterns());
                        }
                    }
                }
                Visit::Hold(id, state) => {
                    if !states[state].terminated && self.tail_owned_expression(id) {
                        states[state].temporaries.insert(id);
                    }
                }
                Visit::Consume(id, state) => self.tail_consume(id, &mut states[state]),
                Visit::ConsumeField(call, field, state) => {
                    let take = self.body.calls.iter().any(|c| {
                        c.expression == call
                            && c.arguments.iter().any(|a| {
                                a.expression == field
                                    && a.parameter
                                        .as_ref()
                                        .is_some_and(|(_, p)| p.passing == PassingMode::Take)
                            })
                    });
                    if take {
                        self.tail_consume(field, &mut states[state]);
                    }
                }
                Visit::Assign(id, state) => {
                    if let Some(place) = self.source_place(id).filter(|p| p.projections.is_empty())
                    {
                        if let SourceRoot::Binding { owner, binding } = &place.root {
                            states[state].live.extend(
                                self.body.bindings.iter().enumerate().filter_map(|(i, b)| {
                                    (b.owner == *owner && b.binding == *binding).then_some(i)
                                }),
                            );
                        }
                    }
                }
                Visit::ReleaseTemporary(id, state) => {
                    states[state].temporaries.remove(&id);
                }
                Visit::Call(id, tail, barrier, state) => {
                    if states[state].terminated {
                        continue;
                    }
                    let Some(call) = self.body.calls.iter().find(|c| c.expression == id).cloned()
                    else {
                        continue;
                    };
                    for arg in &call.arguments {
                        if arg
                            .parameter
                            .as_ref()
                            .is_some_and(|(_, p)| p.passing == PassingMode::Take)
                        {
                            self.tail_consume(arg.expression, &mut states[state]);
                            states[state].temporaries.remove(&arg.expression);
                        }
                    }
                    let hole = holes.get(&id).filter(|h| !tail && h.boxed).cloned();
                    if (tail || hole.is_some())
                        && !barrier
                        && call.state == CallState::Complete
                        && matches!(&call.target,CallTarget::Definition(p) if p == self.input.signature.definition.path())
                    {
                        let mut eligible = true;
                        let mut owned_loop_arguments = vec![];
                        for arg in &call.arguments {
                            let Some((ordinal, parameter)) = &arg.parameter else {
                                eligible = false;
                                break;
                            };
                            if parameter.passing == PassingMode::Mut {
                                eligible = false;
                                break;
                            }
                            let ty = self.values[&arg.expression];
                            if parameter.passing == PassingMode::Borrow
                                && !self.bound(ty, ast::Bound::Plain)?
                            {
                                if states[state].temporaries.contains(&arg.expression) && self.bound(ty,ast::Bound::Data)? {
                                    owned_loop_arguments.push((ParamId(*ordinal),arg.expression));
                                } else if !self.source_place(arg.expression).is_some_and(|p| matches!(p.root, SourceRoot::Binding{ref binding,..} if matches!(binding,ResolvedName::Parameter(_)))) {
                                    eligible=false;break;
                                }
                            }
                        }
                        for index in states[state].live.iter().copied() {
                            let b = &self.body.bindings[index];
                            // A temporary loop argument can retain a view of a
                            // pattern binding; only parameter restarts may leave
                            // these borrowed bindings behind.
                            let borrowed = borrowed_bindings.contains(&index)
                                && owned_loop_arguments.is_empty();
                            if (matches!(b.binding, ResolvedName::Local(_)) && !borrowed
                                || !matches!(b.usage, UseKind::Borrow | UseKind::Mut))
                                && !self.bound(b.ty, ast::Bound::Plain)?
                            {
                                eligible = false;
                                break;
                            }
                        }
                        for temporary in states[state].temporaries.iter().copied() {
                            if !owned_loop_arguments.iter().any(|(_, e)| *e == temporary)
                                && !hole.as_ref().is_some_and(|h| h.fields.contains(&temporary))
                                && !self.bound(self.values[&temporary], ast::Bound::Plain)?
                            {
                                eligible = false;
                                break;
                            }
                        }
                        if eligible {
                            self.body.tails.push(CheckedTail {
                                call: id,
                                kind: match hole {
                                    Some(h) => CheckedTailKind::ConstructorHole(h.path),
                                    None => CheckedTailKind::SelfTail,
                                },
                                owned_loop_arguments,
                            });
                        }
                    }
                    for arg in call.arguments {
                        states[state].temporaries.remove(&arg.expression);
                    }
                }
                Visit::Exit(live, temporaries, state) => {
                    states[state].live.retain(|i| live.contains(i));
                    states[state]
                        .temporaries
                        .retain(|i| temporaries.contains(i));
                }
                Visit::Fork(branches, state) => {
                    let mut children = vec![];
                    let mut visits = vec![];
                    for branch in branches {
                        let child = states.len();
                        states.push(states[state].clone());
                        children.push(child);
                        if let Some(pattern) = branch.pattern {
                            visits.push(Visit::Introduce(pattern, branch.borrowed_pattern, child));
                        }
                        if let Some(guard) = branch.guard {
                            visits.push(Visit::Expression(guard, false, branch.barrier, child));
                        }
                        if let Some(id) = branch.expression {
                            visits.push(Visit::Expression(id, branch.tail, branch.barrier, child));
                        }
                        visits.push(Visit::Exit(
                            states[state].live.clone(),
                            states[state].temporaries.clone(),
                            child,
                        ));
                    }
                    todo.push(Visit::Join(children, state));
                    todo.extend(visits.into_iter().rev());
                }
                Visit::Join(children, state) => {
                    let mut joined = State {
                        terminated: true,
                        ..State::default()
                    };
                    for child in children {
                        let branch = std::mem::take(&mut states[child]);
                        if !branch.terminated {
                            joined.live.extend(branch.live);
                            joined.temporaries.extend(branch.temporaries);
                            joined.terminated = false;
                        }
                    }
                    states[state] = joined;
                }
                Visit::Terminate(state) => states[state].terminated = true,
                Visit::BreakTo(block, state) => {
                    if !states[state].terminated {
                        breaks.entry(block).or_default().push(states[state].clone());
                        states[state].terminated = true;
                    }
                }
                Visit::BlockEnd(block, state) => {
                    for exit in breaks.remove(&block).unwrap_or_default() {
                        if states[state].terminated {
                            states[state] = exit;
                        } else {
                            states[state].live.extend(exit.live);
                            states[state].temporaries.extend(exit.temporaries);
                        }
                    }
                }
            }
        }
        self.body.tails.sort_by_key(|t| t.call);
        self.body.tails.dedup_by_key(|t| t.call);
        Ok(())
    }
    /// LANG-10: a function whose constructor-tail sites can be guaranteed. It
    /// is monomorphic, data in and data out (so no step of the loop holds a
    /// capability and none can suspend), with a global result.
    fn constructor_tail_function(&mut self) -> Result<bool, QueryError> {
        let SignatureKind::Function(f) = &self.input.signature.kind else {
            return Ok(false);
        };
        let f = f.clone();
        if !f.generics.is_empty() || f.foreign.is_some() {
            return Ok(false);
        }
        if self.types.get(f.result).map(|t| t.modes.relation.clone())
            != Some(hirsel_types::Relation::global())
        {
            return Ok(false);
        }
        if !self.bound(f.result, ast::Bound::Data)? {
            return Ok(false);
        }
        for p in f.parameters.iter() {
            if !self.bound(p.ty, ast::Bound::Data)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// The declaration ordinal of `field`, the last written field of the
    /// constructor call `call`, when that constructor is an ordinary data
    /// record or variant: no flat, packed or other layout promise, no
    /// `distinct` fact, no resource and no capability.
    fn constructor_hole(
        &mut self,
        call: ast::ExprId,
        field: ast::ExprId,
    ) -> Result<Option<u32>, QueryError> {
        let Some(c) = self
            .body
            .calls
            .iter()
            .find(|c| c.expression == call)
            .cloned()
        else {
            return Ok(None);
        };
        if !matches!(c.target, CallTarget::Constructor(_)) || c.state != CallState::Complete {
            return Ok(None);
        }
        let Some(ordinal) = c
            .arguments
            .last()
            .filter(|a| a.expression == field)
            .and_then(|a| a.parameter.as_ref().map(|(k, _)| *k))
        else {
            return Ok(None);
        };
        let Some(&ty) = self.values.get(&call) else {
            return Ok(None);
        };
        let TypeKind::Named { definition, .. } = self.kind(ty) else {
            return Ok(None);
        };
        let s = match self.snapshot(definition.path()) {
            Ok(s) => s,
            Err(QueryError::Diagnostics(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let SignatureKind::Type(t) = &s.kind else {
            return Ok(None);
        };
        Ok((t.layout == hirsel_types::Layout::default()
            && !t.distinct
            && !t.resource
            && t.fact.is_none()
            && t.capability.is_none()
            && t.drop.is_none())
        .then_some(ordinal))
    }
    fn tail_owned_expression(&self, id: ast::ExprId) -> bool {
        self.source_place(id).is_some_and(|p| {
            matches!(p.root, SourceRoot::Value(_))
                || matches!(
                    p.access,
                    SourceAccess::Copy | SourceAccess::Unique | SourceAccess::Take
                )
        })
    }
    fn tail_consume(&self, id: ast::ExprId, state: &mut State) {
        if let Some(place) = self.source_place(id) {
            if place.projections.is_empty() {
                if let SourceRoot::Binding { owner, binding } = &place.root {
                    state.live.retain(|i| {
                        self.body.bindings[*i].owner != *owner
                            || self.body.bindings[*i].binding != *binding
                    });
                }
            }
        }
    }
}
