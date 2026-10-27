//! Arena and scoped linkage observations made from actual C-checked units.
//! This retains the existing cross-unit checks without retaining their bodies.
use crate::{
    abi_types::{FnAbi, ParameterPath, ScopedEntry},
    ir::{Callee, Symbol},
    layout_types::LayoutTable,
    stage::{Stage, ValidatedUnit},
};
use hirsel_codec::snapshot_struct;
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub(super) struct LinkageRecord {
    definition: DefPath,
    instance: ContentHash,
    target: ContentHash,
    stage: crate::stage::StageKind,
    abi: ContentHash,
    scoped: ContentHash,
    entries: Vec<Entry>,
    data: bool,
}
snapshot_struct!(LinkageRecord {
    definition,
    instance,
    target,
    stage,
    abi,
    scoped,
    entries,
    data
});
#[derive(Clone, Debug)]
struct Target {
    definition: DefPath,
    instance: Option<ContentHash>,
}
snapshot_struct!(Target {
    definition,
    instance
});
#[derive(Clone, Debug)]
struct Entry {
    target: Option<Target>,
    direct: bool,
    arena: bool,
    data: bool,
    abi: ContentHash,
    scoped: ContentHash,
}
snapshot_struct!(Entry {
    target,
    direct,
    arena,
    data,
    abi,
    scoped
});
#[derive(Clone, Debug)]
struct Scoped {
    parameters: Vec<ParameterPath>,
    captures: Vec<ParameterPath>,
    contract: ContentHash,
    entries: Vec<ScopedEntry>,
}
snapshot_struct!(Scoped {
    parameters,
    captures,
    contract,
    entries
});
fn scoped(abi: &FnAbi) -> ContentHash {
    crate::snapshot::digest(
        "checked-scoped-link.v1",
        &Scoped {
            parameters: abi.nonretaining.clone(),
            captures: abi.nonretaining_captures.clone(),
            contract: abi.scoped_contract,
            entries: abi.scoped_entries.clone(),
        },
    )
}
impl LinkageRecord {
    pub(super) fn capture<S: Stage>(unit: &ValidatedUnit<S>, table: &LayoutTable) -> Option<Self> {
        Self::observe(unit.unit(), table)
    }
    fn observe<S: Stage>(unit: &crate::ir::LirUnit<S>, table: &LayoutTable) -> Option<Self> {
        let keys = crate::LayoutKeys::new(table);
        let own = table.fn_abis.get(unit.header.abi.0 as usize)?;
        let target = |symbol: &Symbol| Target {
            definition: symbol.definition.clone(),
            instance: symbol.instance,
        };
        let mut entries = Vec::new();
        for call in &unit.tables.calls {
            let abi = table.fn_abis.get(call.abi.0 as usize)?;
            let (symbol, direct) = match call.callee {
                Callee::Direct(symbol) => (Some(symbol), true),
                Callee::Bound { code, .. } => (Some(code), false),
                Callee::Indirect(_)
                    if abi.result_arena.is_some()
                        || super::scoped_transport::has_data_paths(abi, table) =>
                {
                    (None, false)
                }
                _ if abi.result_arena.is_some() => (None, false),
                _ => continue,
            };
            let selected = match symbol {
                Some(symbol) => Some(target(unit.tables.symbols.get(symbol.index())?)),
                None => None,
            };
            entries.push(Entry {
                target: selected,
                direct,
                arena: abi.result_arena.is_some(),
                data: super::scoped_transport::has_data_paths(abi, table),
                abi: keys.fn_abi(abi),
                scoped: scoped(abi),
            });
        }
        for code in unit
            .tables
            .closures
            .iter()
            .filter_map(|closure| closure.code.as_ref())
        {
            let abi = table.fn_abis.get(code.abi.0 as usize)?;
            entries.push(Entry {
                target: Some(target(unit.tables.symbols.get(code.symbol.index())?)),
                direct: false,
                arena: false,
                data: super::scoped_transport::has_data_paths(abi, table),
                abi: keys.fn_abi(abi),
                scoped: scoped(abi),
            });
        }
        let data_capture = own.nonretaining_captures.iter().any(|p| {
            let layout = unit
                .captures
                .get(p.parameter as usize)
                .map(|v| unit.types[unit.values[v.index()].ty.index()].layout)
                .and_then(|layout| {
                    p.fields.iter().try_fold(layout, |layout, field| {
                        table
                            .layouts
                            .get(layout.0 as usize)?
                            .fields
                            .iter()
                            .find(|f| f.ordinal == *field)
                            .map(|f| f.layout)
                    })
                });
            layout.is_some_and(|layout| {
                !matches!(
                    table.layouts[layout.0 as usize].kind,
                    crate::layout_types::LayoutKind::Callable { .. }
                )
            })
        });
        Some(Self {
            definition: unit.header.definition.clone(),
            instance: unit.header.instance,
            target: unit.header.target,
            stage: S::KIND,
            abi: keys.fn_abi(own),
            scoped: scoped(own),
            entries,
            data: super::scoped_transport::has_data_paths(own, table) || data_capture,
        })
    }
    pub(super) fn checked(&self) -> CheckedLinkage {
        CheckedLinkage {
            record: Arc::new(self.clone()),
        }
    }
}
/// Opaque current C-owned observations. No public constructor or decoder;
/// persisted ordinary summaries cannot acquire linkage authority.
#[derive(Clone, Debug)]
pub struct CheckedLinkage {
    record: Arc<LinkageRecord>,
}
impl CheckedLinkage {
    pub fn definition(&self) -> &DefPath {
        &self.record.definition
    }
    pub fn instance(&self) -> ContentHash {
        self.record.instance
    }
    pub fn abi(&self) -> ContentHash {
        self.record.abi
    }
}

/// Exact existing result-arena and heap scoped-link obligations, using
/// content-named ABI/facet identities. These are distinct from strict module
/// scoped qualification and future receipt/invocation transport obligations.
pub fn check_arena_scoped_links(units: &[CheckedLinkage]) -> Result<(), String> {
    let targets: BTreeMap<_, _> = units
        .iter()
        .map(|unit| {
            (
                (&unit.record.definition, unit.record.instance),
                &unit.record,
            )
        })
        .collect();
    if targets.len() != units.len() {
        return Err("duplicate scoped body identity".into());
    }
    for unit in units {
        if units.first().is_some_and(|first| {
            first.record.target != unit.record.target || first.record.stage != unit.record.stage
        }) {
            return Err("native linkage target or stage differs".into());
        }
        for entry in &unit.record.entries {
            if entry.arena && !entry.direct {
                return Err("an indirect call claims a result arena".into());
            }
            let target = entry.target.as_ref().and_then(|symbol| {
                symbol
                    .instance
                    .and_then(|instance| targets.get(&(&symbol.definition, instance)))
            });
            if entry.arena {
                let target = target.ok_or("result-arena callee is absent")?;
                if target.abi != entry.abi {
                    return Err("call forges its result arena".into());
                }
            }
            if entry.data && entry.target.is_some() && target.is_none() {
                return Err("nonretaining data callee body absent".into());
            }
            if let Some(target) = target
                && target.scoped != entry.scoped
            {
                return Err("call mismatches scoped body contract".into());
            }
        }
    }
    check_data(&units.iter().map(|u| &*u.record).collect::<Vec<_>>())
}

/// Opaque C products retain the same dependency equations as full-body linking.
/// Local C proves each root cannot reach a sink assuming its callees. Start
/// with no proved bodies: declarations on a cycle cannot prove each other.
fn check_data(units: &[&LinkageRecord]) -> Result<(), String> {
    let targets: BTreeMap<_, _> = units
        .iter()
        .enumerate()
        .map(|(i, u)| ((&u.definition, u.instance), i))
        .collect();
    let mut dependencies = BTreeMap::new();
    for (i, unit) in units.iter().enumerate().filter(|(_, u)| u.data) {
        let mut required = BTreeSet::new();
        for entry in &unit.entries {
            if entry.data && entry.target.is_none() {
                return Err("indirect data nonretention has no checked body".into());
            }
            let target = entry.target.as_ref().and_then(|symbol| {
                symbol
                    .instance
                    .and_then(|instance| targets.get(&(&symbol.definition, instance)))
            });
            if let Some(&target) = target {
                if units[target].data {
                    required.insert(target);
                }
            } else if entry.data {
                return Err("nonretaining data callee body absent".into());
            }
        }
        dependencies.insert(i, required);
    }
    let mut proved = BTreeSet::new();
    loop {
        let before = proved.len();
        for (&body, callees) in &dependencies {
            if callees.is_subset(&proved) {
                proved.insert(body);
            }
        }
        if proved.len() == before {
            break;
        }
    }
    if let Some((&body, _)) = dependencies.iter().find(|(body, _)| !proved.contains(body)) {
        return Err(format!(
            "{} has unproved cyclic data nonretention",
            units[body].definition.0.name
        ));
    }
    Ok(())
}

pub(super) fn check_data_calls<S: Stage>(
    units: &[crate::ir::LirUnit<S>],
    table: &LayoutTable,
) -> Result<(), String> {
    let records = units
        .iter()
        .map(|u| LinkageRecord::observe(u, table).ok_or("nonretaining linkage unavailable"))
        .collect::<Result<Vec<_>, _>>()?;
    check_data(&records.iter().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        abi_types::{CallMultiplicity, CallableShape, FnAbiId, FnAbiKey},
        ir::*,
        stage::Value,
    };
    fn hash(text: &str) -> ContentHash {
        ContentHash::of_bytes(text.as_bytes())
    }
    fn abi(arena: Option<u32>) -> FnAbi {
        FnAbi {
            key: FnAbiKey {
                callable: hash("callable"),
                abi_version: crate::ABI_VERSION,
                receipt_contract: None,
            },
            shape: CallableShape::Definition,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: false,
            caller_site: false,
            destination: false,
            result: None,
            diverges: false,
            result_arena: arena,
            parameters: vec![],
            version: crate::ABI_VERSION,
            nonretaining: vec![],
            nonretaining_captures: vec![],
            scoped_contract: crate::abi::scoped_contract_digest(&[], &[]),
            scoped_entries: vec![],
            receipts: None,
        }
    }
    fn unit(name: &str, table: &LayoutTable) -> LirUnit<Value> {
        LirUnit::new(UnitHeader {
            source_count: 0,
            track_caller: false,
            stage: crate::stage::StageKind::Value,
            lir_version: crate::LIR_VERSION,
            origin: UnitOrigin::Source,
            key: crate::objects::UnitKey(hash(name)),
            definition: DefPath::core(name),
            implementation_revision: hash("source"),
            instance: hash(name),
            target: table.target,
            abi: FnAbiId(0),
        })
    }
    fn pair(arena: Option<u32>) -> (Vec<LirUnit<Value>>, LayoutTable) {
        let mut table = LayoutTable::new(&crate::TargetSpec::x86_64_unknown_linux_gnu());
        table.fn_abis = vec![abi(arena), abi(arena)];
        let callee = unit("callee", &table);
        let mut caller = unit("caller", &table);
        caller.tables.symbols.push(Symbol {
            kind: SymbolKind::Function,
            definition: callee.header.definition.clone(),
            instance: Some(callee.header.instance),
        });
        caller.tables.calls.push(CallSite {
            forward_caller: false,
            pretransfer_failure: None,
            receipts: None,
            callee: Callee::Direct(SymbolId(0)),
            operation_arguments: vec![],
            abi: FnAbiId(1),
            arguments: PoolRange::EMPTY,
            result: None,
            destination: None,
            declared: crate::Effects::NONE,
            anchor: SourceAnchorId(0),
        });
        (vec![caller, callee], table)
    }
    // Raw vectors compare precisely the two existing linkage algorithms. They
    // are not C-acceptance tests: production capture requires ValidatedUnit,
    // and the separate compiler control exercises actual C and relocation.
    fn agrees(label: &str, units: &[LirUnit<Value>], table: &LayoutTable, expected: bool) {
        let old = super::super::check_result_arena_calls(units, table)
            .and_then(|()| super::super::check_scoped_links(units, table))
            .is_ok();
        let observations = units
            .iter()
            .map(|unit| LinkageRecord::observe(unit, table).unwrap().checked())
            .collect::<Vec<_>>();
        let new = check_arena_scoped_links(&observations).is_ok();
        assert_eq!(old, expected, "original {label}");
        assert_eq!(new, old, "observed {label}");
    }
    #[test]
    fn compact_data_dependencies_refuse_missing_cyclic_and_indirect_proofs() {
        // The local C sink proof is separate. These raw observations exercise
        // the compact module equations and their durable field encoding.
        let (units, table) = pair(None);
        let mut records = units
            .iter()
            .map(|u| LinkageRecord::observe(u, &table).unwrap())
            .collect::<Vec<_>>();
        for record in &mut records {
            record.data = true;
        }
        records[0].entries[0].data = true;
        let check = |records: &[LinkageRecord]| {
            check_arena_scoped_links(
                &records
                    .iter()
                    .map(LinkageRecord::checked)
                    .collect::<Vec<_>>(),
            )
        };
        check(&records).unwrap();
        assert!(check(&records[..1]).unwrap_err().contains("body absent"));
        let mut back = records[0].entries[0].clone();
        back.target = Some(Target {
            definition: records[0].definition.clone(),
            instance: Some(records[0].instance),
        });
        records[1].entries.push(back);
        assert!(
            check(&records)
                .unwrap_err()
                .contains("unproved cyclic data nonretention")
        );
        for record in &mut records {
            let encoded = hirsel_codec::to_text(record);
            *record = hirsel_codec::from_text(&encoded).unwrap();
        }
        assert!(
            check(&records)
                .unwrap_err()
                .contains("unproved cyclic data nonretention")
        );
        records[1].entries[0].target = None;
        assert!(
            check(&records)
                .unwrap_err()
                .contains("indirect data nonretention")
        );
        records[1].entries.clear();
        check(&records).unwrap();
    }
    #[test]
    fn observations_preserve_arena_and_heap_scoped_link_outcomes() {
        let (units, table) = pair(Some(0));
        agrees("matching result arena", &units, &table, true);
        agrees("missing arena callee", &units[..1], &table, false);
        let mut duplicate = units.clone();
        duplicate.push(units[1].clone());
        agrees("duplicate callee", &duplicate, &table, false);
        let mut indirect = units.clone();
        indirect[0].tables.calls[0].callee = Callee::Indirect(ValueId(0));
        agrees("indirect result arena", &indirect, &table, false);
        let mut wrong = table.clone();
        wrong.fn_abis[1].diverges = true;
        agrees("wrong full ABI", &units, &wrong, false);
        wrong = table.clone();
        wrong.fn_abis[1].result_arena = Some(1);
        agrees("wrong result arena", &units, &wrong, false);
        let (units, table) = pair(None);
        agrees("matching scoped entry", &units, &table, true);
        let selector = ParameterPath {
            parameter: 0,
            fields: vec![2],
        };
        let mut wrong = table.clone();
        wrong.fn_abis[1].nonretaining.push(selector.clone());
        agrees("changed scoped parameter", &units, &wrong, false);
        agrees(
            "missing scoped callee preserves heap policy",
            &units[..1],
            &wrong,
            true,
        );
        wrong = table.clone();
        wrong.fn_abis[1]
            .nonretaining_captures
            .push(selector.clone());
        agrees("changed scoped capture", &units, &wrong, false);
        wrong = table.clone();
        wrong.fn_abis[1].scoped_contract = hash("changed contract");
        agrees("changed scoped contract", &units, &wrong, false);
        wrong = table.clone();
        wrong.fn_abis[1].scoped_entries.push(ScopedEntry {
            parameter: selector,
            identity: hash("nested entry"),
        });
        agrees("changed nested scoped entry", &units, &wrong, false);
    }
}
