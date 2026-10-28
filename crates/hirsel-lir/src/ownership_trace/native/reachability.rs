//! Generated failure cleanup can be unreachable after an exact variant tag test.
//! Keep ordinary dead-code obligations. Exclude only paths lost when the checked
//! selecting-arm proof removes a synthetic static VariantField failure edge.
use super::*;

pub(super) struct Excluded {
    blocks: BTreeSet<BlockId>,
    edges: BTreeSet<EdgeId>,
}
impl Excluded {
    pub(super) fn contains(&self, point: Point) -> bool {
        match point {
            Point::Instruction { block, .. }
            | Point::CallEntered(block)
            | Point::CallReturned(block)
            | Point::Return(block) => self.blocks.contains(&block),
            Point::Edge(edge) => self.edges.contains(&edge),
            _ => false,
        }
    }
}

pub(super) fn variant_failures<S: Stage>(unit: &LirUnit<S>) -> Option<Excluded> {
    let selected = super::super::compound::selected_addresses(unit)?;
    fn walk<S: Stage>(
        unit: &LirUnit<S>,
        selected: &BTreeSet<(BlockId, u32)>,
    ) -> Option<(BTreeSet<BlockId>, BTreeSet<EdgeId>)> {
        let mut blocks = BTreeSet::new();
        let mut edges = BTreeSet::new();
        let mut pending = vec![unit.entry];
        while let Some(block) = pending.pop() {
            if !blocks.insert(block) {
                continue;
            }
            let code = unit
                .blocks
                .get(block.index())?
                .instructions
                .get(&unit.instructions)?;
            let term = unit.terminators.get(block.index())?;
            let mut next = term.normal_edges(&unit.pools);
            for failure in term.failure_edges(&unit.tables) {
                next.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
            for (index, inst) in code.iter().enumerate() {
                if matches!(inst, Inst::Address { .. }) && selected.contains(&(block, index as u32))
                {
                    continue;
                }
                if let Some(failure) = inst.failure() {
                    next.push(unit.tables.failures.get(failure.index())?.cleanup);
                }
            }
            for edge in next {
                edges.insert(edge);
                pending.push(unit.tables.edges.get(edge.index())?.target);
            }
        }
        Some((blocks, edges))
    }
    let (ordinary_blocks, ordinary_edges) = walk(unit, &BTreeSet::new())?;
    let (feasible_blocks, feasible_edges) = walk(unit, &selected)?;
    Some(Excluded {
        blocks: ordinary_blocks
            .difference(&feasible_blocks)
            .copied()
            .collect(),
        edges: ordinary_edges
            .difference(&feasible_edges)
            .copied()
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{abi_types::FnAbiId, objects::UnitKey, testing::UnitBuilder};

    // Raw CFG fixtures test the proof matcher; production uses C-validated Ready.
    fn fixture() -> LirUnit<Ready> {
        let hash = ContentHash::of_bytes(b"variant-failure-coverage");
        let mut b = UnitBuilder::new(LirUnit::new(UnitHeader {
            source_count: 0,
            track_caller: false,
            stage: crate::stage::StageKind::Ready,
            lir_version: crate::LIR_VERSION,
            origin: UnitOrigin::Source,
            key: UnitKey(hash),
            definition: hirsel_syntax::DefPath::core("variant_failure_coverage"),
            implementation_revision: hash,
            instance: hash,
            target: ContentHash::default(),
            abi: FnAbiId(0),
        }));
        let entry = b.block(BlockKind::Body, &[]);
        let selected = b.block(BlockKind::Cleanup, &[]);
        let empty = b.block(BlockKind::Cleanup, &[]);
        let failed = b.block(BlockKind::FailureCleanup, &[]);
        let dead = b.block(BlockKind::Cleanup, &[]);
        let slot = b.slot(ValueRepr::Aggregate, LayoutId(0), SlotKind::Temporary);
        let root = b.address(
            AddressBase::Slot(slot),
            &[],
            ValueRepr::Aggregate,
            LayoutId(0),
        );
        let child = b.address(
            AddressBase::Address(root),
            &[ProjectionStep::VariantField {
                variant: 1,
                field: 0,
            }],
            ValueRepr::Aggregate,
            LayoutId(0),
        );
        let value = b.plain(ValueRepr::Aggregate, LayoutId(0));
        b.push(
            entry,
            Inst::Read {
                result: value,
                address: root,
                reaching: value,
                mode: ReadMode::Borrow,
            },
        );
        let selected_edge = b.edge(selected, &[]);
        let empty_edge = b.edge(empty, &[]);
        let failure_edge = b.edge(failed, &[]);
        let failure = b.failure(failure_edge, None, &[]);
        let arms = PoolRange::push(
            &mut b.unit.pools.switch_arms,
            [SwitchArm {
                value: 1,
                edge: selected_edge,
            }],
        );
        b.term(
            entry,
            Terminator::Switch {
                value,
                arms,
                otherwise: empty_edge,
            },
        );
        b.push(
            selected,
            Inst::Address {
                address: child,
                failure,
            },
        );
        for block in [selected, empty, failed, dead] {
            b.ret(block, None);
        }
        b.finish()
    }

    #[test]
    fn only_tag_proved_failure_cleanup_is_excluded() {
        let unit = fixture();
        let point = |block| Point::Return(BlockId(block));
        let excluded = variant_failures(&unit).unwrap();
        assert!(excluded.contains(point(3)));
        assert!(!excluded.contains(point(1)));
        assert!(
            !excluded.contains(point(4)),
            "ordinary dead code retains its obligations"
        );
        for corruption in ["tag", "root", "other entry", "reused arm edge"] {
            let mut forged = unit.clone();
            match corruption {
                "tag" => {
                    forged.pools.projections[0] = ProjectionStep::VariantField {
                        variant: 0,
                        field: 0,
                    }
                }
                "root" => {
                    let mut other = forged.addresses[0].clone();
                    other.base = AddressBase::Slot(SlotId(1));
                    let address = AddressId(forged.addresses.len() as u32);
                    forged.addresses.push(other);
                    let Inst::Read {
                        address: borrowed, ..
                    } = &mut forged.instructions[0]
                    else {
                        unreachable!()
                    };
                    *borrowed = address;
                }
                "other entry" => {
                    let edge = EdgeId(forged.tables.edges.len() as u32);
                    forged.tables.edges.push(Edge {
                        target: BlockId(3),
                        arguments: PoolRange::EMPTY,
                    });
                    forged.terminators[2] = Terminator::Jump(edge);
                }
                "reused arm edge" => {
                    let Terminator::Switch {
                        arms, otherwise, ..
                    } = &mut forged.terminators[0]
                    else {
                        unreachable!()
                    };
                    *otherwise = arms.get(&forged.pools.switch_arms).unwrap()[0].edge;
                }
                _ => unreachable!(),
            }
            assert!(
                !variant_failures(&forged).unwrap().contains(point(3)),
                "{corruption}"
            );
        }
    }
}
