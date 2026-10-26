//! LANG-10 room checks: admissions of depth-leaf callees.
//!
//! A *depth-leaf* body has no admission of its own and calls only leaf
//! operations ([`hirsel_core_contract::depth::adds_activation`]), so nothing
//! while it runs reads the task's counters or its invocation sites: no nested
//! admission, callback, finalizer or host call. Its caller's admission is then
//! observable only through its failure, so native code may lower it to a
//! read-only test of the room left (`runtime_abi::CTX_FOREGROUND_COUNT_OFFSET`
//! and the words after it) and its retirements to nothing. The failure edge,
//! record and site are the admission's own, so overflow fails at exactly the
//! same frame and call site.
use crate::ir::*;
use crate::stage::Stage;
use hirsel_core_contract::depth::adds_activation;
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A unit's identity as its callers' direct symbols name it.
pub type UnitKey = (DefPath, Option<ContentHash>);

/// Whether `unit` is depth-leaf. Conservative: only in-place instructions and only
/// calls of leaf operations; anything that releases, finalizes, switches regions or
/// catches failure is refused.
pub fn depth_leaf<S: Stage>(unit: &LirUnit<S>) -> bool {
    let instructions = unit.instructions.iter().all(in_place);
    instructions
        && unit.terminators.iter().all(|terminator| match terminator {
            Terminator::Jump(_)
            | Terminator::Branch { .. }
            | Terminator::Switch { .. }
            | Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::Propagate
            | Terminator::Unreachable
            | Terminator::InvariantFault { .. } => true,
            Terminator::Call { call, .. } => unit
                .tables
                .calls
                .get(call.index())
                .and_then(|call| match call.callee {
                    Callee::Direct(symbol) => unit
                        .tables
                        .symbols
                        .get(symbol.index())
                        .map(|s| &s.definition),
                    Callee::Operation(operation) => unit
                        .tables
                        .operations
                        .get(operation.index())
                        .map(|o| &o.definition),
                    Callee::Indirect(_) | Callee::Bound { .. } => None,
                })
                .is_some_and(|callee| !adds_activation(callee)),
            Terminator::RejectTransfer
            | Terminator::Recover { .. }
            | Terminator::Isolate { .. } => false,
        })
}

/// An instruction that touches memory or scalars in place: it reads no LANG-10
/// counter and runs no language code (a store releases nothing; LIR releases are
/// explicit instructions).
fn in_place(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Const { .. }
            | Inst::Unary { .. }
            | Inst::Binary { .. }
            | Inst::Compare { .. }
            | Inst::Convert { .. }
            | Inst::IntArith { .. }
            | Inst::MulAdd { .. }
            | Inst::ScalarUnary { .. }
            | Inst::Check { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::Address { .. }
            | Inst::Read { .. }
            | Inst::Store { .. }
            | Inst::CopyValue { .. }
            | Inst::Copy { .. }
            | Inst::SeqLen { .. }
            | Inst::SeqCapacity { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Forget { .. }
    )
}

/// The admissions a native emitter lowers to room checks, by index into
/// `LirUnit::instructions`: each `FrameEnter` whose only invocation is a direct
/// call of a depth-leaf unit and that covers nothing else but in-place
/// instructions, and every `FrameLeave` retiring one. An admission that stays live
/// across other work (a finalizer's, through its value's field drops) is counted
/// by that work, so it keeps its entry and retirement.
///
/// Outside its own admissions a frame sees the counters at their entry values:
/// every callee, callback and runtime finalizer admission is retired before control
/// returns to it. Only `FinalizeBegin`/`FinalizeEnd` hold one across instructions of
/// the frame, so a unit with them selects nothing, and a native emitter may read the
/// room once, in its prologue.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoomChecks {
    pub enters: BTreeSet<usize>,
    pub leaves: BTreeSet<usize>,
}

impl RoomChecks {
    /// Follow each admission along the validated CFG, as `invocations::check`
    /// does. An unexpected shape selects nothing: full admission stays sound.
    pub fn of<S: Stage>(unit: &LirUnit<S>, leaf: &BTreeSet<UnitKey>) -> RoomChecks {
        Self::find(unit, leaf).unwrap_or_default()
    }

    fn find<S: Stage>(unit: &LirUnit<S>, leaf: &BTreeSet<UnitKey>) -> Option<RoomChecks> {
        if leaf.is_empty()
            || unit
                .instructions
                .iter()
                .any(|inst| matches!(inst, Inst::FinalizeBegin { .. } | Inst::FinalizeEnd { .. }))
        {
            return None;
        }
        let edge = |id: EdgeId| unit.tables.edges.get(id.index()).map(|e| e.target);
        // Per admission: whether every call it covers is a depth-leaf direct
        // call (and there is one), and the retirements that reach it.
        let mut admissions: BTreeMap<usize, (bool, bool, Vec<usize>)> = BTreeMap::new();
        let mut incoming: BTreeMap<BlockId, Option<usize>> = BTreeMap::from([(unit.entry, None)]);
        let mut pending = VecDeque::from([unit.entry]);
        while let Some(block) = pending.pop_front() {
            let mut active = incoming[&block];
            let mut outgoing = Vec::new();
            let range = unit.blocks.get(block.index())?.instructions;
            let start = range.start as usize;
            for (offset, inst) in range.get(&unit.instructions)?.iter().enumerate() {
                if let Some(failure) = inst.failure() {
                    let cleanup = unit.tables.failures.get(failure.index())?.cleanup;
                    outgoing.push((edge(cleanup)?, active));
                }
                match inst {
                    Inst::FrameEnter { .. } => {
                        if active.is_some() {
                            return None;
                        }
                        active = Some(start + offset);
                        admissions.insert(start + offset, (true, false, Vec::new()));
                    }
                    Inst::FrameLeave => {
                        admissions.get_mut(&active.take()?)?.2.push(start + offset);
                    }
                    _ if in_place(inst) => (),
                    _ => {
                        if let Some(enter) = active {
                            admissions.get_mut(&enter)?.0 = false;
                        }
                    }
                }
            }
            let terminator = unit.terminators.get(block.index())?;
            if let (Terminator::Call { call, .. }, Some(enter)) = (terminator, active) {
                let call = unit.tables.calls.get(call.index())?;
                let covered = match call.callee {
                    Callee::Direct(symbol) => {
                        let symbol = unit.tables.symbols.get(symbol.index())?;
                        leaf.contains(&(symbol.definition.clone(), symbol.instance))
                    }
                    _ => false,
                };
                let admission = admissions.get_mut(&enter)?;
                admission.0 &= covered && !admission.1;
                admission.1 = true;
            }
            outgoing.extend(
                terminator
                    .normal_edges(&unit.pools)
                    .into_iter()
                    .map(|e| edge(e).map(|b| (b, active)))
                    .collect::<Option<Vec<_>>>()?,
            );
            for failure in terminator.failure_edges(&unit.tables) {
                let cleanup = unit.tables.failures.get(failure.index())?.cleanup;
                outgoing.push((edge(cleanup)?, active));
            }
            for (target, state) in outgoing {
                match incoming.get(&target) {
                    Some(previous) if *previous != state => return None,
                    Some(_) => (),
                    None => {
                        incoming.insert(target, state);
                        pending.push_back(target);
                    }
                }
            }
        }
        let mut checks = RoomChecks::default();
        for (enter, (covered, called, leaves)) in admissions {
            if covered && called {
                checks.enters.insert(enter);
                checks.leaves.extend(leaves);
            }
        }
        Some(checks)
    }
}

/// The depth-leaf units among `units`, keyed as direct symbols name them.
pub fn depth_leaf_units<'a, S: Stage + 'a>(
    units: impl IntoIterator<Item = &'a LirUnit<S>>,
) -> BTreeSet<UnitKey> {
    units
        .into_iter()
        .filter(|unit| depth_leaf(unit))
        .map(|unit| (unit.header.definition.clone(), Some(unit.header.instance)))
        .collect()
}

/// Direct compiled callees need only the logical counter, unless their own
/// body invokes runtime-only checked callbacks. Such a callback inherits the
/// immediately invoking decode/extern scope's full site entry. Calls made by a
/// counter-only callee establish their own scopes in the ordinary way.
///
/// Unknown/imported/foreign and indirect targets are absent from this set and
/// keep full entries. The same independently validated single-call scope walk
/// as RoomChecks proves each selected retirement matches exactly one admission.
pub fn counter_units<'a, S: Stage + 'a>(
    units: impl IntoIterator<Item = &'a LirUnit<S>>,
) -> BTreeSet<UnitKey> {
    units
        .into_iter()
        .filter(|unit| {
            !unit
                .instructions
                .iter()
                .any(|inst| matches!(inst, Inst::Decode { .. }))
        })
        .map(|unit| (unit.header.definition.clone(), Some(unit.header.instance)))
        .collect()
}
