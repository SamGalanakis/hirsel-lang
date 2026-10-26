//! Physical demanded-guard selection over the actual specialized unit graph.
//! These are allocation hints only. Source grant permission, complete inventory
//! and seal correspondence remain independent V/C/runtime obligations.
use hirsel_lir::ir::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub mod equations;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Demand {
    values: Vec<BTreeSet<ValueId>>,
}

impl Demand {
    /// Follow grant flow backwards from actual seals. No source type spelling,
    /// unknown callee or absent original inventory is treated as an empty plan.
    pub fn analyze<S>(units: &[LirUnit<S>], entries: &[usize]) -> Result<Self, String> {
        let locals = units
            .iter()
            .map(equations::LocalDemandEquations::extract)
            .collect::<Result<Vec<_>, _>>()?;
        equations::solve(&locals, entries)
    }

    /// Placement keeps original token IDs. Only its matching physical region
    /// receives the hint; unrelated arenas and compiler-local regions stay 64B.
    pub fn apply<S>(&self, unit_index: usize, unit: &mut LirUnit<S>) -> Result<(), String> {
        let demanded = self
            .values
            .get(unit_index)
            .ok_or("missing seal-demand unit")?;
        let marks: BTreeSet<_> = unit
            .instructions
            .iter()
            .filter_map(|inst| match *inst {
                Inst::ArenaToken { result, mark, .. } if demanded.contains(&result) => Some(mark),
                _ => None,
            })
            .collect();
        for inst in &unit.instructions {
            if let Inst::RegionEnter { result, region } = *inst {
                unit.tables.regions[region.index()].seal_demand = marks.contains(&result);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_lir::{
        abi_types::FnAbiId,
        objects::UnitKey,
        stage::{StageKind, Value},
    };
    use hirsel_syntax::{ContentHash, DefPath};
    fn unit() -> LirUnit<Value> {
        let digest = ContentHash([1; 32]);
        LirUnit::new(UnitHeader {
            source_count: 0,
            key: UnitKey(digest),
            track_caller: false,
            stage: StageKind::Value,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Source,
            definition: DefPath::core("guard_demand_test"),
            implementation_revision: digest,
            instance: digest,
            target: digest,
            abi: FnAbiId(0),
        })
    }
    #[test]
    fn no_seal_unit_keeps_all_regions_ordinary_and_rejects_missing_roots() {
        let mut unit = unit();
        unit.tables.regions.push(RegionInstance {
            scope: ScopeId(0),
            anchor: SourceAnchorId(0),
            seal_demand: false,
        });
        let before = hirsel_codec::to_text(&unit);
        let demand = Demand::analyze(&[unit.clone()], &[0]).unwrap();
        demand.apply(0, &mut unit).unwrap();
        assert_eq!(hirsel_codec::to_text(&unit), before);
        assert!(Demand::analyze(&[unit], &[1]).is_err());
    }
}
