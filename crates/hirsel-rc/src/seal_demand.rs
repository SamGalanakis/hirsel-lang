//! Physical demanded-guard selection over the actual specialized unit graph.
//! These are allocation hints only. Source grant permission, complete inventory
//! and seal correspondence remain independent V/C/runtime obligations.
use hirsel_lir::ir::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug)]
pub struct Demand {
    values: Vec<BTreeSet<ValueId>>,
}

impl Demand {
    /// Follow grant flow backwards from actual seals. No source type spelling,
    /// unknown callee or absent original inventory is treated as an empty plan.
    pub fn analyze<S>(units: &[LirUnit<S>], entries: &[usize]) -> Result<Self, String> {
        if entries.iter().any(|&entry| entry >= units.len()) {
            return Err("seal-demand entry outside the unit graph".into());
        }
        let mut reachable: BTreeSet<_> = entries.iter().copied().collect();
        let mut offsets = vec![0];
        for unit in units {
            offsets.push(offsets.last().unwrap() + unit.values.len());
        }
        let mut graph = vec![BTreeSet::new(); *offsets.last().unwrap()];
        let mut origins = vec![BTreeSet::<(usize, usize)>::new(); graph.len()];
        let mut seeds = vec![];
        let mut symbols = BTreeMap::new();
        for (u, unit) in units.iter().enumerate() {
            if symbols
                .insert((unit.header.definition.clone(), unit.header.instance), u)
                .is_some()
            {
                return Err("duplicate seal-demand unit instance".into());
            }
            let edge = |graph: &mut Vec<BTreeSet<usize>>, to: ValueId, from: ValueId| {
                if to.index() < unit.values.len() && from.index() < unit.values.len() {
                    graph[offsets[u] + to.index()].insert(offsets[u] + from.index());
                }
            };
            for inst in &unit.instructions {
                match *inst {
                    Inst::SealArena { grant, .. } => seeds.push((u, offsets[u] + grant.index())),
                    Inst::Closure {
                        result, closure, ..
                    } => {
                        origins[offsets[u] + result.index()].insert((u, closure.index()));
                    }
                    Inst::CopyValue { result, value }
                    | Inst::Weaken { result, value }
                    | Inst::Recast { result, value }
                    | Inst::Read {
                        result,
                        reaching: value,
                        ..
                    }
                    | Inst::Copy {
                        result,
                        reaching: value,
                        ..
                    }
                    | Inst::Store {
                        version: result,
                        value,
                        ..
                    }
                    | Inst::Replace {
                        version: result,
                        value,
                        ..
                    } => edge(&mut graph, result, value),
                    _ => (),
                }
            }
            for row in &unit.tables.edges {
                let params = unit.blocks[row.target.index()]
                    .parameters
                    .get(&unit.pools.values)
                    .ok_or("invalid seal-demand block parameters")?;
                let args = row
                    .arguments
                    .get(&unit.pools.values)
                    .ok_or("invalid seal-demand edge arguments")?;
                if params.len() != args.len() {
                    return Err("seal-demand edge arity".into());
                }
                for (&to, &from) in params.iter().zip(args) {
                    edge(&mut graph, to, from);
                }
            }
        }
        // The ordinary path does not analyze calls or change any region row.
        if seeds.is_empty() {
            return Ok(Self {
                values: vec![BTreeSet::new(); units.len()],
            });
        }
        let resolve = |u: usize, symbol: SymbolId| -> Option<usize> {
            let symbol = units[u].tables.symbols.get(symbol.index())?;
            if let Some(instance) = symbol.instance {
                symbols.get(&(symbol.definition.clone(), instance)).copied()
            } else {
                let mut matches = symbols
                    .iter()
                    .filter(|((path, _), _)| *path == symbol.definition);
                let (_, &unit) = matches.next()?;
                matches.next().is_none().then_some(unit)
            }
        };
        // Callable origins and interprocedural value edges grow monotonically.
        // A returned closure, CFG alternative or recursive call cannot truncate
        // demand through a clone/iteration budget.
        loop {
            let mut changed = false;
            let mut unresolved = Vec::new();
            for value in 0..graph.len() {
                let discovered: BTreeSet<_> = graph[value]
                    .iter()
                    .flat_map(|&source| origins[source].iter().copied())
                    .collect();
                let before = origins[value].len();
                origins[value].extend(discovered);
                changed |= before != origins[value].len();
            }
            for (u, unit) in units.iter().enumerate() {
                if !reachable.contains(&u) {
                    continue;
                }
                for term in &unit.terminators {
                    let Terminator::Call { call, .. } = *term else {
                        continue;
                    };
                    let call = &unit.tables.calls[call.index()];
                    let environments = call
                        .callee
                        .value()
                        .map(|v| origins[offsets[u] + v.index()].clone())
                        .unwrap_or_default();
                    let targets: BTreeSet<_> = match call.callee {
                        Callee::Direct(code) | Callee::Bound { code, .. } => {
                            resolve(u, code).into_iter().collect()
                        }
                        Callee::Indirect(_) => environments
                            .iter()
                            .filter_map(|&(from, c)| {
                                units[from].tables.closures[c]
                                    .code
                                    .and_then(|c| resolve(from, c.symbol))
                            })
                            .collect(),
                        Callee::Operation(_) => BTreeSet::new(),
                    };
                    let args = call
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .ok_or("invalid seal-demand call arguments")?;
                    // No optimistic empty expansion for an unresolved grant
                    // carrier. A later complete dispatch graph can support it.
                    if targets.is_empty()
                        && args.iter().any(|arg| {
                            let value = match arg.value {
                                ArgumentValue::Value(v) => v,
                                ArgumentValue::Address { version, .. } => version,
                            };
                            unit.types[unit.values[value.index()].ty.index()].repr
                                == ValueRepr::Arena
                        })
                    {
                        unresolved.push(format!("{} {:?}", unit.header.definition, call.callee));
                    }
                    for target in targets {
                        changed |= reachable.insert(target);
                        let callee = &units[target];
                        for arg in args {
                            let input = match arg.value {
                                ArgumentValue::Value(value) => value,
                                ArgumentValue::Address { version, .. } => version,
                            };
                            let formal = *callee
                                .parameters
                                .get(arg.parameter as usize)
                                .ok_or("seal-demand parameter arity")?;
                            changed |= graph[offsets[target] + formal.index()]
                                .insert(offsets[u] + input.index());
                        }
                        if let Some(result) = call.result {
                            for term in &callee.terminators {
                                if let Terminator::Return(Some(value)) = *term {
                                    changed |= graph[offsets[u] + result.index()]
                                        .insert(offsets[target] + value.index());
                                }
                            }
                        }
                        for &(from, closure) in &environments {
                            let captures = units[from].tables.closures[closure]
                                .captures
                                .get(&units[from].pools.captures)
                                .ok_or("invalid seal-demand captures")?;
                            if captures.len() != callee.captures.len() {
                                return Err("seal-demand capture arity".into());
                            }
                            for (&formal, actual) in callee.captures.iter().zip(captures) {
                                changed |= graph[offsets[target] + formal.index()]
                                    .insert(offsets[from] + actual.value.index());
                            }
                        }
                    }
                }
            }
            if !changed {
                if !unresolved.is_empty() {
                    return Err(format!(
                        "seal demand needs a complete arena call target: {}",
                        unresolved.join(", ")
                    ));
                }
                break;
            }
        }
        let mut seen = vec![false; graph.len()];
        let mut pending: VecDeque<_> = seeds
            .into_iter()
            .filter(|(unit, _)| reachable.contains(unit))
            .map(|(_, value)| value)
            .collect();
        while let Some(value) = pending.pop_front() {
            if seen[value] {
                continue;
            }
            seen[value] = true;
            pending.extend(graph[value].iter().copied());
        }
        Ok(Self {
            values: units
                .iter()
                .enumerate()
                .map(|(u, unit)| {
                    (0..unit.values.len())
                        .filter(|&v| seen[offsets[u] + v])
                        .map(|v| ValueId(v as u32))
                        .collect()
                })
                .collect(),
        })
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
