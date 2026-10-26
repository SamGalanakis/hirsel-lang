//! LIR schema revision 26 (MEM-6): a bounded arena's budget and the recovery
//! of its own `OutOfSpace`. Each mutation is paired with its valid twin and is
//! rejected by the rule it names; with that rule removed, C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Placed, Value};
use support::*;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mutant {
    None,
    /// A budget that is not a u64.
    Budget,
    /// A recovery in ordinary code, reached and left by ordinary edges.
    Ordinary,
    /// A recovery whose caught edge stays in failure cleanup.
    Unresumed,
    /// A recovery of an unbounded opening.
    Unbounded,
    /// A recovery of an arena the unit did not open: a parameter's.
    Foreign,
}

/// `arena.bounded`'s shape: open an arena bounded by the u64 parameter, fail,
/// and recover that arena's failure to a return, else propagate.
fn bounded(m: Mutant) -> Vec<K> {
    let mut t = Lir::<Value>::new(
        &[(Passing::Borrow, l::U64), (Passing::Borrow, l::ARENA)],
        None,
    );
    t.builder.unit.header.origin = UnitOrigin::Library;
    let cap = t.plain(l::U64);
    t.param(cap);
    let foreign = t.plain(l::ARENA);
    t.param(foreign);
    let budget = if m == Mutant::Budget {
        t.int(ENTRY)
    } else {
        cap
    };
    let token = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: token,
            budget: (m != Mutant::Unbounded).then_some(budget),
        },
    );
    let caught = if m == Mutant::Unresumed {
        t.builder.propagate()
    } else {
        let caught = t.builder.block(BlockKind::Body, &[]);
        t.builder.ret(caught, None);
        caught
    };
    let (failed, kind) = if m == Mutant::Ordinary {
        let failed = t.builder.block(BlockKind::Body, &[]);
        t.builder.ret(failed, None);
        (failed, BlockKind::Body)
    } else {
        (t.builder.propagate(), BlockKind::FailureCleanup)
    };
    let recover = t.builder.block(kind, &[]);
    let caught = t.builder.edge(caught, &[]);
    let failed = t.builder.edge(failed, &[]);
    let arena = if m == Mutant::Foreign { foreign } else { token };
    t.builder.term(
        recover,
        Terminator::Recover {
            arena,
            caught,
            failed,
        },
    );
    if m == Mutant::Ordinary {
        t.builder.jump(ENTRY, recover, &[]);
    } else {
        let raise = t.failure(recover);
        t.builder.unit.tables.failures[raise.index()].rule = Some("MEM-6".into());
        t.builder.term(ENTRY, Terminator::Raise(raise));
    }
    t.check()
}

/// After placement: a library unit (valid) or a source unit (mutant) enters
/// a bounded arena's region, fails, and recovers it, exiting the region on
/// both paths.
fn placed(source: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[(Passing::Borrow, l::U64)], None);
    if !source {
        t.builder.unit.header.origin = UnitOrigin::Library;
    }
    let cap = t.plain(l::U64);
    t.param(cap);
    t.builder.unit.tables.regions.push(RegionInstance {
        scope: ROOT,
        anchor: SourceAnchorId(0),
        seal_demand: false,
    });
    let region = RegionInstanceId(0);
    let mark = t.owned(l::PTR);
    t.builder.push(
        ENTRY,
        Inst::RegionEnter {
            result: mark,
            region,
        },
    );
    let token = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaToken {
            result: token,
            mark,
            budget: Some(cap),
        },
    );
    let exit = |t: &mut Lir<Placed>, b: BlockId| {
        let failure = t.fails();
        t.builder.push(
            b,
            Inst::RegionExit {
                mark,
                region,
                failure,
            },
        );
    };
    let caught = t.builder.block(BlockKind::Body, &[]);
    exit(&mut t, caught);
    t.builder.ret(caught, None);
    let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
    exit(&mut t, failed);
    t.builder.term(failed, Terminator::Propagate);
    let recover = t.builder.block(BlockKind::FailureCleanup, &[]);
    let (caught, failed) = (t.builder.edge(caught, &[]), t.builder.edge(failed, &[]));
    t.builder.term(
        recover,
        Terminator::Recover {
            arena: token,
            caught,
            failed,
        },
    );
    let raise = t.failure(recover);
    t.builder.unit.tables.failures[raise.index()].rule = Some("MEM-6".into());
    t.builder.term(ENTRY, Terminator::Raise(raise));
    t.check()
}

#[test]
fn a_bounded_arena_budget_is_a_u64() {
    pair(
        |m| bounded(if m { Mutant::Budget } else { Mutant::None }),
        |k| *k == K::Type("arena budget"),
    );
}

#[test]
fn only_a_library_body_recovers() {
    pair(placed, |k| *k == K::Vocabulary(Opcode::Recover));
}

#[test]
fn a_recovery_runs_in_failure_cleanup() {
    pair(
        |m| bounded(if m { Mutant::Ordinary } else { Mutant::None }),
        |k| *k == K::BlockKind("recover outside failure cleanup"),
    );
}

#[test]
fn a_recovery_resumes_ordinary_code() {
    pair(
        |m| bounded(if m { Mutant::Unresumed } else { Mutant::None }),
        |k| *k == K::BlockKind("failure path transfer"),
    );
}

#[test]
fn a_recovery_names_a_bounded_arena_the_unit_opened() {
    for mutant in [Mutant::Unbounded, Mutant::Foreign] {
        pair(
            |m| bounded(if m { mutant } else { Mutant::None }),
            |k| *k == K::Placement("a recovery of an arena the unit did not open bounded"),
        );
    }
}
