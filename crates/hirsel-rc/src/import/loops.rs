use super::*;

impl Importer<'_> {
    /// The loop prologue: every loop-carried parameter moves (a taken one) or
    /// is copied (a borrowed one: one count operation) into its loop slot.
    pub(super) fn enter_loop(&mut self, parameter_slots: &[Option<SlotId>]) -> R<()> {
        let plan = self.plan.expect("a loop plan");
        let mut order: Vec<usize> = (0..plan.homes.len())
            .filter(|&k| plan.homes[k] == Home::Loop)
            .collect();
        // Owned values first, so a failing copy finds them in their slots.
        order.sort_by_key(|&k| parameter_slots[k].is_some());
        for k in order {
            let l = self.loop_slots[k].expect("loop slot");
            self.push(Inst::StorageLive { slot: l });
            self.state.live(l);
            let value = match parameter_slots[k] {
                None => self.out.parameters[k],
                Some(ps) => {
                    let p = self.whole(ps);
                    let address = self.fixed_address(&p)?;
                    self.operand_at(UseKind::Copy, &p, address, None)?
                }
            };
            let lp = self.whole(l);
            let address = self.fixed_address(&lp)?;
            self.store(&lp, address, value)?;
        }
        Ok(())
    }

    /// A verified self-tail site: the arguments become the loop slots' next
    /// contents and control returns to the header. Nothing of the frame is
    /// pending at a verified site, so every other slot ends here.
    pub(super) fn back_edge(&mut self, site: &LoopSite, arguments: &[tir::CallArgument]) -> R<()> {
        let plan = self.plan.expect("a loop plan");
        let n = plan.homes.len();
        let mut values: Vec<Option<ValueId>> = vec![None; n];
        for a in arguments {
            let k = a.parameter.0 as usize;
            if plan.homes[k] == Home::Parameter || site.owned.iter().any(|(o, _)| *o as usize == k)
            {
                continue;
            }
            let v = match a.value.usage {
                UseKind::Take | UseKind::Copy => self.operand(&a.value)?,
                UseKind::Borrow => {
                    let p = self.place(a.value.place)?;
                    let address = self.address(&p)?;
                    self.operand_at(UseKind::Copy, &p, address, None)?
                }
                UseKind::Mut => return malformed("a mut argument at a verified self-tail site"),
            };
            values[k] = Some(v);
        }
        for l in 0..self.state.loans.len() {
            if let Step2::Inline(inst) = self.action(&Action::EndLoan {
                loan: LoanId(l as u32),
            })? {
                self.push(inst);
            }
        }
        for (k, place) in &site.owned {
            let p = self.place(*place)?;
            let address = self.address(&p)?;
            values[*k as usize] = Some(self.operand_at(UseKind::Take, &p, address, None)?);
        }
        for s in 0..self.out.slots.len() {
            let slot = SlotId(s as u32);
            let info = &self.out.slots[s];
            if !matches!(
                info.kind,
                SlotKind::Local | SlotKind::Temporary | SlotKind::Displaced
            ) {
                continue;
            }
            let st = &self.state.slots[s];
            if st.live == Tri::No {
                continue;
            }
            if self.owns(info.ty) && st.present().may() {
                return malformed(format!(
                    "slot {s} still owns content at the verified self-tail site in block {}",
                    site.call_block.0
                ));
            }
            let if_live = st.live == Tri::Maybe;
            self.push(Inst::StorageDead { slot, if_live });
            self.state.dead(slot);
            self.versions[s] = None;
        }
        for k in 0..n {
            if plan.homes[k] != Home::Loop {
                continue;
            }
            let l = self.loop_slots[k].expect("loop slot");
            let Some(v) = values[k] else {
                return malformed(format!("no argument for parameter {k}"));
            };
            if let Step2::Drop(drop) = self.action(&Action::Release {
                slot: l,
                anchor: (LOOP_RELEASE, k as u32),
            })? {
                drop.apply(self);
                let failure = self.fail(None)?;
                self.push(Inst::Drop {
                    address: drop.address,
                    if_live: drop.if_live,
                    anchor: drop.anchor,
                    failure,
                });
            }
            if self.state.slots[l.index()].present() != Tri::No {
                // Plain content: a store may not overwrite it, so the slot's
                // storage restarts.
                self.push(Inst::StorageDead {
                    slot: l,
                    if_live: false,
                });
                self.push(Inst::StorageLive { slot: l });
                self.state.live(l);
            }
            let lp = self.whole(l);
            let address = self.fixed_address(&lp)?;
            self.store(&lp, address, v)?;
        }
        let e = self.enter(self.body.entry);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Lay the builder blocks out into the unit's arenas.
    pub(super) fn finish(mut self) -> R<LirUnit<Value>> {
        let view = self.instances.view(self.id);
        let revision = self
            .program
            .revisions
            .get(self.def.path.path())
            .copied()
            .ok_or_else(|| InstanceError::MissingRevision(self.def.path.path().clone()))?;
        let table = self.layouts.table();
        let mut unit = LirUnit::<Value>::new(UnitHeader {
            key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
            track_caller: self.def.tracks_caller(view.body),
            stage: StageKind::Value,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Source,
            definition: self.def.path.path().clone(),
            implementation_revision: revision,
            instance: view.key.digest(),
            target: table.target,
            abi: self.instances.abis[self.id.index()],
        });
        let out = std::mem::take(&mut self.out);
        let mut pools = out.pools;
        let mut tables = out.tables;
        let mut values = out.values;
        let mut incoming: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
        for edge in &out.edges {
            let target = &out.blocks[edge.target.index()];
            let mut args = edge.args.clone();
            for &(slot, _) in &target.phis {
                match edge.versions.get(slot.index()).copied().flatten() {
                    Some(v) => args.push(v),
                    None => {
                        return malformed(format!(
                            "an edge into block {} carries no version of slot {}",
                            edge.target.0, slot.0
                        ));
                    }
                }
            }
            for &(l, p) in &target.view_phis {
                match edge.views.get(l).copied().flatten() {
                    Some(v) => {
                        args.push(v);
                        incoming.entry(p).or_default().push(v);
                    }
                    None => {
                        return malformed(format!(
                            "an edge into block {} carries no view of local {l}",
                            edge.target.0
                        ));
                    }
                }
            }
            tables.edges.push(Edge {
                target: edge.target,
                arguments: PoolRange::push(&mut pools.values, args),
            });
        }
        for (i, block) in out.blocks.into_iter().enumerate() {
            let params = block
                .params
                .iter()
                .copied()
                .chain(block.phis.iter().map(|(_, v)| *v))
                .chain(block.view_phis.iter().map(|(_, v)| *v));
            let parameters = PoolRange::push(&mut pools.values, params);
            let start = unit.instructions.len() as u32;
            unit.instructions.extend(block.insts);
            let Some(term) = block.term else {
                return malformed(format!("block {i} has no terminator"));
            };
            unit.blocks.push(Block {
                kind: block.kind,
                parameters,
                instructions: PoolRange::new(start, unit.instructions.len() as u32 - start),
                anchor: block.anchor,
            });
            unit.terminators.push(term);
        }
        join_view_holdings(&mut values, &incoming);
        unit.types = out.types;
        unit.values = values;
        unit.slots = out.slots;
        unit.addresses = out.addresses;
        unit.entry = self.prologue;
        unit.parameters = out.parameters;
        unit.captures = self.capture_versions.clone();
        let outer = self
            .outer_scope
            .then_some(ScopeId(self.body.scopes.len() as u32));
        unit.scopes = self
            .body
            .scopes
            .iter()
            .map(|s| ScopeInfo {
                parent: s.parent.map(|p| ScopeId(p.0)).or(outer),
            })
            .collect();
        if outer.is_some() {
            unit.scopes.push(ScopeInfo { parent: None });
        }
        unit.tables = tables;
        unit.pools = pools;
        Ok(unit)
    }
}
