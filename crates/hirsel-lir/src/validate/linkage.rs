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
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub(super) struct LinkageRecord {
    definition: DefPath,
    instance: ContentHash,
    target: ContentHash,
    stage: crate::stage::StageKind,
    abi: ContentHash,
    scoped: ContentHash,
    entries: Vec<Entry>,
}
snapshot_struct!(LinkageRecord {
    definition,
    instance,
    target,
    stage,
    abi,
    scoped,
    entries
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
    abi: ContentHash,
    scoped: ContentHash,
}
snapshot_struct!(Entry {
    target,
    direct,
    arena,
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
                abi: keys.fn_abi(abi),
                scoped: scoped(abi),
            });
        }
        Some(Self {
            definition: unit.header.definition.clone(),
            instance: unit.header.instance,
            target: unit.header.target,
            stage: S::KIND,
            abi: keys.fn_abi(own),
            scoped: scoped(own),
            entries,
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
            if let Some(target) = target
                && target.scoped != entry.scoped
            {
                return Err("call mismatches scoped body contract".into());
            }
        }
    }
    Ok(())
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
