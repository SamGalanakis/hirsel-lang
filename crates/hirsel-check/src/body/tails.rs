//! Source tail eligibility belongs to H's declaration and scoped-bound facts.
use super::*;
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
    tail: bool,
    barrier: bool,
}
impl Branch {
    fn expression(id: ast::ExprId, tail: bool, barrier: bool) -> Self {
        Self {
            expression: Some(id),
            guard: None,
            pattern: None,
            tail,
            barrier,
        }
    }
    fn unchanged() -> Self {
        Self {
            expression: None,
            guard: None,
            pattern: None,
            tail: false,
            barrier: false,
        }
    }
}
enum Visit {
    Expression(ast::ExprId, bool, bool, usize),
    Statement(ast::StmtId, bool, usize),
    Introduce(ast::PatternId, usize),
    Hold(ast::ExprId, usize),
    Consume(ast::ExprId, usize),
    ReleaseTemporary(ast::ExprId, usize),
    Assign(ast::ExprId, usize),
    Call(ast::ExprId, bool, bool, usize),
    Exit(BTreeSet<usize>, BTreeSet<ast::ExprId>, usize),
    Fork(Vec<Branch>, usize),
    Join(Vec<usize>, usize),
    Terminate(usize),
}
impl Checker<'_, '_> {
    pub(super) fn publish_tails(&mut self) -> Result<(), QueryError> {
        let ast::ItemKind::Function {
            body: Some(root),
            signature,
            ..
        } = &self.input.tree.items[self.input.item.0 as usize].kind
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
        if !self.body.calls.iter().any(|c|matches!(&c.target,CallTarget::Definition(p) if p == &self.input.signature.definition)) {return Ok(());}
        let owner = NameSite::Item(self.input.item);
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
        let mut todo = vec![Visit::Expression(*root, true, false, 0)];
        while let Some(visit) = todo.pop() {
            match visit {
                Visit::Expression(id, tail, barrier, state) => {
                    if states[state].terminated {
                        continue;
                    }
                    use ast::ExprKind as E;
                    match self.input.tree.expressions[id.0 as usize].kind.clone() {
                        E::Block {
                            statements,
                            tail: value,
                        } => {
                            todo.push(Visit::Exit(
                                states[state].live.clone(),
                                states[state].temporaries.clone(),
                                state,
                            ));
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
                            let branches = arms
                                .into_iter()
                                .map(|a| Branch {
                                    expression: Some(a.body),
                                    guard: a.guard,
                                    pattern: Some(a.pattern),
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
                            for arg in arguments.into_iter().rev() {
                                todo.push(Visit::Hold(arg.value, state));
                                todo.push(Visit::Expression(arg.value, false, barrier, state));
                            }
                            todo.push(Visit::Expression(callee, false, barrier, state));
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
                                    Branch::expression(right, false, barrier),
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
                        | E::Formatter(_)
                        | E::Shape(_)
                        | E::Heap
                        | E::Result
                        | E::Todo(_)
                        | E::Error => (),
                        E::Break(_) | E::Continue(_) => states[state].terminated = true,
                    }
                }
                Visit::Statement(id, barrier, state) => {
                    if states[state].terminated {
                        continue;
                    }
                    match self.input.tree.statements[id.0 as usize].kind.clone() {
                        ast::StmtKind::Let {
                            pattern,
                            value,
                            otherwise,
                            ..
                        } => {
                            todo.push(Visit::Introduce(pattern, state));
                            if let Some(otherwise) = otherwise {
                                // LANG-13: a refused value runs the else block.
                                todo.push(Visit::Fork(
                                    vec![
                                        Branch::expression(otherwise, false, barrier),
                                        Branch::unchanged(),
                                    ],
                                    state,
                                ));
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
                Visit::Introduce(pattern, state) => {
                    let mut patterns = vec![pattern];
                    while let Some(pattern) = patterns.pop() {
                        states[state].live.extend(
                            self.body.bindings.iter().enumerate().filter_map(|(i, b)| {
                                (b.owner == owner && b.site == NameSite::Pattern(pattern))
                                    .then_some(i)
                            }),
                        );
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
                    if tail
                        && !barrier
                        && call.state == CallState::Complete
                        && matches!(&call.target,CallTarget::Definition(p) if p == &self.input.signature.definition)
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
                            if (matches!(b.binding, ResolvedName::Local(_))
                                || !matches!(b.usage, UseKind::Borrow | UseKind::Mut))
                                && !self.bound(b.ty, ast::Bound::Plain)?
                            {
                                eligible = false;
                                break;
                            }
                        }
                        for temporary in states[state].temporaries.iter().copied() {
                            if !owned_loop_arguments.iter().any(|(_, e)| *e == temporary)
                                && !self.bound(self.values[&temporary], ast::Bound::Plain)?
                            {
                                eligible = false;
                                break;
                            }
                        }
                        if eligible {
                            self.body.tails.push(CheckedTail {
                                call: id,
                                kind: CheckedTailKind::SelfTail,
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
                            visits.push(Visit::Introduce(pattern, child));
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
            }
        }
        self.body.tails.sort_by_key(|t| t.call);
        self.body.tails.dedup_by_key(|t| t.call);
        Ok(())
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
