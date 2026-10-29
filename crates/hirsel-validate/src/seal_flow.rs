//! Independent last-use and storage-closure obligations of a seal transition.
use super::*;

impl Checker<'_, '_> {
    pub(super) fn seal_flow(
        &mut self,
        model: &Model,
        state: &State,
        callee: &Callee,
        arguments: &[CallArgument],
        report: bool,
    ) -> Option<PlaceId> {
        if !matches!(callee, Callee::Definition { path, .. } if path.is_core("arena.seal")) {
            return None;
        }
        let arg = |p| {
            arguments
                .iter()
                .find(|a| a.parameter.0 == p)
                .map(|a| a.value.place)
        };
        let (Some(grant), Some(root)) = (arg(0), arg(1)) else {
            return None;
        };
        let identities = self.origin_identity(model, state, grant);
        let grant_index = model.mapping[grant.0 as usize];
        let root_index = model.mapping[root.0 as usize];
        let allowed = |origin: &Origin| {
            identities
                .iter()
                .any(|identity| identity.root == origin.root && identity.fields == origin.fields)
        };
        // Current retained proof includes owned fields and captures. It cannot
        // be replaced by a producer's global-result or resource-free flag.
        let foreign = state.origins.retained[root_index]
            .iter()
            // A closed definition brand names immortal global identity, not
            // foreign arena storage. Local/parameter brands still need the grant.
            .any(|origin| !matches!(origin.root, OriginRoot::Definition(_)) && !allowed(origin))
            || self
                .t
                .row(self.place_type(root))
                .modes
                .relation
                .reaches()
                .iter()
                .any(|reach| {
                    matches!(
                        reach,
                        hirsel_types::Reach::Region(_) | hirsel_types::Reach::Environment
                    )
                });
        if foreign {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "seal storage closure",
                },
            );
        }
        let block = self.site.block.unwrap();
        let live = self.seal_live_after(block);
        let root_local = self.body.places[root.0 as usize].root;
        let grant_local = self.body.places[grant.0 as usize].root;
        let mut has_alias = false;
        for local in live {
            if local == root_local.0 || local == grant_local.0 {
                continue;
            }
            for &part in &model.root_parts[local as usize] {
                if state.places[part] & LIVE == 0 {
                    continue;
                }
                if state.origins.retained[part].iter().any(allowed)
                    || state.sources[part].contains(&(grant_local.0 as usize))
                    || state.content_loans[local as usize]
                        .held()
                        .iter()
                        .any(|loan| {
                            let (_, place, _) = model.loans[*loan];
                            model.mapping[place.0 as usize] == grant_index
                        })
                {
                    has_alias = true;
                    self.flow_error(
                        report,
                        ViolationKind::TypeMismatch {
                            entity: "seal live arena view",
                        },
                    );
                }
            }
        }
        // Lowering ends the grant's source name while evaluating its take,
        // before evaluating the root argument. That expires arena bounds on
        // the root, but the taken grant still owns that exact incarnation.
        // Only this transaction may transport those expired bounds. Foreign
        // incarnations and hidden capture loans remain ordinary invalid uses.
        let grant_owned = state.places[grant_index] == LIVE
            && state.must[grant_local.0 as usize].has(MustFacts::OWNED)
            && !identities.is_empty()
            && identities.iter().all(|identity| {
                state.origins.owners[grant_index]
                    .iter()
                    .any(|owner| super::flow_origins::contains(owner, identity))
            });
        let own_expiration = model
            .parts(root)
            .all(|part| state.origins.expired[part].iter().all(allowed))
            && self
                .environment(model, state, root)
                .expired()
                .iter()
                .all(allowed);
        (!foreign && !has_alias && grant_owned && own_expiration).then_some(root)
    }

    /// Recompute demand from executable uses, CFG joins and observable cleanup.
    /// Producer EndLoan claims do not erase future uses. Copyable holder cleanup
    /// alone is not a future view use, matching LANG-3's last-use rule.
    fn seal_live_after(&self, call: BlockId) -> BTreeSet<u32> {
        let mut inputs = vec![BTreeSet::<u32>::new(); self.body.blocks.len()];
        let root = |p: PlaceId| self.body.places[p.0 as usize].root.0;
        let cleanup = |id: CleanupId, live: &mut BTreeSet<u32>| {
            for action in self.body.cleanup_actions(id) {
                match action {
                    CleanupAction::Drop { place, .. }
                        if self.destroyed(self.place_type(*place)) =>
                    {
                        live.insert(root(*place));
                    }
                    CleanupAction::CloseArena(place)
                    | CleanupAction::JoinTasks { scope: place, .. } => {
                        live.insert(root(*place));
                    }
                    _ => (),
                }
            }
        };
        loop {
            let mut changed = false;
            for index in (0..self.body.blocks.len()).rev() {
                let block = &self.body.blocks[index];
                let mut live = BTreeSet::new();
                for successor in self.body.successors(&block.terminator) {
                    live.extend(&inputs[successor.0 as usize]);
                }
                match &block.terminator {
                    Terminator::Call {
                        callee,
                        arguments,
                        returns,
                        on_failure,
                    } => {
                        if let Some(returns) = returns {
                            live.remove(&root(returns.destination));
                        }
                        if let Callee::Indirect { value, .. } = callee {
                            live.insert(root(value.place));
                        }
                        live.extend(arguments.iter().map(|a| root(a.value.place)));
                        cleanup(*on_failure, &mut live);
                    }
                    Terminator::Return { value, cleanup: id } => {
                        live.insert(root(value.place));
                        cleanup(*id, &mut live);
                    }
                    Terminator::Fail { cleanup: id, .. } => cleanup(*id, &mut live),
                    Terminator::Branch { condition, .. } => {
                        live.insert(root(condition.place));
                    }
                    Terminator::Switch { value, .. } => {
                        live.insert(root(value.place));
                    }
                    Terminator::Match { table, .. } => {
                        live.insert(root(self.body.matches[table.0 as usize].scrutinee));
                    }
                    _ => (),
                }
                for instruction in block.instructions.iter().rev() {
                    cleanup(instruction.on_failure, &mut live);
                    match &instruction.kind {
                        InstructionKind::Assign { destination, value } => {
                            if self.body.places[destination.0 as usize]
                                .projection
                                .is_empty()
                            {
                                live.remove(&root(*destination));
                            }
                            live.extend(
                                super::super::typing::operands(
                                    &self.body.rvalues[value.0 as usize],
                                )
                                .iter()
                                .map(|v| root(v.place)),
                            );
                        }
                        InstructionKind::Replace {
                            destination,
                            value,
                            displaced,
                            ..
                        } => {
                            live.remove(&root(*displaced));
                            live.insert(root(*destination));
                            live.insert(root(value.place));
                        }
                        InstructionKind::Drop { place, .. }
                            if self.destroyed(self.place_type(*place)) =>
                        {
                            live.insert(root(*place));
                        }
                        InstructionKind::RunCleanup(id) => cleanup(*id, &mut live),
                        InstructionKind::BeginLoan { place, .. } => {
                            live.insert(root(*place));
                        }
                        InstructionKind::StorageLive(local) => {
                            live.remove(&local.0);
                        }
                        InstructionKind::StorageDead { place, .. } => {
                            live.remove(&root(*place));
                        }
                        InstructionKind::Check { condition, .. } => {
                            live.insert(root(condition.place));
                        }
                        _ => (),
                    }
                }
                if inputs[index] != live {
                    inputs[index] = live;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.body
            .successors(&self.body.blocks[call.0 as usize].terminator)
            .into_iter()
            .flat_map(|b| inputs[b.0 as usize].iter().copied())
            .collect()
    }
}
