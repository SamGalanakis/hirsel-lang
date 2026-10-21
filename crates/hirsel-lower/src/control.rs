use super::builder::{Builder, Loop, State, Value};
use super::*;
use hirsel_syntax::{BinaryOp, ExprKind, MatchArm as SourceMatchArm, StmtKind};
use hirsel_tir::*;
use hirsel_types::UseKind;
use std::collections::BTreeSet;
impl Builder<'_, '_> {
    pub fn entry_checks(&mut self, bodies: &mut Vec<Option<Body>>) -> Result<()> {
        let checks: Vec<_> = self
            .decisions
            .entries
            .get(&self.owner)
            .iter()
            .copied()
            .filter(|(owner, _, _)| *owner == self.owner)
            .cloned()
            .collect();
        for (_, expression, rule) in checks {
            let span = self.span(expression)?;
            let Some(mut value) = self.expression(expression, bodies)? else {
                return self.error(span, "entry contract diverges");
            };
            let _ = self.loan(&mut value, span);
            let failure = self.cleanup();
            let condition = self.consume(value);
            self.emit_with(InstructionKind::Check { condition, rule }, span, failure);
        }
        let contracts: Vec<_> = self
            .decisions
            .contracts
            .get(&self.owner)
            .iter()
            .copied()
            .filter(|c| c.owner == self.owner)
            .cloned()
            .collect();
        for contract in contracts {
            self.clause_value(&contract, bodies)?;
            // Snapshots are evaluated once at entry, including ensures-only uses.
            for before in &contract.snapshots {
                let before = *before;
                if self.snapshots.iter().any(|(e, _)| *e == before) {
                    continue;
                }
                let ExprKind::Before(expression) =
                    self.input.syntax.tree.expressions[before.0 as usize].kind
                else {
                    return self.error(self.span(before)?, "snapshot plan refers to non-before");
                };
                let span = self.span(expression)?;
                let Some(value) = self.expression(expression, bodies)? else {
                    return self.error(span, "entry snapshot diverges");
                };
                let value = self.transport(value, Storage::Temporary, span);
                self.snapshots.push((before, value.place));
            }
            if contract.kind == hirsel_syntax::ContractKind::Requires {
                self.invoke_clause(&contract)?;
            }
        }
        Ok(())
    }
    fn clause_value(
        &mut self,
        clause: &hirsel_check_protocol::body::ContractPlan,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        let span = self.span(clause.expression)?;
        let body_id = BodyId(bodies.len() as u32);
        bodies.push(None);
        let hirsel_types::TypeKind::Function(function) =
            &self.types.get(clause.function).unwrap().kind
        else {
            return self.error(span, "missing checked clause function");
        };
        let result = function.result;
        let mut child = super::builder::Builder::new(
            self.input,
            self.decisions,
            hirsel_types::NameSite::Expression(clause.expression),
            result,
        )?;
        for (ordinal, parameter) in clause.parameters.iter().enumerate() {
            let place = child.roots[child.body.parameters[ordinal].0 as usize];
            match parameter {
                hirsel_check_protocol::body::ContractParameter::Snapshot(expression) => {
                    child.snapshots.push((*expression, place))
                }
                hirsel_check_protocol::body::ContractParameter::Result => {
                    child.contract_result = Some(place)
                }
                hirsel_check_protocol::body::ContractParameter::Parameter(_) => (),
            }
        }
        let Some(value) = child.expression(clause.expression, bodies)? else {
            return child.error(span, "contract clause diverges");
        };
        child.return_value(value, span, bodies)?;
        bodies[body_id.0 as usize] = Some(child.finish()?);
        let value = self.rvalue(
            clause.function,
            Rvalue::Closure {
                body: body_id,
                captures: vec![],
                destination: self.row_destination(clause.function),
            },
            span,
        );
        self.clauses.push((clause.expression, value.place));
        Ok(())
    }
    fn invoke_clause(&mut self, clause: &hirsel_check_protocol::body::ContractPlan) -> Result<()> {
        let span = self.span(clause.expression)?;
        let callable = self
            .clauses
            .iter()
            .find(|(e, _)| *e == clause.expression)
            .map(|(_, p)| *p)
            .ok_or_else(|| LowerError {
                site: span,
                message: "missing clause callable".into(),
            })?;
        let mut loans = vec![];
        let mut arguments = vec![];
        let mut function = Value {
            place: callable,
            usage: hirsel_types::UseKind::Borrow,
            checked_ty: None,
        };
        loans.push(self.loan(&mut function, span));
        for (ordinal, parameter) in clause.parameters.iter().enumerate() {
            let place = match parameter {
                hirsel_check_protocol::body::ContractParameter::Parameter(parameter) => self
                    .lookup(
                        &hirsel_types::ResolvedName::Parameter(*parameter),
                        &clause.owner,
                        span,
                    )?,
                hirsel_check_protocol::body::ContractParameter::Snapshot(snapshot) => self
                    .snapshots
                    .iter()
                    .find(|(e, _)| e == snapshot)
                    .map(|(_, p)| *p)
                    .ok_or_else(|| LowerError {
                        site: span,
                        message: "missing entry snapshot argument".into(),
                    })?,
                hirsel_check_protocol::body::ContractParameter::Result => {
                    self.contract_result.ok_or_else(|| LowerError {
                        site: span,
                        message: "missing exit result argument".into(),
                    })?
                }
            };
            let mut value = Value {
                place,
                usage: hirsel_types::UseKind::Borrow,
                checked_ty: None,
            };
            loans.push(self.loan(&mut value, span));
            arguments.push(CallArgument {
                parameter: hirsel_types::ParamId(ordinal as u32),
                value: self.consume(value),
            });
        }
        let hirsel_types::TypeKind::Function(header) =
            &self.types.get(clause.function).unwrap().kind
        else {
            return self.error(span, "missing checked clause function");
        };
        let destination = self.slot(header.result, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let next = self.block(span);
        let on_failure = self.cleanup();
        let function = self.consume(function);
        self.terminate(
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: true,
                    value: function,
                    arguments: vec![],
                },
                arguments,
                returns: Some(CallReturn {
                    destination: destination,
                    next: next,
                }),
                on_failure,
            },
            span,
        );
        self.current = Some(next);
        self.state.live.insert(self.root(destination).0);
        for loan in loans.into_iter().rev() {
            self.end_loan(loan, span);
        }
        self.emit(
            InstructionKind::Check {
                condition: Operand {
                    place: destination,
                    usage: hirsel_types::UseKind::Copy,
                },
                rule: const { hirsel_diag::RuleId::of("FACT-1") },
            },
            span,
        );
        let temps = [self.root(destination).0].into();
        self.retire_plain(&temps, span);
        self.state.live.remove(&self.root(destination).0);
        Ok(())
    }
    pub fn exit_checks(&mut self, _bodies: &mut Vec<Option<Body>>) -> Result<()> {
        let clauses: Vec<_> = self
            .decisions
            .contracts
            .get(&self.owner)
            .iter()
            .copied()
            .filter(|c| c.owner == self.owner && c.kind == hirsel_syntax::ContractKind::Ensures)
            .cloned()
            .collect();
        for clause in clauses {
            self.invoke_clause(&clause)?;
        }
        Ok(())
    }

    pub fn source_block(
        &mut self,
        id: ExprId,
        statements: Vec<StmtId>,
        tail: Option<ExprId>,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let parent = self.scope;
        let scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = scope;
        if let Some(target) = self.blocks.last_mut().filter(|b| b.block == id) {
            target.scope = Some(scope);
        }
        for statement in statements {
            if self.current.is_none() {
                break;
            }
            let old_temps = self.state.live.clone();
            self.statement(statement, bodies)?;
            if self.current.is_some() {
                let temps: BTreeSet<_> = self
                    .state
                    .live
                    .added_since(&old_temps)
                    .into_iter()
                    .filter(|root| {
                        self.body.locals[*root as usize].storage == Storage::Temporary
                            && !self.loan_supplies_root(*root)
                            && !self.named_view_holds_root(*root)
                    })
                    .collect();
                let cleanup = self.cleanup_for(&temps, &[]);
                for root in &temps {
                    self.state.live.remove(root);
                }
                self.emit(InstructionKind::RunCleanup(cleanup), span);
                self.retire_plain(&temps, span);
                self.end_unheld_loans(span);
            }
        }
        let result = if self.current.is_some() {
            let value = match tail {
                Some(e) => self.expression(e, bodies)?,
                None => Some(self.unit(span)),
            };
            if let Some(value) = value {
                let result = self.extract_block_value(value, scope, parent, span);
                self.exit_scope(scope, span);
                Some(result)
            } else {
                None
            }
        } else {
            None
        };
        self.scope = parent;
        Ok(result)
    }
    /// A block value is extracted into the enclosing scope `parent` before
    /// the owners of the block's `scope` are cleaned (also for Return and a
    /// break that leaves a labelled block). A borrowed value that names
    /// storage outliving the block is that borrow (spec issue #50): it needs no
    /// slot of its own, and a non-owning slot would have no storage-ending
    /// action. Leaves `self.scope` at `parent`.
    fn extract_block_value(
        &mut self,
        value: Value,
        scope: ScopeId,
        parent: ScopeId,
        span: Span,
    ) -> Value {
        self.scope = parent;
        let result = if !self.value_owned(value) && self.outlives(value.place, scope) {
            value
        } else {
            self.transport(value, Storage::Temporary, span)
        };
        {
            // A view of global content still holds access permissions. Its
            // type's empty locality does not make those permissions disposable.
            // Only the extracted content keeps permissions. Dead sibling
            // holders must release theirs in this scope, before their source
            // temporaries are destroyed by its cleanup.
            let retained = self.transitive_operand_loans(&Operand {
                place: result.place,
                usage: result.usage,
            });
            let moved: Vec<_> = self
                .state
                .loans
                .iter()
                .filter(|(loan, s)| self.within(*s, scope) && retained.contains(&loan.0))
                .map(|(loan, _)| loan.0)
                .collect();
            for (loan, s) in &mut self.state.loans {
                if moved.contains(&loan.0) {
                    *s = parent;
                }
            }
        }
        result
    }
    /// A labelled block (LANG-11) joins its tail and each break that leaves
    /// it in one result slot of the enclosing scope, as an `if` joins its
    /// branches.
    pub fn labelled_block(
        &mut self,
        id: ExprId,
        label: hirsel_intern::Symbol,
        statements: Vec<StmtId>,
        tail: Option<ExprId>,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let lent = self.lent_join(id)?;
        let destination = self.slot(ty, Storage::Temporary, span, false);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let exit = self.block(span);
        self.blocks.push(super::builder::LabelledBlock {
            block: id,
            label,
            destination,
            lent,
            outer: self.scope,
            scope: None,
            exit,
            exits: vec![],
        });
        let value = self.source_block(id, statements, tail, bodies)?;
        let mut states = self.blocks.pop().expect("lowered labelled block").exits;
        if let Some(value) = value {
            self.transfer_to(destination, value, lent, span);
            states.push(self.state.clone());
            self.terminate(Terminator::Goto(exit), span);
        }
        self.join_value(exit, destination, states, span)
    }
    /// `break 'l value`: the value leaves every scope up to the labelled
    /// block's, as its tail does, and joins its result (LANG-11).
    pub fn block_exit(
        &mut self,
        target: usize,
        value: Option<ExprId>,
        span: Span,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let value = match value {
            Some(e) => match self.expression(e, bodies)? {
                Some(v) => v,
                None => return Ok(None),
            },
            None => self.unit(span),
        };
        let current = self.scope;
        let block = &self.blocks[target];
        let (outer, destination, lent, exit) =
            (block.outer, block.destination, block.lent, block.exit);
        let Some(scope) = block.scope else {
            return self.error(span, "break before its labelled block's scope");
        };
        let value = self.extract_block_value(value, scope, outer, span);
        self.exit_scope(scope, span);
        self.transfer_to(destination, value, lent, span);
        self.blocks[target].exits.push(self.state.clone());
        self.terminate(Terminator::Goto(exit), span);
        self.scope = current;
        Ok(None)
    }
    pub fn statement(&mut self, id: StmtId, bodies: &mut Vec<Option<Body>>) -> Result<()> {
        let Some(statement) = self
            .input
            .syntax
            .tree
            .statements
            .get(id.0 as usize)
            .cloned()
        else {
            return self.error(Span::default(), "statement index out of range");
        };
        let span = statement.origin.span;
        match statement.kind {
            StmtKind::Let {
                pattern,
                value,
                otherwise,
                ..
            } => {
                // A taken value, and a copy of a computed value that ends
                // with this statement, make the binding a new owner (PROV-1).
                // Blocks are transparent (spec #50): `{take x}` is `take x`.
                let fresh = match self.input.syntax.tree.expressions
                    [self.transparent_value(value).0 as usize]
                    .kind
                {
                    hirsel_syntax::ExprKind::Unary {
                        op: hirsel_syntax::UnaryOp::Take,
                        ..
                    } => true,
                    hirsel_syntax::ExprKind::Unary {
                        op: hirsel_syntax::UnaryOp::Copy,
                        value: copied,
                    } => {
                        !self.names_outer_storage(copied)
                            && self
                                .decisions
                                .places
                                .get(&copied)
                                .iter()
                                .copied()
                                .any(|place| {
                                    place.expression == copied
                                        && matches!(place.root, hirsel_types::SourceRoot::Value(_))
                                })
                    }
                    _ => false,
                };
                let loans_before = self.loan_count;
                let discarded = matches!(
                    self.input.syntax.tree.patterns[pattern.0 as usize].kind,
                    hirsel_syntax::PatternKind::Wildcard
                );
                let outer = std::mem::replace(&mut self.keeping, !discarded);
                let outer_span = std::mem::replace(&mut self.keep_span, span);
                let value = self.expression(value, bodies);
                self.keeping = outer;
                self.keep_span = outer_span;
                if let Some(mut value) = value? {
                    // Owned initializers need staging for transfers and simultaneous
                    // field introductions. A borrowed initializer is already a
                    // source view; pattern binding must retain that place, rather
                    // than create a nonowning slot with no storage-ending action.
                    if self.value_owned(value) {
                        value = self.transport(value, Storage::Temporary, span);
                    } else {
                        let _ = self.loan(&mut value, span);
                    }
                    self.bind_refutable(pattern, value.place, fresh, otherwise, span, bodies)?;
                    if matches!(
                        self.input.syntax.tree.patterns[pattern.0 as usize].kind,
                        hirsel_syntax::PatternKind::Wildcard
                    ) {
                        // A discarded value has no consumer beyond this statement.
                        // End its evaluation loans before statement temporaries die
                        // and before a later operation accesses the source again.
                        let loans = self.new_loans_since(loans_before);
                        self.state.loans.retain(|(loan, _)| loan.0 < loans_before);
                        let cleanup = self.cleanup_for(&Default::default(), &loans);
                        self.emit(InstructionKind::RunCleanup(cleanup), span);
                    }
                }
            }
            StmtKind::Expr(value) => {
                if let Some(evaluated) = self.expression(value, bodies)? {
                    // A discarded scalar projection still evaluates its address:
                    // eliminating the value must not eliminate MEM-14 bounds (or
                    // an enclosing variant-tag check). A plain snapshot needs no
                    // counted copy or resource ownership.
                    let place = &self.body.places[evaluated.place.0 as usize];
                    if !place.projection.is_empty() && self.plain(self.ty(evaluated.place)) {
                        self.transport(evaluated, Storage::Temporary, span);
                    }
                }
            }
            StmtKind::Assign { place, op, value } => {
                let Some(destination) = self.expression(place, bodies)? else {
                    return Ok(());
                };
                // The address (including sequence index) precedes RHS effects.
                let loans_before = self.loan_count;
                // An existing binding cannot keep a temporary declared after it.
                let outer = std::mem::replace(&mut self.keeping, false);
                let value = self.expression(value, bodies);
                self.keeping = outer;
                let Some(value) = value? else {
                    return Ok(());
                };
                let value = self.transport(value, Storage::Temporary, span);
                let value = if let Some(op) = op.0 {
                    // The LHS read's mode is supplied by its checked expression.
                    let mut read = destination;
                    let loan = self.loan(&mut read, span);
                    let failure = self.cleanup();
                    let left = self.consume(read);
                    let right = self.consume(value);
                    let result = self.rvalue_with_failure(
                        self.ty(destination.place),
                        Rvalue::Binary { op, left, right },
                        span,
                        failure,
                    );
                    // The read loan covers calculation and its failure edge,
                    // then ends before replacing the same source storage.
                    if let Some(loan) = loan {
                        self.end_loan(Some(loan), span);
                    }
                    result
                } else {
                    value
                };
                let value = self.transport(value, Storage::Temporary, span);
                let mut rows = self
                    .decisions
                    .replacements
                    .get(&place)
                    .iter()
                    .copied()
                    .filter(|(expression, _, _)| *expression == place);
                let Some((_, content, live)) = rows.next() else {
                    return self.error(span, "missing owned replacement content type");
                };
                let content = *content;
                let live = *live;
                if rows.next().is_some() {
                    return self.error(span, "duplicate owned replacement content type");
                }
                if self.types.get(content).is_none() {
                    return self.error(span, "invalid owned replacement content type");
                }
                match live {
                    Some(true) => {
                        self.emit(InstructionKind::EndOrigin(destination.place), span);
                        self.replace_with_liveness(destination.place, value, content, false, span);
                    }
                    Some(false) => {
                        let operand = self.consume(value);
                        self.assign(destination.place, Rvalue::Use(operand), span);
                    }
                    None => {
                        self.replace_with_liveness(destination.place, value, content, true, span)
                    }
                }
                if live != Some(true)
                    && self.body.places[destination.place.0 as usize]
                        .binding
                        .is_some()
                {
                    // Whole owned replacement gives a fresh identity (PROV-1),
                    // also when the old value already moved out without a
                    // written take. Storage that may have been dead can only
                    // end its name once the new value is installed; the name's
                    // fresh introduction below then starts a new namespace
                    // instead of joining the old one's field identities.
                    self.emit(InstructionKind::EndOrigin(destination.place), span);
                }
                if !self
                    .types
                    .get(self.ty(value.place))
                    .unwrap()
                    .modes
                    .relation
                    .is_global()
                {
                    // A stored view is used in the receiving scope, even when
                    // its RHS call ran in a nested branch with a Unit result.
                    let home = self.body.locals[self.root(destination.place).0 as usize].scope;
                    for (loan, scope) in &mut self.state.loans {
                        if loan.0 >= loans_before {
                            *scope = home;
                        }
                    }
                }
                if let Some(origin) = self.body.places[destination.place.0 as usize]
                    .binding
                    .clone()
                {
                    self.bind_origin(destination.place, origin, OriginBindingKind::Fresh, span);
                }
            }
            StmtKind::While {
                condition, body, ..
            } => self.while_loop(id, condition, body, span, bodies)?,
            StmtKind::For {
                pattern,
                source,
                body,
                ..
            } => {
                self.decision(source)?;
                if let Some((_, plan)) = self
                    .decisions
                    .iterations
                    .get(&id)
                    .iter()
                    .copied()
                    .find(|(s, _)| *s == id)
                    .cloned()
                {
                    self.range_loop(id, pattern, plan, body, span, bodies)?;
                } else if let Some(cursor) =
                    self.decisions.cursors.get(&id).first().copied().cloned()
                {
                    self.protocol_loop(
                        id,
                        pattern,
                        source,
                        hirsel_check_protocol::body::ForPlanKind::Cursor { cursor },
                        body,
                        span,
                        bodies,
                    )?;
                } else if let Some((_, plan)) = self
                    .decisions
                    .protocols
                    .get(&id)
                    .iter()
                    .copied()
                    .find(|(s, _)| *s == id)
                    .cloned()
                {
                    self.protocol_loop(
                        id,
                        pattern,
                        source,
                        hirsel_check_protocol::body::ForPlanKind::Direct {
                            statement: id,
                            execution: plan,
                        },
                        body,
                        span,
                        bodies,
                    )?;
                } else {
                    return self.error(
                        span,
                        "for requires H's checked range/container/Iter protocol plan",
                    );
                }
            }
            StmtKind::Function { body, .. } => {
                let value = self.closure_for(body, body, NameSite::Statement(id), bodies)?;
                let Some(binding) = self
                    .decisions
                    .bindings
                    .get(&NameSite::Statement(id))
                    .iter()
                    .copied()
                    .find(|b| b.site == NameSite::Statement(id) && b.owner == self.owner)
                    .cloned()
                else {
                    return self.error(span, "missing local function binding row");
                };
                let mut value = self.transport(value, Storage::Local, span);
                if let Some(origin) =
                    Self::binding_origin(self.decisions.lexical, binding.owner, &binding.binding)
                {
                    value.place =
                        self.bind_origin(value.place, origin, OriginBindingKind::Alias, span);
                }
                self.bindings
                    .push((binding.owner, binding.binding, value.place));
            }
        }
        Ok(())
    }
    /// Whether every local a place names (its root and its index operands)
    /// belongs to a scope outside `scope`.
    fn outlives(&self, place: PlaceId, scope: ScopeId) -> bool {
        let place = &self.body.places[place.0 as usize];
        std::iter::once(place.root)
            .chain(place.projection.iter().filter_map(|p| match p {
                Projection::Index(local) => Some(*local),
                _ => None,
            }))
            .all(|local| !self.within(self.body.locals[local.0 as usize].scope, scope))
    }
    pub fn if_expression(
        &mut self,
        id: ExprId,
        condition: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let lent = self.lent_join(id)?;
        let Some(mut condition) = self.expression(condition, bodies)? else {
            return Ok(None);
        };
        let _ = self.loan(&mut condition, span);
        let condition = self.consume(condition);
        let yes = self.block(span);
        let no = self.block(span);
        let next = self.block(span);
        let destination = self.slot(ty, Storage::Temporary, span, false);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        self.terminate(Terminator::Branch { condition, yes, no }, span);
        let incoming = self.state.clone();
        let mut states = vec![];
        for (block, expression) in [(yes, Some(then_branch)), (no, else_branch)] {
            self.current = Some(block);
            self.state = incoming.clone();
            let value = match expression {
                Some(e) => self.expression(e, bodies)?,
                None => Some(self.unit(span)),
            };
            if let Some(value) = value {
                self.transfer_to(destination, value, lent, span);
                states.push(self.state.clone());
                self.terminate(Terminator::Goto(next), span);
            }
        }
        self.join_value(next, destination, states, span)
    }
    /// The scope that owns a branch's value when ownership judged the joined
    /// value a borrow (spec issue #50: a block, branch or arm is transparent to
    /// its consumer's use).
    fn lent_join(&self, id: ExprId) -> Result<Option<ScopeId>> {
        Ok((self.decision(id)?.usage == UseKind::Borrow).then_some(self.scope))
    }
    /// A borrowed view whose owning local this body owns, lives in a scope
    /// nested in `home` (so it ends first) and lends to nothing else.
    fn lends_ending_owner(&self, value: Value, home: ScopeId) -> bool {
        let root = self.root(value.place);
        value.usage == UseKind::Borrow
            && self.owned[root.0 as usize]
            && self.state.live.contains(&root.0)
            && self.body.locals[root.0 as usize].scope != home
            && self.within(self.body.locals[root.0 as usize].scope, home)
            && !self.loan_supplies_root(root.0)
    }
    /// `lent`: the join is a borrowed view. A branch that computed an owned
    /// value keeps it in a temporary of that scope and lends it, so the
    /// destination is a view on every path and the owner is dropped with the
    /// consumer's other temporaries.
    pub fn transfer_to(
        &mut self,
        destination: PlaceId,
        mut value: Value,
        lent: Option<ScopeId>,
        span: Span,
    ) {
        if let Some(home) = lent.filter(|_| self.value_owned(value)) {
            let scope = std::mem::replace(&mut self.scope, home);
            value = self.transport(value, Storage::Temporary, span);
            value.usage = UseKind::Borrow;
            let _ = self.loan(&mut value, span);
            self.scope = scope;
        } else if let Some(home) = lent.filter(|home| self.lends_ending_owner(value, *home)) {
            // The view borrows an owner that ends before the join's scope,
            // such as an arm binding that took its payload from an owned
            // scrutinee. The owner moves to that scope and the view
            // re-projects from it, as an owned branch value is kept there.
            let place = self.body.places[value.place.0 as usize].clone();
            let scope = std::mem::replace(&mut self.scope, home);
            let owner = self.transport(
                Value {
                    checked_ty: None,
                    place: self.roots[place.root.0 as usize],
                    usage: UseKind::Take,
                },
                Storage::Temporary,
                span,
            );
            self.scope = scope;
            let root = self.root(owner.place);
            value.place = self.place(root, place.projection, place.ty);
        }
        let root = self.root(destination).0 as usize;
        self.receive_ownership(root, self.value_owned(value));
        let _ = self.loan(&mut value, span);
        if let Some(home) = lent {
            let held = self.operand_loans(&Operand {
                place: value.place,
                usage: value.usage,
            });
            let promoted: BTreeSet<_> = self
                .state
                .loans
                .iter()
                .filter(|(loan, scope)| {
                    self.within(*scope, home)
                        && held.iter().any(|(_, loans)| loans.contains(&loan.0))
                })
                .map(|(loan, _)| loan.0)
                .collect();
            for (loan, scope) in &mut self.state.loans {
                if promoted.contains(&loan.0) {
                    *scope = home;
                }
            }
        }
        let failure = self.cleanup();
        let operand = self.consume(value);
        let rv = RvalueId(self.body.rvalues.len() as u32);
        self.retain_rvalue(destination, &Rvalue::Use(operand.clone()));
        self.body.rvalues.push(Rvalue::Use(operand));
        self.emit_with(
            InstructionKind::Assign {
                destination,
                value: rv,
            },
            span,
            failure,
        );
        self.state.live.insert(self.root(destination).0);
    }
    fn join_value(
        &mut self,
        next: BlockId,
        destination: PlaceId,
        states: Vec<State>,
        _span: Span,
    ) -> Result<Option<Value>> {
        if states.is_empty() {
            self.current = None;
            return Ok(None);
        }
        self.state = self.merge(&states);
        self.current = Some(next);
        Ok(Some(Value {
            checked_ty: None,
            place: destination,
            usage: UseKind::Take,
        }))
    }
    pub fn short_circuit(
        &mut self,
        id: ExprId,
        left: ExprId,
        right: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let is_and = matches!(
            self.input.syntax.tree.expressions[id.0 as usize].kind,
            ExprKind::Binary {
                op: BinaryOp::And,
                ..
            }
        );
        let Some(mut left) = self.expression(left, bodies)? else {
            return Ok(None);
        };
        let _ = self.loan(&mut left, span);
        let condition = self.consume(left);
        let evaluate = self.block(span);
        let constant = self.block(span);
        let next = self.block(span);
        let destination = self.slot(ty, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        self.terminate(
            Terminator::Branch {
                condition,
                yes: if is_and { evaluate } else { constant },
                no: if is_and { constant } else { evaluate },
            },
            span,
        );
        let incoming = self.state.clone();
        let mut states = vec![];
        self.current = Some(constant);
        self.assign(destination, Rvalue::Constant(Constant::Bool(!is_and)), span);
        states.push(self.state.clone());
        self.terminate(Terminator::Goto(next), span);
        self.state = incoming;
        self.current = Some(evaluate);
        if let Some(value) = self.expression(right, bodies)? {
            self.transfer_to(destination, value, None, span);
            states.push(self.state.clone());
            self.terminate(Terminator::Goto(next), span);
        }
        self.join_value(next, destination, states, span)
    }
    pub fn while_loop(
        &mut self,
        statement: StmtId,
        condition: ExprId,
        body: ExprId,
        span: Span,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        let header = self.block(span);
        let run = self.block(span);
        let exit = self.block(span);
        self.terminate(Terminator::Goto(header), span);
        self.revive_in_loop(span);
        let incoming = self.state.clone();
        self.current = Some(header);
        self.publish_identity_joins(&NameSite::Statement(statement), span)?;
        let parent = self.scope;
        let scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = scope;
        self.loops.push(Loop {
            statement,
            next: header,
            exit,
            scope,
            exits: vec![],
            feedback: vec![],
        });
        let Some(mut value) = self.expression(condition, bodies)? else {
            self.scope = parent;
            let lp = self.loops.pop().unwrap();
            if !lp.exits.is_empty() {
                self.state = self.merge(&lp.exits);
                self.current = Some(exit);
            }
            return Ok(());
        };
        // The branch consumes its condition after the evaluation scope ends.
        // Move those Plain bits into the enclosing lifetime first.
        self.scope = parent;
        value = self.transport(value, Storage::Temporary, span);
        self.scope = scope;
        let _ = self.loan(&mut value, span);
        let operand = self.consume(value);
        // Preserve the consumed condition storage until Branch reads it, while
        // excluding it from cleanup, and close condition evaluation temps.
        self.exit_scope(scope, span);
        self.terminate(
            Terminator::Branch {
                condition: operand,
                yes: run,
                no: exit,
            },
            span,
        );
        let condition_exit = self.state.clone();
        self.loops.last_mut().unwrap().exits.push(condition_exit);
        self.current = Some(run);
        self.state = incoming;
        self.expression(body, bodies)?;
        if self.current.is_some() {
            self.exit_scope(scope, span);
            self.loops
                .last_mut()
                .unwrap()
                .feedback
                .push(self.state.clone());
            self.terminate(Terminator::Goto(header), span);
        }
        let mut lp = self.loops.pop().unwrap();
        lp.exits.extend(lp.feedback);
        self.scope = parent;
        self.state = self.merge(&lp.exits);
        self.current = Some(exit);
        Ok(())
    }
    /// Lower a match arm's body into the match result and continue at the join.
    #[allow(clippy::too_many_arguments)]
    fn arm_body(
        &mut self,
        body: ExprId,
        destination: PlaceId,
        ty: TypeId,
        lent: Option<ScopeId>,
        scope: ScopeId,
        parent: ScopeId,
        states: &mut Vec<super::builder::State>,
        next: &mut Option<BlockId>,
        bodies: &mut Vec<Option<Body>>,
        span: Span,
    ) -> Result<()> {
        if let Some(result) = self.expression(body, bodies)? {
            self.transfer_to(destination, result, lent, span);
            if !self.types.get(ty).unwrap().modes.relation.is_global() {
                for (_, loan_scope) in &mut self.state.loans {
                    if *loan_scope == scope {
                        *loan_scope = parent;
                    }
                }
            }
            self.exit_scope(scope, span);
            states.push(self.state.clone());
            let join = *next.get_or_insert_with(|| self.block(span));
            self.terminate(Terminator::Goto(join), span);
        }
        Ok(())
    }
    pub fn match_expression(
        &mut self,
        id: ExprId,
        value: ExprId,
        arms: Vec<SourceMatchArm>,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let lent = self.lent_join(id)?;
        let Some(value) = self.expression(value, bodies)? else {
            return Ok(None);
        };
        let value = self.transport(value, Storage::Temporary, span);
        let destination = self.slot(ty, Storage::Temporary, span, false);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let mut next = None;
        let incoming = self.state.clone();
        let mut states = vec![];
        let table_id = MatchTableId(self.body.matches.len() as u32);
        let mut table = MatchTable {
            scrutinee: value.place,
            patterns: vec![],
            arms: vec![],
        };
        self.body.matches.push(table.clone());
        self.terminate(
            Terminator::Match {
                table: table_id,
                from: 0,
            },
            span,
        );
        for arm in arms {
            let Some(choices) = self.input.syntax.tree.pattern_choices(arm.pattern) else {
                return self.error(span, "unchecked or-pattern expansion");
            };
            // LANG-13: each way the arm's alternatives match is its own table
            // arm. An arm that commits nothing shares one body; one that binds
            // lowers its body once per way, each with that way's bindings.
            let shared =
                (choices.len() > 1 && !self.pattern_commits(arm.pattern)).then(|| self.block(span));
            let mut shared_state = None;
            for choice in &choices {
                let from = table.arms.len() as u32 + 1;
                let hit = match (shared, arm.guard) {
                    (Some(body), None) => body,
                    _ => self.block(span),
                };
                let target = match (shared, arm.guard) {
                    (Some(body), _) => body,
                    (None, Some(_)) => self.block(span),
                    (None, None) => hit,
                };
                let pattern = self.table_pattern(
                    arm.pattern,
                    value.place,
                    choice,
                    &mut table.patterns,
                    arm.guard.is_some(),
                )?;
                table.arms.push(hirsel_tir::MatchArm {
                    pattern,
                    guard: arm.guard.map(|_| hit),
                    target,
                });
                if shared.is_some() && arm.guard.is_none() {
                    continue;
                }
                self.current = Some(hit);
                self.state = incoming.clone();
                let parent = self.scope;
                let scope = ScopeId(self.body.scopes.len() as u32);
                self.body.scopes.push(Scope {
                    parent: Some(parent),
                    span,
                });
                self.scope = scope;
                let binding_mark = self.bindings.len();
                if arm.guard.is_some() {
                    self.guard_pattern(arm.pattern, value.place, choice, bodies, true)?;
                } else {
                    self.bind_pattern(arm.pattern, value.place, choice, bodies)?;
                }
                if let Some(guard) = arm.guard {
                    let Some(mut guard) = self.expression(guard, bodies)? else {
                        return self.error(span, "checked guard diverges");
                    };
                    let _ = self.loan(&mut guard, span);
                    let condition = self.consume(guard);
                    let yes = self.block(span);
                    let no = self.block(span);
                    self.terminate(Terminator::Branch { condition, yes, no }, span);
                    let guard_state = self.state.clone();
                    self.current = Some(no);
                    self.exit_scope(scope, span);
                    self.terminate(
                        Terminator::Match {
                            table: table_id,
                            from,
                        },
                        span,
                    );
                    self.current = Some(yes);
                    self.state = guard_state;
                    self.exit_scope(scope, span);
                    self.terminate(Terminator::Goto(target), span);
                    if shared.is_some() {
                        shared_state.get_or_insert_with(|| self.state.clone());
                        self.bindings.truncate(binding_mark);
                        self.scope = parent;
                        continue;
                    }
                    self.current = Some(target);
                    self.guard_pattern(arm.pattern, value.place, choice, bodies, false)?;
                }
                self.arm_body(
                    arm.body,
                    destination,
                    ty,
                    lent,
                    scope,
                    parent,
                    &mut states,
                    &mut next,
                    bodies,
                    span,
                )?;
                self.bindings.truncate(binding_mark);
                self.scope = parent;
            }
            if let Some(body) = shared {
                self.current = Some(body);
                self.state = shared_state.unwrap_or_else(|| incoming.clone());
                let parent = self.scope;
                let scope = ScopeId(self.body.scopes.len() as u32);
                self.body.scopes.push(Scope {
                    parent: Some(parent),
                    span,
                });
                self.scope = scope;
                self.arm_body(
                    arm.body,
                    destination,
                    ty,
                    lent,
                    scope,
                    parent,
                    &mut states,
                    &mut next,
                    bodies,
                    span,
                )?;
                self.scope = parent;
            }
        }
        self.body.matches[table_id.0 as usize] = table;
        if let Some(next) = next {
            self.join_value(next, destination, states, span)
        } else {
            self.current = None;
            Ok(None)
        }
    }
    pub fn propagate(
        &mut self,
        id: ExprId,
        expression: ExprId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<Option<Value>> {
        let span = self.span(id)?;
        let ty = self.decision(id)?.ty;
        let Some((_, plan)) = self
            .decisions
            .propagation
            .get(&id)
            .iter()
            .copied()
            .find(|(e, _)| *e == id)
            .cloned()
        else {
            return self.error(span, "missing nominal Result propagation plan");
        };
        let Some(value) = self.expression(expression, bodies)? else {
            return Ok(None);
        };
        let value = self.transport(value, Storage::Temporary, span);
        let carrier = self.body.places[value.place.0 as usize].ty;
        let yes = self.block(span);
        let no = self.block(span);
        let bad = self.block(span);
        // Switch reads the discriminator non-destructively. This is an internal
        // IR tag observation, not a source borrow judgment.
        let mut tag = Value {
            checked_ty: None,
            place: value.place,
            usage: UseKind::Borrow,
        };
        let tag_loan = self.loan(&mut tag, span);
        if let Some(loan) = tag_loan {
            self.state.loans.retain(|(id, _)| *id != loan);
        }
        let switch = Operand {
            place: tag.place,
            usage: UseKind::Borrow,
        };
        self.terminate(
            Terminator::Switch {
                value: switch,
                arms: vec![(plan.success_variant, yes), (plan.error_variant, no)],
                otherwise: bad,
            },
            span,
        );
        let incoming = self.state.clone();
        for block in [bad, no, yes] {
            self.current = Some(block);
            self.state = incoming.clone();
            // Both Result tags are covered. Keep the impossible default empty;
            // its observation loan is ended only on executable successors.
            if block != bad {
                self.end_loan(tag_loan, span);
            }
            if block == bad {
                self.terminate(Terminator::Unreachable, span);
            } else if block == no {
                let ty = self.part_type(carrier, plan.error_type);
                let error = self.project(
                    value.place,
                    Projection::VariantField {
                        variant: plan.error_variant,
                        field: 0,
                    },
                    ty,
                );
                let mut error = Value {
                    checked_ty: None,
                    place: error,
                    usage: UseKind::Take,
                };
                if let Some(mapper) = &plan.mapper {
                    let (mapper_result, constructor) = match mapper {
                        ErrorMapper::Definition { result, .. } => (*result, None),
                        ErrorMapper::Constructor {
                            result,
                            constructor,
                        } => (*result, Some(constructor.clone())),
                    };
                    error = self.transport(error, Storage::Temporary, span);
                    if let Some(constructor) = constructor {
                        let failure = self.cleanup();
                        let value = self.consume(error);
                        error = self.rvalue_with_failure(
                            mapper_result,
                            Rvalue::Construct {
                                constructor,
                                fields: vec![ConstructField { field: 0, value }],
                                destination: self.row_destination(mapper_result),
                            },
                            span,
                            failure,
                        );
                    } else {
                        let ErrorMapper::Definition { path, passing, .. } = mapper else {
                            unreachable!()
                        };
                        error.usage = match passing {
                            hirsel_syntax::PassingMode::Borrow => UseKind::Borrow,
                            hirsel_syntax::PassingMode::Take => UseKind::Take,
                            hirsel_syntax::PassingMode::Mut => {
                                return Err(LowerError {
                                    site: span,
                                    message: "mut error mapper was not rejected".into(),
                                });
                            }
                        };
                        let loan = self.loan(&mut error, span);
                        let arg = self.consume(error);
                        let destination = self.slot(mapper_result, Storage::Temporary, span, true);
                        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
                        let next = self.block(span);
                        let on_failure = self.cleanup();
                        let generics = self
                            .decisions
                            .instantiations
                            .get(&id)
                            .iter()
                            .copied()
                            .find(|i| i.expression == id && &i.callee == path)
                            .map(|i| i.arguments.clone())
                            .unwrap_or_default();
                        self.terminate(
                            Terminator::Call {
                                callee: Callee::Definition {
                                    path: path.clone(),
                                    arguments: generics,
                                },
                                arguments: vec![CallArgument {
                                    parameter: hirsel_types::ParamId(0),
                                    value: arg,
                                }],
                                returns: Some(CallReturn {
                                    destination: destination,
                                    next: next,
                                }),
                                on_failure,
                            },
                            span,
                        );
                        self.current = Some(next);
                        self.state.live.insert(self.root(destination).0);
                        self.end_loan(loan, span);
                        error = Value {
                            checked_ty: None,
                            place: destination,
                            usage: UseKind::Take,
                        };
                    }
                }
                // As `return Error(e)`: the propagated Result is as local as
                // its error, and returning it checks that against the result.
                let mut propagated = self.types.get(self.body.result).unwrap().clone();
                propagated.modes.relation = self
                    .types
                    .get(self.body.places[error.place.0 as usize].ty)
                    .unwrap()
                    .modes
                    .relation
                    .clone();
                let propagated = if propagated.modes.relation.is_global() {
                    self.body.result
                } else {
                    self.types.intern(propagated)
                };
                let failure = self.cleanup();
                let operand = self.consume(error);
                let rv = Rvalue::Construct {
                    constructor: plan.result_constructor.clone(),
                    fields: vec![ConstructField {
                        field: 0,
                        value: operand,
                    }],
                    destination: self.row_destination(propagated),
                };
                let result = self.rvalue_with_failure(propagated, rv, span, failure);
                self.return_value(result, span, bodies)?;
            }
        }
        let success = self.part_type(carrier, plan.success_type);
        let payload = self.project(
            value.place,
            Projection::VariantField {
                variant: plan.success_variant,
                field: 0,
            },
            success,
        );
        Ok(Some(self.transport_typed(
            Value {
                checked_ty: None,
                place: payload,
                usage: UseKind::Take,
            },
            ty,
            Storage::Temporary,
            span,
        )))
    }
}

impl Builder<'_, '_> {
    /// Both the borrowed base and boolean result convention come from N's plan.
    pub fn fact_check(
        &mut self,
        fact: &hirsel_check_protocol::body::FactPlan,
        base: Value,
        span: Span,
    ) -> Result<()> {
        let loans_before = self.loan_count;
        let mut argument = Value {
            checked_ty: None,
            place: base.place,
            usage: fact.usage,
        };
        let _ = self.loan(&mut argument, span);
        let destination = self.slot(fact.result, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let next = self.block(span);
        let on_failure = self.cleanup();
        self.terminate(
            Terminator::Call {
                callee: Callee::Definition {
                    path: fact.predicate.clone(),
                    arguments: fact.arguments.clone(),
                },
                arguments: vec![CallArgument {
                    parameter: hirsel_types::ParamId(0),
                    value: Operand {
                        place: argument.place,
                        usage: fact.usage,
                    },
                }],
                returns: Some(CallReturn {
                    destination: destination,
                    next: next,
                }),
                on_failure,
            },
            span,
        );
        self.current = Some(next);
        self.state.live.insert(self.root(destination).0);
        let loans = self.new_loans_since(loans_before);
        self.state.loans.retain(|(loan, _)| loan.0 < loans_before);
        let cleanup = self.cleanup_for(&std::collections::BTreeSet::new(), &loans);
        self.emit(InstructionKind::RunCleanup(cleanup), span);
        let failure = self.cleanup();
        let condition = self.consume(Value {
            checked_ty: None,
            place: destination,
            usage: UseKind::Take,
        });
        self.emit_with(
            InstructionKind::Check {
                condition,
                rule: const { hirsel_diag::RuleId::of("FACT-2") },
            },
            span,
            failure,
        );
        Ok(())
    }
}
