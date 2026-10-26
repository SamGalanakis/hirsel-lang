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

/// Portable physical-capacity selection for one original instance. This is
/// ordinary producer metadata, with no source grant or seal permission.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitDemand {
    owner: equations::Function,
    original_values: u32,
    values: Vec<u32>,
}

impl UnitDemand {
    pub fn owner(&self) -> &equations::Function {
        &self.owner
    }

    pub fn original_value_count(&self) -> u32 {
        self.original_values
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.values.windows(2).any(|w| w[0] >= w[1])
            || self.values.iter().any(|&v| v >= self.original_values)
        {
            return Err("invalid local seal-demand selection".into());
        }
        Ok(())
    }

    /// Placement preserves original value IDs and may append fresh ones. Bind
    /// the original instance before selecting its matching physical guards.
    pub fn apply<S>(&self, unit: &mut LirUnit<S>) -> Result<(), String> {
        self.validate()?;
        if self.owner != (unit.header.definition.clone(), unit.header.instance)
            || self.original_values as usize > unit.values.len()
        {
            return Err("seal-demand selection names another original instance".into());
        }
        let values = self.values.iter().copied().map(ValueId).collect();
        apply(&values, unit)
    }
}

impl Demand {
    pub fn for_unit(
        &self,
        index: usize,
        original: &equations::LocalDemandEquations,
    ) -> Result<UnitDemand, String> {
        original.validate()?;
        let values = self.values.get(index).ok_or("missing seal-demand unit")?;
        let out = UnitDemand {
            owner: original.owner().clone(),
            original_values: original.value_count(),
            values: values.iter().map(|v| v.0).collect(),
        };
        out.validate()?;
        Ok(out)
    }
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
        apply(demanded, unit)
    }
}

fn apply<S>(demanded: &BTreeSet<ValueId>, unit: &mut LirUnit<S>) -> Result<(), String> {
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
            unit.tables
                .regions
                .get_mut(region.index())
                .ok_or("seal-demand region index")?
                .seal_demand = marks.contains(&result);
        }
    }
    Ok(())
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
