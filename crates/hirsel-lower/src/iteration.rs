//! Integer ranges use checked scalar CFG operations. The plan is H's selection;
//! counter copies and literals are compiler mechanics, not source mode choices.
use super::builder::{Builder, Loop, Value};
use super::*;
use hirsel_syntax::{BinaryOp, PatternId};
use hirsel_tir::*;
use hirsel_types::UseKind;
impl Builder<'_, '_> {
    pub fn range_loop(
        &mut self,
        statement: StmtId,
        pattern: PatternId,
        plan: RangePlan,
        expression: ExprId,
        span: Span,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        let Some(start) = self.expression(plan.start, bodies)? else {
            return Ok(());
        };
        let counter = self.transport(start, Storage::Temporary, span);
        let Some(end) = self.expression(plan.end, bodies)? else {
            return Ok(());
        };
        let end = self.transport(end, Storage::Temporary, span);
        let ty = self.ty(counter.place);
        if !hirsel_types::semantic_equal(&self.types, ty, &self.types, self.ty(end.place))
            .unwrap_or(false)
        {
            return self.error(span, "range endpoints have inconsistent checked types");
        }
        let header = self.block(span);
        let run = self.block(span);
        let step = self.block(span);
        let exit = self.block(span);
        self.terminate(Terminator::Goto(header), span);
        self.revive_in_loop(span);
        let incoming = self.state.clone();
        self.current = Some(header);
        self.publish_identity_joins(&NameSite::Statement(statement), span)?;
        let comparison = self.rvalue(
            self.input.boolean,
            Rvalue::Binary {
                op: if plan.inclusive {
                    BinaryOp::Le
                } else {
                    BinaryOp::Lt
                },
                left: Operand {
                    place: counter.place,
                    usage: UseKind::Copy,
                },
                right: Operand {
                    place: end.place,
                    usage: UseKind::Copy,
                },
            },
            span,
        );
        let condition = self.consume(comparison);
        self.terminate(
            Terminator::Branch {
                condition,
                yes: run,
                no: exit,
            },
            span,
        );
        let parent = self.scope;
        let scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = scope;
        self.loops.push(Loop {
            statement,
            next: step,
            exit,
            scope,
            exits: vec![incoming.clone()],
            feedback: vec![],
        });
        self.current = Some(run);
        self.state = incoming.clone();
        let item = self.transport(
            Value {
                checked_ty: None,
                place: counter.place,
                usage: UseKind::Copy,
            },
            Storage::Temporary,
            span,
        );
        self.bind_refutable(pattern, item.place, true, None, span, bodies)?;
        self.expression(expression, bodies)?;
        if self.current.is_some() {
            self.exit_scope(scope, span);
            self.loops
                .last_mut()
                .unwrap()
                .feedback
                .push(self.state.clone());
            self.terminate(Terminator::Goto(step), span);
        }
        self.scope = parent;
        self.state = incoming.clone();
        if self
            .body
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Goto(target) if target == step))
        {
            self.current = Some(step);
            // Inclusive ranges stop *before* incrementing their final item. In
            // particular MAX..=MAX never evaluates overflowing MAX+1. The header
            // proved `counter <= end`, so `counter < end` continues exactly when
            // `counter == end` would exit, and it also proves the header's next
            // test, `counter + 1 <= end`, so a loop tests its bound once per
            // item rather than twice.
            if plan.inclusive {
                let compare = self.rvalue(
                    self.input.boolean,
                    Rvalue::Binary {
                        op: BinaryOp::Lt,
                        left: Operand {
                            place: counter.place,
                            usage: UseKind::Copy,
                        },
                        right: Operand {
                            place: end.place,
                            usage: UseKind::Copy,
                        },
                    },
                    span,
                );
                let condition = self.consume(compare);
                let increment = self.block(span);
                self.terminate(
                    Terminator::Branch {
                        condition,
                        yes: increment,
                        no: exit,
                    },
                    span,
                );
                self.current = Some(increment);
            }
            let one = self.rvalue(ty, Rvalue::Constant(Constant::Integer(1)), span);
            let failure = self.cleanup();
            let right = self.consume(one);
            let value = self.rvalue_with_failure(
                ty,
                Rvalue::Binary {
                    op: BinaryOp::Add,
                    left: Operand {
                        place: counter.place,
                        usage: UseKind::Copy,
                    },
                    right,
                },
                span,
                failure,
            );
            self.replace(counter.place, value, self.ty(counter.place), span);
            self.terminate(Terminator::Goto(header), span);
        }
        let mut lp = self.loops.pop().unwrap();
        lp.exits.extend(lp.feedback);
        self.state = self.merge(&lp.exits);
        self.current = Some(exit);
        Ok(())
    }
}

impl Builder<'_, '_> {
    fn protocol_call(
        &mut self,
        source: ExprId,
        definition: DefPath,
        receiver: Value,
        result: TypeId,
        retain_receiver: bool,
        span: Span,
    ) -> Result<Value> {
        self.protocol_call_index(
            source,
            definition,
            receiver,
            result,
            retain_receiver,
            None,
            span,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn protocol_call_index(
        &mut self,
        source: ExprId,
        definition: DefPath,
        mut receiver: Value,
        result: TypeId,
        retain_receiver: bool,
        index: Option<Value>,
        span: Span,
    ) -> Result<Value> {
        let actuals = self
            .decisions
            .instantiations
            .get(&source)
            .iter()
            .copied()
            .find(|i| i.expression == source && i.callee == definition)
            .ok_or_else(|| LowerError {
                site: span,
                message: "loop operation lacks H's checked instantiation".into(),
            })?
            .arguments
            .clone();
        let loans_before = self.loan_count;
        let _ = self.loan(&mut receiver, span);
        let operand = self.consume(receiver);
        let held = self.operand_loans(&operand);
        let destination = self.slot(result, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(destination)), span);
        let next = self.block(span);
        let on_failure = self.cleanup();
        let mut arguments = vec![CallArgument {
            parameter: hirsel_types::ParamId(0),
            value: operand,
        }];
        if let Some(index) = index {
            arguments.push(CallArgument {
                parameter: hirsel_types::ParamId(1),
                value: self.consume(index),
            });
        }
        self.terminate(
            Terminator::Call {
                callee: Callee::Definition {
                    path: definition,
                    arguments: actuals,
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
        if retain_receiver {
            self.borrowed_cursor_types.insert(result);
        }
        if retain_receiver || !self.types.get(result).unwrap().modes.relation.is_global() {
            self.call_retention
                .insert(destination.0, ([0].into(), false, false));
            self.set_held_loans(destination, held);
        }
        if !retain_receiver && self.types.get(result).unwrap().modes.relation.is_global() {
            self.end_new_loans(loans_before, span);
        }
        Ok(Value {
            checked_ty: None,
            place: destination,
            usage: UseKind::Take,
        })
    }
    pub fn protocol_loop(
        &mut self,
        statement: StmtId,
        pattern: PatternId,
        source_id: ExprId,
        plan: hirsel_check_protocol::body::ForPlanKind,
        expression: ExprId,
        span: Span,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        use hirsel_check_protocol::body::{ForExecution, ForPlanKind, LoopLength, LoopRead};
        // Prepared views, cursor owners and their source loans last for this
        // loop, enclosing the shorter lifetime of each yielded item.
        let enclosing = self.scope;
        let preparation = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(enclosing),
            span,
        });
        self.scope = preparation;
        let Some(mut source) = self.expression(source_id, bodies)? else {
            self.scope = enclosing;
            return Ok(());
        };
        source.usage = self.decision(source_id)?.usage;
        if matches!(source.usage, UseKind::Take | UseKind::Copy) {
            source = self.transport(source, Storage::Temporary, span);
        }
        let plan = match plan {
            ForPlanKind::Direct { execution, .. } => execution,
            ForPlanKind::Cursor { cursor } => {
                if self.decisions.cursors.get(&statement).len() != 1
                    || cursor.source != source_id
                    || [cursor.cursor, cursor.step.result, cursor.step.item]
                        .iter()
                        .any(|id| self.types.get(*id).is_none())
                    || !matches!(cursor.source_use, UseKind::Borrow | UseKind::Take)
                {
                    return self.error(span, "invalid or duplicate cursor plan");
                }
                source = self.protocol_call(
                    source_id,
                    cursor.open,
                    Value {
                        checked_ty: None,
                        place: source.place,
                        usage: cursor.source_use,
                    },
                    cursor.cursor,
                    cursor.source_use == UseKind::Borrow,
                    span,
                )?;
                ForExecution::Advance {
                    operation: cursor.next,
                    step: cursor.step,
                }
            }
        };
        let indexed = if let ForExecution::Indexed { length, read } = &plan {
            // One whole-source loan protects the entry positions until loop exit.
            let _ = self.loan(&mut source, span);
            self.retain_view(source.place);
            let index_type = match length {
                LoopLength::Definition { result, .. } | LoopLength::Inline { result, .. } => {
                    *result
                }
            };
            let length = match length {
                LoopLength::Definition { definition, result } => self.protocol_call(
                    source_id,
                    definition.clone(),
                    Value {
                        checked_ty: None,
                        place: source.place,
                        usage: UseKind::Borrow,
                    },
                    *result,
                    false,
                    span,
                )?,
                LoopLength::Inline {
                    size: hirsel_types::Size::Constant(n),
                    ..
                } => self.rvalue(
                    index_type,
                    Rvalue::Constant(Constant::Integer(*n as u128)),
                    span,
                ),
                // A size parameter reads its instantiation's actual, as a
                // source expression naming it does.
                LoopLength::Inline {
                    size: hirsel_types::Size::Parameter(parameter),
                    ..
                } => self.rvalue(index_type, Rvalue::Size(*parameter), span),
            };
            let zero = self.rvalue(index_type, Rvalue::Constant(Constant::Integer(0)), span);
            let result = match read {
                LoopRead::Index { result } | LoopRead::MapIndex { result, .. } => *result,
            };
            Some((length, zero, result))
        } else {
            // Cursor storage is used again at the next loop step. It remains an
            // independent holder even when its current item permission ends.
            self.retain_view(source.place);
            None
        };
        self.revive_in_loop(span);
        let incoming = self.state.clone();
        let header = self.block(span);
        let run = self.block(span);
        let step = self.block(span);
        let exit = self.block(span);
        self.terminate(Terminator::Goto(header), span);
        let parent = self.scope;
        let scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = scope;
        self.loops.push(Loop {
            statement,
            next: step,
            exit,
            scope,
            exits: vec![incoming.clone()],
            feedback: vec![],
        });
        self.current = Some(header);
        self.publish_identity_joins(&NameSite::Statement(statement), span)?;
        self.state = incoming.clone();
        let item = if let Some((length, counter, result)) = indexed {
            if let ForExecution::Indexed {
                read: LoopRead::MapIndex { advance, .. },
                ..
            } = &plan
            {
                let index_type = self.ty(counter.place);
                let next = self.protocol_call_index(
                    source_id,
                    advance.clone(),
                    Value {
                        checked_ty: None,
                        place: source.place,
                        usage: UseKind::Borrow,
                    },
                    index_type,
                    false,
                    Some(Value {
                        checked_ty: None,
                        place: counter.place,
                        usage: UseKind::Copy,
                    }),
                    span,
                )?;
                self.replace(counter.place, next, index_type, span);
            }
            let comparison = self.rvalue(
                self.input.boolean,
                Rvalue::Binary {
                    op: BinaryOp::Lt,
                    left: Operand {
                        place: counter.place,
                        usage: UseKind::Copy,
                    },
                    right: Operand {
                        place: length.place,
                        usage: UseKind::Copy,
                    },
                },
                span,
            );
            let condition = self.consume(comparison);
            let miss = self.block(span);
            self.terminate(
                Terminator::Branch {
                    condition,
                    yes: run,
                    no: miss,
                },
                span,
            );
            self.current = Some(miss);
            self.exit_scope(scope, span);
            self.terminate(Terminator::Goto(exit), span);
            self.current = Some(run);
            self.state = incoming.clone();
            self.project(
                source.place,
                Projection::Index(self.root(counter.place)),
                result,
            )
        } else if let ForExecution::Advance {
            operation: definition,
            step: layout,
        } = &plan
        {
            // H selected the core Iter.next protocol: its receiver is mutable;
            // this mechanical call does not authorize a source move.
            let next = self.protocol_call(
                source_id,
                definition.clone(),
                Value {
                    checked_ty: None,
                    place: source.place,
                    usage: UseKind::Mut,
                },
                layout.result,
                false,
                span,
            )?;
            let mut tag = Value {
                checked_ty: None,
                place: next.place,
                usage: UseKind::Borrow,
            };
            let loan = self.loan(&mut tag, span);
            let hit = self.block(span);
            let miss = self.block(span);
            let fallback = self.block(span);
            self.terminate(
                Terminator::Switch {
                    value: Operand {
                        place: tag.place,
                        usage: UseKind::Borrow,
                    },
                    arms: vec![(layout.some_variant.0, hit), (layout.none_variant.0, miss)],
                    otherwise: fallback,
                },
                span,
            );
            let after_call = self.state.clone();
            for target in [miss, fallback] {
                self.current = Some(target);
                self.state = after_call.clone();
                self.end_loan(loan, span);
                self.exit_scope(scope, span);
                self.terminate(Terminator::Goto(exit), span);
            }
            self.current = Some(hit);
            self.state = after_call;
            self.end_loan(loan, span);
            self.terminate(Terminator::Goto(run), span);
            self.current = Some(run);
            self.project(
                next.place,
                Projection::VariantField {
                    variant: layout.some_variant.0,
                    field: 0,
                },
                layout.item,
            )
        } else {
            return self.error(span, "loop advance has no executable checked operation");
        };
        // The yielded value is named anew on each iteration. In particular,
        // an indexed element has no whole-container identity to alias.
        self.bind_refutable(pattern, item, true, None, span, bodies)?;
        self.expression(expression, bodies)?;
        if self.current.is_some() {
            self.exit_scope(scope, span);
            self.loops
                .last_mut()
                .unwrap()
                .feedback
                .push(self.state.clone());
            self.terminate(Terminator::Goto(step), span);
        }
        self.scope = parent;
        if self
            .body
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Goto(target) if target == step))
        {
            self.current = Some(step);
            self.state = incoming.clone();
            if let Some((_, counter, _)) = indexed {
                let index_type = self.ty(counter.place);
                let one = self.rvalue(index_type, Rvalue::Constant(Constant::Integer(1)), span);
                let right = self.consume(one);
                let increment = self.rvalue(
                    index_type,
                    Rvalue::Binary {
                        op: BinaryOp::Add,
                        left: Operand {
                            place: counter.place,
                            usage: UseKind::Copy,
                        },
                        right,
                    },
                    span,
                );
                self.replace(counter.place, increment, index_type, span);
            }
            self.terminate(Terminator::Goto(header), span);
        }
        let mut lp = self.loops.pop().unwrap();
        lp.exits.extend(lp.feedback);
        self.state = self.merge(&lp.exits);
        self.current = Some(exit);
        self.exit_scope(preparation, span);
        self.scope = enclosing;
        Ok(())
    }
}
