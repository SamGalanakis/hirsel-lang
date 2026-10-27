//! The existing per-object template closure over compact instance facts.
//! Ready transformations never depend on other Ready bodies merely for LLVM
//! copying: those are Codegen reads, avoiding artificial recursive Ready cycles.
use hirsel_lir::ir::{Callee, Inst, LirUnit, Symbol, UnitOrigin};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type Function = hirsel_rc::arena_credit::equations::Function;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateFacts {
    pub small: bool,
    pub execution: bool,
    pub library: bool,
    pub references: Vec<Function>,
}
impl TemplateFacts {
    pub(super) fn of<S>(unit: &LirUnit<S>, finalizers: impl Iterator<Item = Function>) -> Self {
        let mut references = references(unit)
            .filter_map(|symbol| symbol.instance.map(|id| (symbol.definition, id)))
            .chain(finalizers)
            .collect::<Vec<_>>();
        references.sort();
        references.dedup();
        let execution = unit.header.definition.is_core("$native.task_main")
            || unit.header.definition.is_core("$native.cpu_worker")
            || unit.tables.calls.iter().any(|call| match call.callee {
                Callee::Operation(op) => !matches!(
                    crate::native::continuation::classify(
                        &unit.tables.operations[op.index()].definition
                    ),
                    Some(hirsel_lir::continuation::CalleeClass::Local)
                ),
                Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => matches!(
                    crate::native::continuation::classify(
                        &unit.tables.symbols[symbol.index()].definition
                    ),
                    Some(
                        hirsel_lir::continuation::CalleeClass::Boundary
                            | hirsel_lir::continuation::CalleeClass::Unknown
                    )
                ),
                _ => true,
            });
        Self {
            execution,
            small: unit.instructions.len() <= crate::native::link::SMALL_UNIT_INSTRUCTIONS,
            library: unit.header.origin == UnitOrigin::Library,
            references,
        }
    }
    pub(super) fn canonical(&self) -> bool {
        !self.references.windows(2).any(|pair| pair[0] >= pair[1])
    }
}

/// Ported unchanged from native/units.rs:187-201. Only copyable templates
/// extend traversal; a noncopyable intermediate does not expose its callees.
/// The owner is excluded even when recursive edges return to it.
pub(crate) fn copy_closure<K: Ord + Clone>(
    owner: &K,
    mut references: impl FnMut(&K) -> Vec<K>,
    mut copyable: impl FnMut(&K) -> bool,
) -> Vec<K> {
    let mut seen = BTreeSet::new();
    let mut pending = references(owner);
    while let Some(member) = pending.pop() {
        if &member != owner && copyable(&member) && seen.insert(member.clone()) {
            pending.extend(references(&member));
        }
    }
    seen.into_iter().collect()
}

/// Every function the unit names directly, by a function reference or as closure code.
pub(crate) fn references<S>(unit: &LirUnit<S>) -> impl Iterator<Item = Symbol> + '_ {
    let calls = unit.tables.calls.iter().filter_map(|c| match c.callee {
        Callee::Direct(s) | Callee::Bound { code: s, .. } => Some(s),
        _ => None,
    });
    let closures = unit
        .tables
        .closures
        .iter()
        .filter_map(|site| site.code.map(|code| code.symbol));
    let refs = unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::FunctionRef { symbol, .. } => Some(symbol),
        _ => None,
    });
    calls
        .chain(refs)
        .chain(closures)
        .filter_map(|s| unit.tables.symbols.get(s.index()).cloned())
}
