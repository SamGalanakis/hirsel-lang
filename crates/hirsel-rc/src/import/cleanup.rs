use super::*;

impl Importer<'_> {
    // --- failure successors and cleanup chains ---

    /// The TIR actions of a cleanup chain, in execution order.
    pub(super) fn chain_actions(&self, cleanup: tir::CleanupId) -> R<Vec<Action>> {
        let Some(c) = self.body.cleanups.get(cleanup.0 as usize) else {
            return malformed(format!("cleanup {} outside the body", cleanup.0));
        };
        let mut out = Vec::new();
        let mut next = c.head;
        while let Some(node) = next {
            out.push(Action::Tir { node: node.0 });
            next = self.body.cleanup_nodes[node.0 as usize].parent;
        }
        Ok(out)
    }

    /// An exit's actions: the TIR chain, with the internal loop owners of
    /// borrowed parameters released where parameters are (after inner scopes
    /// and temporaries, before the parameter-scope names).
    pub(super) fn exit_actions(&self, mut actions: Vec<Action>) -> Vec<Action> {
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        let mut releases = self
            .out
            .tables
            .receipt_entry
            .as_ref()
            .into_iter()
            .flat_map(|entry| &entry.parameters)
            .filter(|binding| {
                abi.receipts.as_ref().is_some_and(|contract| {
                    contract.parameters.iter().any(|p| {
                        p.source == binding.source
                            && p.mode == hirsel_lir::sealed_payload_source::ReceiptMode::Take
                    })
                })
            })
            .map(|binding| Action::Release {
                slot: binding.slot,
                anchor: (STATE_RELEASE, binding.slot.0),
            })
            .collect::<Vec<_>>();
        releases.extend(
            self.out
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| {
                    matches!(slot.kind, SlotKind::Local | SlotKind::Temporary)
                        && self.out.types[slot.ty.index()].repr == ValueRepr::PayloadReceipt
                })
                .map(|(index, _)| Action::Release {
                    slot: SlotId(index as u32),
                    anchor: (STATE_RELEASE, index as u32),
                }),
        );
        if let Some(plan) = self.plan {
            if let Some(h) = self.holes {
                releases.push(Action::Release {
                    slot: h.root,
                    anchor: (LOOP_RELEASE, plan.homes.len() as u32),
                });
            }
            for (k, home) in plan.homes.iter().enumerate().rev() {
                if *home == Home::Loop && abi.parameters[k].convention == Passing::Borrow {
                    let slot = self.loop_slots[k].expect("loop slot");
                    releases.push(Action::Release {
                        slot,
                        anchor: (LOOP_RELEASE, k as u32),
                    });
                }
            }
        }
        releases.sort_by_key(|a| match a {
            Action::Release { slot, .. } => std::cmp::Reverse(slot.0),
            _ => std::cmp::Reverse(0),
        });
        let position = actions
            .iter()
            .position(|a| match a {
                Action::Tir { node, .. } => match self.body.cleanup_nodes[*node as usize].action {
                    tir::CleanupAction::Drop { place, .. } => {
                        let root = self.body.places[place.0 as usize].root;
                        let local = &self.body.locals[root.0 as usize];
                        local.scope.0 == 0 && local.storage != tir::Storage::Temporary
                    }
                    _ => false,
                },
                _ => false,
            })
            .unwrap_or(actions.len());
        actions.splice(position..position, releases);
        actions
    }

    /// Release what the state says is owned, for a failure TIR names no
    /// cleanup for: end active loans, then release in TIR's nested order
    /// (inner scopes first, temporaries before names, later slots first).
    pub(super) fn state_actions(&self) -> Vec<Action> {
        let abi = &self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize];
        let mut slots: Vec<SlotId> = self
            .state
            .slots
            .lacking(EMPTY)
            .into_iter()
            .map(|(s, _)| SlotId(s as u32))
            .filter(|&s| s.index() < self.out.slots.len())
            .filter(|&s| {
                let info = &self.out.slots[s.index()];
                let owned_here = match info.kind {
                    SlotKind::Parameter(k) => {
                        abi.parameters.get(k as usize).map(|p| p.convention) == Some(Passing::Take)
                    }
                    SlotKind::ReceiptParameter(source) => {
                        abi.receipts.as_ref().is_some_and(|contract| {
                            contract.parameters.iter().any(|p| {
                                p.source == source
                                    && p.mode
                                        == hirsel_lir::sealed_payload_source::ReceiptMode::Take
                            })
                        })
                    }
                    SlotKind::Capture(_) | SlotKind::ReturnSlot | SlotKind::ReceiptResult => false,
                    _ => true,
                };
                let st = &self.state.slots[s.index()];
                owned_here && st.live.may() && st.present().may() && self.owns(info.ty)
            })
            .collect();
        slots.sort_by_key(|s| {
            let info = &self.out.slots[s.index()];
            (
                std::cmp::Reverse(info.scope),
                !info.kind.is_temporary(),
                std::cmp::Reverse(s.0),
            )
        });
        // Views end before any holder is released.
        let mut out: Vec<Action> = self
            .state
            .loans
            .lacking(DEAD)
            .into_iter()
            .map(|(l, _)| Action::EndLoan {
                loan: LoanId(l as u32),
            })
            .collect();
        out.extend(slots.into_iter().map(|slot| Action::Release {
            slot,
            anchor: (STATE_RELEASE, slot.0),
        }));
        out
    }

    /// The actions a failure here runs.
    pub(super) fn failure_actions(&self) -> R<Vec<Action>> {
        let mut actions = self.failure.before.clone();
        match self.failure.cleanup {
            Some(c) => actions.extend(self.chain_actions(c)?),
            None => return Ok(self.state_actions()),
        }
        Ok(self.exit_actions(actions))
    }

    /// Structural equality or order of two non-scalar operands:
    /// `Inst::Compare` over the left operand's layout, failing with MEM-3
    /// Depth (`hirsel_interp::structural::binary`). The comparison only reads,
    /// so an owned operand is read through a view under a synthetic loan; a
    /// taken one is released after the comparison, as the interpreter discards
    /// it, and a copied one is never copied.
    pub(super) fn compare(
        &mut self,
        op: BinaryOp,
        left: &tir::Operand,
        right: &tir::Operand,
        ty: LirTypeId,
    ) -> R<ValueId> {
        let op = match op {
            BinaryOp::Eq => CompareOp::Eq,
            BinaryOp::Ne => CompareOp::Ne,
            BinaryOp::Lt => CompareOp::Lt,
            BinaryOp::Le => CompareOp::Le,
            BinaryOp::Gt => CompareOp::Gt,
            BinaryOp::Ge => CompareOp::Ge,
            other => return malformed(format!("{other:?} of non-scalar values")),
        };
        let mut loans = Vec::new();
        let mut taken = Vec::new();
        let l = self.compared(left, &mut loans, &mut taken)?;
        // Preparing the right place can fail (bounds/tag checks). The left
        // operand's operation-owned storage and loan already exist then,
        // but TIR cleanup cannot name them.
        let prepared = self.reading_actions(&loans, &taken);
        self.failure.before.splice(0..0, prepared.iter().cloned());
        let r = self.compared(right, &mut loans, &mut taken)?;
        self.failure.before.drain(0..prepared.len());
        let layout = self.layout_of(self.value_type(l));
        self.structural_access(self.body.places[left.place.0 as usize].ty, layout)?;
        let result = self.fresh(ty);
        self.reading(loans, taken, |this| {
            let failure = this.fail(Some("MEM-3".to_string()))?;
            this.push(Inst::Compare {
                result,
                op,
                left: l,
                right: r,
                layout,
                failure,
            });
            Ok(())
        })?;
        Ok(result)
    }

    /// Cleanup for prepared read-only operands, also used while preparing
    /// later operands that can fail before the operation is entered.
    pub(super) fn reading_actions(&self, loans: &[LoanId], taken: &[SlotId]) -> Vec<Action> {
        // Temporaries of one scope release in descending slot order.
        let mut taken = taken.to_vec();
        taken.sort_by(|a, b| b.cmp(a));
        loans
            .iter()
            .copied()
            .map(|loan| Action::EndLoan { loan })
            .chain(taken.into_iter().map(|slot| Action::Release {
                slot,
                anchor: (INSTRUCTION_DROP, self.statement_action()),
            }))
            .collect()
    }

    /// Emit an operation that only reads operands `compared` prepared: their
    /// synthetic loans end and the taken slots are released after it, and on
    /// its failure edge before the frame's cleanup.
    pub(super) fn reading(
        &mut self,
        loans: Vec<LoanId>,
        taken: Vec<SlotId>,
        emit: impl FnOnce(&mut Self) -> R<()>,
    ) -> R<()> {
        let release = self.reading_actions(&loans, &taken);
        self.failure.before.splice(0..0, release.iter().cloned());
        emit(self)?;
        self.failure.before.drain(0..release.len());
        for (index, action) in release.iter().enumerate() {
            match self.action(action)? {
                Step2::Inline(inst) => self.push(inst),
                Step2::Drop(drop) => {
                    drop.apply(self);
                    // A failed source release must still drain the remaining
                    // construction temporaries, which TIR does not name.
                    let remaining = &release[index + 1..];
                    self.failure.before.splice(0..0, remaining.iter().cloned());
                    let failure = self.fail(None)?;
                    self.failure.before.drain(0..remaining.len());
                    self.push(Inst::Drop {
                        address: drop.address,
                        if_live: drop.if_live,
                        anchor: drop.anchor,
                        failure,
                    });
                }
                Step2::Skip => {}
            }
        }
        Ok(())
    }

    /// One comparison operand as a read-only value: a borrowed or plain
    /// operand as usual, an owned copied or taken one as a view under a
    /// synthetic loan (`loans`). A taken owner moves into operation-owned
    /// temporary storage and releases after the operation (`taken`). Those
    /// temporaries follow all source staging slots: failure releases the
    /// operands before any kept source or intermediate copy in TIR cleanup.
    pub(super) fn compared(
        &mut self,
        op: &tir::Operand,
        loans: &mut Vec<LoanId>,
        taken: &mut Vec<SlotId>,
    ) -> R<ValueId> {
        self.compared_at(op, None, loans, taken)
    }

    /// Reuse a key address whose dynamic projections were evaluated before
    /// preflight. The eventual transfer must read that same evaluated place.
    pub(super) fn compared_at(
        &mut self,
        op: &tir::Operand,
        prepared: Option<AddressId>,
        loans: &mut Vec<LoanId>,
        taken: &mut Vec<SlotId>,
    ) -> R<ValueId> {
        if let Some(view) = self.joined_view(op) {
            return view;
        }
        let p = self.place(op.place)?;
        if let Some((source, _)) = p.source {
            // A variable part of a flat value is read in place, never copied.
            if op.usage == UseKind::Take {
                return unsupported("a taken part of a flat value as a comparison operand");
            }
            let address = match prepared {
                Some(address) => address,
                None => self.address(&p)?,
            };
            let loan = match p.loan {
                Some(l) if op.usage == UseKind::Borrow => self.loan(l)?,
                _ => {
                    let loan = self.synthetic_loan(address);
                    loans.push(loan);
                    loan
                }
            };
            return self.flat_view(address, loan, source);
        }
        if !matches!(op.usage, UseKind::Take | UseKind::Copy) || !self.owns(p.ty) {
            return match prepared {
                Some(address) => self.operand_at(op.usage, &p, address, p.loan),
                None => self.operand(op),
            };
        }
        let take = op.usage == UseKind::Take && !self.borrowed_storage(p.slot);
        if take && (!p.fixed || !p.steps.is_empty()) {
            return unsupported("a taken part of a value as a comparison operand");
        }
        let p = if take {
            let owned = self.operand(op)?;
            let scope = self.out.slots[p.slot.index()].scope;
            let slot = self.new_slot(p.ty, SlotKind::Temporary, scope);
            self.push(Inst::StorageLive { slot });
            self.state.live(slot);
            let place = self.whole(slot);
            let address = self.address(&place)?;
            self.store(&place, address, owned)?;
            place
        } else {
            p
        };
        let address = if take {
            self.address(&p)?
        } else {
            match prepared {
                Some(address) => address,
                None => self.address(&p)?,
            }
        };
        let loan = self.synthetic_loan(address);
        let reaching = self.reaching(p.slot)?;
        let v = self.value(p.ty, Holding::Borrowed(loan));
        self.push(Inst::Read {
            result: v,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        loans.push(loan);
        if take {
            taken.push(p.slot);
        }
        Ok(v)
    }

    /// A failure successor for the operation at hand: dispose pending owned
    /// values, run the failure cleanup, propagate.
    pub(super) fn fail(&mut self, rule: Option<String>) -> R<FailureId> {
        let live = std::mem::replace(&mut self.invocation_live, false);
        let pending = self.failure.pending.clone();
        let mut target = self.failure_target(&pending)?;
        self.invocation_live = live;
        if live {
            let saved = self.current;
            let retired = self.block(BlockKind::FailureCleanup);
            self.current = retired;
            let mut arguments = Vec::new();
            for &value in &pending {
                let replacement = self.value(self.value_type(value), Holding::Owned);
                self.out.blocks[retired.index()].params.push(replacement);
                arguments.push(replacement);
            }
            self.push(Inst::FrameLeave);
            let edge = self.edge(target, arguments);
            self.terminate(Terminator::Jump(edge));
            self.current = saved;
            target = retired;
        }
        let anchor = self.anchor_here();
        let edge = self.edge(target, pending);
        Ok(self.failure_edge(edge, rule, anchor))
    }

    /// Where a failure here continues: open constructor-tail holes are
    /// abandoned first (LANG-10), then the cleanup runs and `pending` is
    /// disposed of.
    fn failure_target(&mut self, pending: &[ValueId]) -> R<BlockId> {
        if self.holes_open() {
            let saved = self.save();
            let block = self.block(BlockKind::FailureCleanup);
            self.current = block;
            let mut forwarded = Vec::new();
            for &value in pending {
                let param = self.value(self.value_type(value), Holding::Owned);
                self.out.blocks[block.index()].params.push(param);
                forwarded.push(param);
            }
            self.abandon_holes()?;
            let target = self.failure_target(&forwarded)?;
            let edge = self.edge(target, forwarded);
            self.terminate(Terminator::Jump(edge));
            self.restore(saved);
            return Ok(block);
        }
        let actions = self.failure_actions()?;
        if pending.is_empty() {
            self.chain(&actions)
        } else {
            self.dispose(pending, &actions, false)
        }
    }

    /// A failure-cleanup block that receives `pending` owned values and
    /// disposes of them through scratch slots (see `fill_dispose`).
    pub(super) fn dispose(
        &mut self,
        pending: &[ValueId],
        actions: &[Action],
        outer: bool,
    ) -> R<BlockId> {
        let saved = self.save();
        let block = self.block(BlockKind::FailureCleanup);
        self.current = block;
        self.fill_dispose(pending, actions, outer)?;
        self.restore(saved);
        Ok(block)
    }

    /// Fill the current failure-cleanup block: receive `pending` and move each
    /// into a scratch slot. Values pending when an operation fails go first,
    /// in the innermost scope the cleanup releases. Values pending when a
    /// cleanup drop fails (a returned result) outlive the frame's cleanup:
    /// they go last, in the synthetic scope enclosing the body's scopes, so
    /// cleanup order stays nested.
    pub(super) fn fill_dispose(
        &mut self,
        pending: &[ValueId],
        actions: &[Action],
        outer: bool,
    ) -> R<()> {
        let block = self.current;
        let innermost = actions
            .iter()
            .find_map(|a| match a {
                Action::Tir { node, .. } => match self.body.cleanup_nodes[*node as usize].action {
                    tir::CleanupAction::Drop { place, .. } => {
                        let root = self.body.places[place.0 as usize].root;
                        self.local_slot[root.0 as usize]
                            .map(|slot| self.out.slots[slot.index()].scope)
                    }
                    _ => None,
                },
                Action::Release { slot, .. } => Some(self.out.slots[slot.index()].scope),
                Action::EndLoan { .. } => None,
            })
            .unwrap_or(ScopeId(0));
        let scope = if outer {
            self.outer_scope = true;
            ScopeId(self.body.scopes.len() as u32)
        } else {
            innermost
        };
        let types: Vec<LirTypeId> = pending.iter().map(|&v| self.value_type(v)).collect();
        // An inner scope's later temporaries release before its earlier
        // ones, so only the outer scope, which holds nothing else, shares.
        let slots = match self.scratch.get(&types).filter(|_| outer) {
            Some(slots) => slots.clone(),
            None => {
                let slots: Vec<SlotId> = types
                    .iter()
                    .map(|&ty| self.new_slot(ty, SlotKind::Temporary, scope))
                    .collect();
                if outer {
                    self.scratch.insert(types.clone(), slots.clone());
                }
                slots
            }
        };
        let mut releases = Vec::new();
        for (&ty, slot) in types.iter().zip(slots) {
            let param = self.value(ty, Holding::Owned);
            self.out.blocks[block.index()].params.push(param);
            self.push(Inst::StorageLive { slot });
            self.state.live(slot);
            let p = self.whole(slot);
            let address = self.fixed_address(&p)?;
            let version = self.value(ty, Holding::Plain);
            self.push(Inst::Store {
                address,
                value: param,
                version,
            });
            self.state.write(slot, &[]);
            self.versions[slot.index()] = Some(version);
            let n = releases.len() as u32;
            releases.push(Action::Release {
                slot,
                anchor: (SCRATCH_DISPOSAL, n),
            });
        }
        releases.reverse();
        if outer {
            let mut all = actions.to_vec();
            all.extend(releases);
            // A result no finalizer can observe is released at one site for
            // every exit, so the chains of all exits meet at their common
            // tail; one with user-visible destruction keeps its exit's site.
            let table = self.layouts.table();
            let silent = pending.iter().all(|&v| {
                let layout = self.out.types[self.value_type(v).index()].layout;
                !crate::insert::Lay(table).destructible(layout)
            });
            let span = self.span;
            if silent {
                self.span = self.body.blocks[self.body.entry.0 as usize].span;
            }
            let ran = self.run_failure(&all);
            self.span = span;
            ran
        } else {
            releases.extend(actions.iter().cloned());
            self.run_failure(&releases)
        }
    }

    /// The failure-cleanup chain running `actions` from the current state.
    /// Chains share tails: two failure points meet at the first block from
    /// which both run the same instructions on slots in the same state, so
    /// what else differs between them (their statement temporaries, slots a
    /// prefix has already released) costs no block.
    pub(super) fn chain(&mut self, actions: &[Action]) -> R<BlockId> {
        let saved = self.save();
        let links = self.links(actions)?;
        let mut next = None;
        for link in links.into_iter().rev() {
            let key = (link, next);
            next = Some(match self.chain_memo.get(&key) {
                Some(&block) => block,
                None => {
                    let block = self.block(BlockKind::FailureCleanup);
                    self.current = block;
                    self.link(&key.0, next);
                    self.chain_memo.insert(key, block);
                    block
                }
            });
        }
        self.restore(saved);
        Ok(next.expect("a chain ends with its propagation"))
    }

    /// Emit `actions` in failure mode into the current block, which keeps
    /// the first link; the rest of the chain is shared (`chain`).
    pub(super) fn run_failure(&mut self, actions: &[Action]) -> R<()> {
        let block = self.current;
        let mut links = self.links(actions)?.into_iter();
        let first = links.next().expect("a chain ends with its propagation");
        let rest: Vec<Link> = links.collect();
        let mut next = None;
        for link in rest.into_iter().rev() {
            let key = (link, next);
            next = Some(match self.chain_memo.get(&key) {
                Some(&block) => block,
                None => {
                    let block = self.block(BlockKind::FailureCleanup);
                    self.current = block;
                    self.link(&key.0, next);
                    self.chain_memo.insert(key, block);
                    block
                }
            });
        }
        self.current = block;
        self.link(&first, next);
        Ok(())
    }

    /// What `actions` do from the current state, one link per block of the
    /// chain: the loan ends up to a drop and that drop, the last link ending
    /// in the propagation. The state is left as the chain leaves it.
    pub(super) fn links(&mut self, actions: &[Action]) -> R<Vec<Link>> {
        let mut links = Vec::new();
        let mut ends = Vec::new();
        for action in actions {
            match self.action(action)? {
                Step2::Skip => {}
                Step2::Inline(Inst::EndLoan { loan, if_active }) => ends.push((loan, if_active)),
                Step2::Inline(other) => {
                    return malformed(format!("{other:?} as a cleanup action"));
                }
                Step2::Drop(drop) => {
                    let before = self.state.slots[drop.slot.index()].clone();
                    drop.apply(self);
                    links.push(Link {
                        ends: std::mem::take(&mut ends),
                        drop: Some((drop.address, drop.if_live, drop.anchor, before)),
                    });
                }
            }
        }
        links.push(Link { ends, drop: None });
        Ok(links)
    }

    /// Fill the current block with one link continuing at `next`: a drop that
    /// fails continues the same remaining chain, so both successors meet.
    pub(super) fn link(&mut self, link: &Link, next: Option<BlockId>) {
        for &(loan, if_active) in &link.ends {
            self.push(Inst::EndLoan { loan, if_active });
        }
        let (Some((address, if_live, anchor, _)), Some(next)) = (&link.drop, next) else {
            self.terminate(Terminator::Propagate);
            return;
        };
        let source = self.anchor_here();
        let fe = self.edge(next, Vec::new());
        let failure = self.failure_edge(fe, None, source);
        self.push(Inst::Drop {
            address: *address,
            if_live: *if_live,
            anchor: *anchor,
            failure,
        });
        let e = self.edge(next, Vec::new());
        self.terminate(Terminator::Jump(e));
    }

    /// Emit `actions` on a normal path into `Cleanup` blocks, ending with the
    /// current block still open. A drop that fails runs the rest of the list,
    /// then `tail`, disposing the pending values first.
    pub(super) fn run_normal(&mut self, actions: &[Action], tail: &[Action]) -> R<()> {
        let mut opened = false;
        for (i, action) in actions.iter().enumerate() {
            let step = self.action(action)?;
            if matches!(step, Step2::Skip) {
                continue;
            }
            if !opened {
                self.split(BlockKind::Cleanup);
                opened = true;
            }
            match step {
                Step2::Skip => {}
                Step2::Inline(inst) => self.push(inst),
                Step2::Drop(drop) => {
                    let (address, if_live, anchor) = drop.parts();
                    drop.apply(self);
                    let mut rest: Vec<Action> = actions[i + 1..].to_vec();
                    rest.extend(tail.iter().cloned());
                    let pending = self.failure.pending.clone();
                    let target = if pending.is_empty() {
                        self.chain(&rest)?
                    } else {
                        self.dispose(&pending, &rest, true)?
                    };
                    let source = self.anchor_here();
                    let fe = self.edge(target, pending);
                    let failure = self.failure_edge(fe, None, source);
                    self.push(Inst::Drop {
                        address,
                        if_live,
                        anchor,
                        failure,
                    });
                }
            }
        }
        Ok(())
    }

    /// What one action does from the current state: nothing (its slot or loan
    /// is definitely inactive), an inline loan end, or a drop.
    pub(super) fn action(&mut self, action: &Action) -> R<Step2> {
        let (slot, steps, anchor_at, span) = match action {
            Action::EndLoan { loan } => {
                return Ok(match self.state.loans[loan.index()] {
                    Tri::No => Step2::Skip,
                    t => {
                        self.state.loans[loan.index()] = Tri::No;
                        Step2::Inline(Inst::EndLoan {
                            loan: *loan,
                            if_active: t == Tri::Maybe,
                        })
                    }
                });
            }
            Action::Release { slot, anchor } => {
                // Plain content needs no release; storage end covers it.
                if !self.owns(self.out.slots[slot.index()].ty) {
                    return Ok(Step2::Skip);
                }
                // A synthetic state cleanup names the local it destroys,
                // just as its TIR cleanup does. Anchoring at each failing
                // call would duplicate otherwise identical cleanup suffixes.
                // The FailureEdge still retains the failing call's own site.
                let span = if anchor.0 == STATE_RELEASE {
                    self.local_slot
                        .iter()
                        .zip(&self.body.locals)
                        .find_map(|(bound, local)| (*bound == Some(*slot)).then_some(local.span))
                        .unwrap_or(self.span)
                } else {
                    self.span
                };
                (*slot, Vec::new(), *anchor, span)
            }
            Action::Tir { node } => match &self.body.cleanup_nodes[*node as usize].action {
                tir::CleanupAction::EndLoan { loan, .. } => {
                    if self.elided_loan(*loan) {
                        return Ok(Step2::Skip);
                    }
                    let loan = self.loan(*loan)?;
                    return self.action(&Action::EndLoan { loan });
                }
                tir::CleanupAction::Drop { place, .. } => {
                    if self.elided_place(*place) {
                        return Ok(Step2::Skip);
                    }
                    let tp = &self.body.places[place.0 as usize];
                    let root = tp.root.0 as usize;
                    if tp.projection.is_empty()
                        && (self.aliases[root].is_some() || self.joined[root])
                    {
                        // A view temporary owns nothing.
                        return Ok(Step2::Skip);
                    }
                    let unprojected_root = tp.projection.is_empty().then_some(tp.root);
                    let p = self.place(*place)?;
                    if unprojected_root.is_some()
                        && self.borrowed_storage(p.slot)
                        && self.out.types[p.ty.index()].repr == ValueRepr::Arena
                        && self.body.arena_scope_grant.as_ref().is_some_and(|grant| {
                            self.body
                                .parameters
                                .get(grant.arena_parameter.0 as usize)
                                .copied()
                                == unprojected_root
                                && hirsel_types::core_roles::arena_scope_callback(&grant.intrinsic)
                                    == Some(grant.callback_parameter.0)
                        })
                    {
                        // V-qualified scope grants have logical cleanup even
                        // with the public borrowed Arena spelling. The native
                        // token is inert: its opener owns the physical guard
                        // and closes it after the callback on either outcome.
                        // This is not a drop of the borrowed parameter storage.
                        return Ok(Step2::Skip);
                    }
                    if matches!(self.out.slots[p.slot.index()].kind, SlotKind::Capture(_)) {
                        // A closure's captures are its caller's environment,
                        // lent for the call: the environment releases them.
                        return Ok(Step2::Skip);
                    }
                    if !p.fixed {
                        return unsupported("a cleanup drop of a computed place");
                    }
                    let root = self.body.places[place.0 as usize].root;
                    let span = self.body.locals[root.0 as usize].span;
                    (p.slot, p.steps, self.node_anchors[*node as usize], span)
                }
                other => {
                    return unsupported(format!(
                        "cleanup action {other:?} (M2; the TIR validator reports it unsupported)"
                    ));
                }
            },
        };
        let st = &self.state.slots[slot.index()];
        let presence = if st.live == Tri::No {
            Tri::No
        } else if steps.is_empty() {
            match st.present() {
                Tri::Yes if st.live == Tri::Yes => Tri::Yes,
                Tri::No => Tri::No,
                _ => Tri::Maybe,
            }
        } else {
            let moved = st
                .moved
                .iter()
                .filter(|(q, _)| overlap(q, &steps))
                .map(|(q, t)| (q.len() <= steps.len(), *t))
                .collect::<Vec<_>>();
            if st.init == Tri::No || moved.iter().any(|&(covers, t)| covers && t == Tri::Yes) {
                Tri::No
            } else if st.init == Tri::Yes
                && st.live == Tri::Yes
                && moved.iter().all(|&(_, t)| t == Tri::No)
            {
                Tri::Yes
            } else {
                Tri::Maybe
            }
        };
        if presence == Tri::No {
            return Ok(Step2::Skip);
        }
        let ty = self.out.slots[slot.index()].ty;
        let place = PlaceRef {
            value: None,
            source_place: None,
            slot,
            steps: steps.clone(),
            projection: steps
                .iter()
                .map(|s| match s {
                    Step::Payload => ProjectionStep::Payload,
                    Step::Field(f) => ProjectionStep::Field(*f),
                    _ => unreachable!("fixed cleanup places have field steps only"),
                })
                .collect(),
            ty,
            fixed: true,
            crosses: false,
            loan: None,
            source: None,
            site: None,
        };
        let place = if steps.is_empty() {
            place
        } else {
            // The part's own type.
            let mut layout = self.layout_of(ty);
            for s in &steps {
                let Step::Field(f) = s else { unreachable!() };
                layout = self
                    .layouts
                    .record(layout)
                    .fields
                    .iter()
                    .find(|x| x.ordinal == *f)
                    .map(|x| x.layout)
                    .expect("field of a resolved place");
            }
            PlaceRef {
                value: None,
                source_place: None,
                ty: self.intern(layout),
                ..place
            }
        };
        let address = self.fixed_address(&place)?;
        let anchor = self.cleanup_anchor(anchor_at.0, anchor_at.1, span);
        Ok(Step2::Drop(DropStep {
            address,
            if_live: presence != Tri::Yes,
            anchor,
            slot,
            steps,
        }))
    }
}
