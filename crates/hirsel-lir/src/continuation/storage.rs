use super::*;

type State = (BTreeSet<SlotId>, BTreeSet<LoanId>);

/// Settled forward may-live entries for one immutable Ready unit. Instruction
/// failure edges observe the pre-instruction state, as do queried coordinates.
pub(super) struct StorageFlow<'a> {
    unit: &'a LirUnit<Ready>,
    settled: std::cell::OnceCell<(Vec<State>, BTreeSet<BlockId>)>,
}
impl<'a> StorageFlow<'a> {
    pub(super) fn new(unit: &'a LirUnit<Ready>) -> Self {
        Self {
            unit,
            settled: std::cell::OnceCell::new(),
        }
    }
    fn solve(&self) -> (Vec<State>, BTreeSet<BlockId>) {
        let unit = self.unit;
        let mut entries = vec![State::default(); unit.blocks.len()];
        entries[unit.entry.index()]
            .0
            .extend(unit.slots.iter().enumerate().filter_map(|(i, slot)| {
                matches!(
                    slot.kind,
                    SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
                )
                .then_some(SlotId(i as u32))
            }));
        let mut reached = BTreeSet::from([unit.entry]);
        let mut pending = vec![unit.entry];
        while let Some(block) = pending.pop() {
            let mut state = entries[block.index()].clone();
            let mut successors = Vec::new();
            for inst in unit.blocks[block.index()]
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
            {
                if let Some(failure) = inst.failure() {
                    successors.push((
                        unit.tables.edges[unit.tables.failures[failure.index()].cleanup.index()]
                            .target,
                        state.clone(),
                    ));
                }
                transfer(&mut state, inst);
            }
            let term = &unit.terminators[block.index()];
            let mut edges = term.normal_edges(&unit.pools);
            edges.extend(
                term.failure_edges(&unit.tables)
                    .into_iter()
                    .map(|failure| unit.tables.failures[failure.index()].cleanup),
            );
            successors.extend(
                edges
                    .into_iter()
                    .map(|edge| (unit.tables.edges[edge.index()].target, state.clone())),
            );
            for (next, state) in successors {
                let entry = &mut entries[next.index()];
                let before = (entry.0.len(), entry.1.len());
                entry.0.extend(state.0);
                entry.1.extend(state.1);
                let grew = before != (entry.0.len(), entry.1.len());
                if reached.insert(next) || grew {
                    pending.push(next);
                }
            }
        }
        (entries, reached)
    }
    pub(super) fn before(&self, block: BlockId, before: usize) -> State {
        let (entries, reached) = self.settled.get_or_init(|| self.solve());
        if !reached.contains(&block) {
            return State::default();
        }
        let insts = self.unit.blocks[block.index()]
            .instructions
            .get(&self.unit.instructions)
            .unwrap_or(&[]);
        if before != usize::MAX && before >= insts.len() {
            return State::default();
        }
        let mut state = entries[block.index()].clone();
        for inst in insts.iter().take(before) {
            transfer(&mut state, inst);
        }
        state
    }
}
fn transfer(state: &mut State, inst: &Inst) {
    match *inst {
        Inst::StorageLive { slot } => {
            state.0.insert(slot);
        }
        Inst::StorageDead { slot, .. } => {
            state.0.remove(&slot);
        }
        Inst::BeginLoan { loan, .. } => {
            state.1.insert(loan);
        }
        Inst::EndLoan { loan, .. } => {
            state.1.remove(&loan);
        }
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{abi_types::FnAbiId, objects::UnitKey, stage::StageKind, testing::UnitBuilder};

    #[test]
    fn one_storage_solution_matches_per_site_flow_across_cycles_failure_edges_and_unreachable_blocks()
     {
        let hash = ContentHash::of_bytes(b"storage flow");
        let mut builder = UnitBuilder::new(LirUnit::<Ready>::new(UnitHeader {
            source_count: 1,
            key: UnitKey(hash),
            track_caller: false,
            stage: StageKind::Ready,
            lir_version: crate::LIR_VERSION,
            origin: UnitOrigin::Source,
            definition: DefPath::core("flow"),
            implementation_revision: hash,
            instance: hash,
            target: hash,
            abi: FnAbiId(0),
        }));
        for kind in [
            SlotKind::Parameter(0),
            SlotKind::Capture(0),
            SlotKind::Temporary,
        ] {
            builder.unit.slots.push(SlotInfo {
                ty: LirTypeId(0),
                kind,
                scope: ScopeId(0),
            });
        }
        let entry = builder.block(BlockKind::Body, &[]);
        let cycle = builder.block(BlockKind::Body, &[]);
        let exit = builder.block(BlockKind::Body, &[]);
        let cleanup = builder.block(BlockKind::FailureCleanup, &[]);
        let unreachable = builder.block(BlockKind::Body, &[]);
        builder.unit.entry = entry;
        let failure_edge = builder.edge(cleanup, &[]);
        builder.unit.tables.failures.push(FailureEdge {
            cleanup: failure_edge,
            rule: None,
            anchor: SourceAnchorId(0),
            dispose: PoolRange::EMPTY,
        });
        builder.push(
            entry,
            Inst::BeginLoan {
                loan: LoanId(0),
                address: AddressId(0),
                mutable: false,
                if_inactive: false,
            },
        );
        builder.push(
            entry,
            Inst::Check {
                condition: ValueId(0),
                failure: FailureId(0),
            },
        );
        builder.push(
            entry,
            Inst::StorageDead {
                slot: SlotId(0),
                if_live: false,
            },
        );
        builder.jump(entry, cycle, &[]);
        builder.push(
            cycle,
            Inst::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        );
        builder.push(cycle, Inst::StorageLive { slot: SlotId(2) });
        let yes = builder.edge(entry, &[]);
        let no = builder.edge(exit, &[]);
        builder.term(
            cycle,
            Terminator::Branch {
                condition: ValueId(0),
                yes,
                no,
            },
        );
        builder.push(
            exit,
            Inst::StorageDead {
                slot: SlotId(2),
                if_live: false,
            },
        );
        builder.term(exit, Terminator::Raise(FailureId(0)));
        builder.push(
            cleanup,
            Inst::EndLoan {
                loan: LoanId(0),
                if_active: true,
            },
        );
        builder.ret(cleanup, None);
        builder.push(unreachable, Inst::StorageLive { slot: SlotId(2) });
        builder.ret(unreachable, None);
        let unit = builder.finish();
        let flow = StorageFlow::new(&unit);
        assert!(
            flow.settled.get().is_none(),
            "units with no queried sites allocate no flow"
        );
        for (b, block) in unit.blocks.iter().enumerate() {
            for before in (0..=block.instructions.len as usize).chain([usize::MAX]) {
                assert_eq!(
                    flow.before(BlockId(b as u32), before),
                    super::super::storage_before(&unit, BlockId(b as u32), before),
                    "block {b}, before {before}"
                );
            }
        }
        let settled = flow.settled.get().unwrap() as *const _;
        assert_eq!(flow.before(unreachable, usize::MAX), State::default());
        assert!(
            flow.before(cleanup, 0).1.contains(&LoanId(0)),
            "instruction failure takes the loan before the instruction executes"
        );
        assert!(!flow.before(exit, usize::MAX).0.contains(&SlotId(2)));
        assert_eq!(flow.settled.get().unwrap() as *const _, settled);
    }
}
