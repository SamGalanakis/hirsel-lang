//! LIR schema revision 31 (FAIL-2): `task.isolate`'s catch. Each mutation is
//! paired with its valid twin and is rejected by the rule it names; with that
//! rule removed, C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use support::*;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mutant {
    None,
    /// A start that is not a u64 record count.
    Start,
    /// A catch in ordinary code, reached by an ordinary edge.
    Ordinary,
    /// A catch whose caught edge stays in failure cleanup.
    Unresumed,
    /// A catch in a source unit.
    Source,
}

/// `task.isolate`'s shape: the record count arrives as a u64, the body fails,
/// and its cleanup catches the failures since that count and returns.
fn isolated(m: Mutant) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::U64)], None);
    if m != Mutant::Source {
        t.builder.unit.header.origin = UnitOrigin::Library;
    }
    let count = t.plain(l::U64);
    t.param(count);
    let start = if m == Mutant::Start {
        t.int(ENTRY)
    } else {
        count
    };
    let caught = if m == Mutant::Unresumed {
        t.builder.propagate()
    } else {
        let caught = t.builder.block(BlockKind::Body, &[]);
        t.builder.ret(caught, None);
        caught
    };
    let kind = if m == Mutant::Ordinary {
        BlockKind::Body
    } else {
        BlockKind::FailureCleanup
    };
    let catch = t.builder.block(kind, &[]);
    let caught = t.builder.edge(caught, &[]);
    t.builder.term(catch, Terminator::Isolate { start, caught });
    if m == Mutant::Ordinary {
        t.builder.jump(ENTRY, catch, &[]);
    } else {
        let raise = t.failure(catch);
        t.builder.unit.tables.failures[raise.index()].rule = Some("SEM-1".into());
        t.builder.term(ENTRY, Terminator::Raise(raise));
    }
    t.check()
}

#[test]
fn an_isolation_starts_from_a_u64_count() {
    pair(
        |m| isolated(if m { Mutant::Start } else { Mutant::None }),
        |k| *k == K::Type("isolation start"),
    );
}

#[test]
fn only_a_library_body_isolates() {
    pair(
        |m| isolated(if m { Mutant::Source } else { Mutant::None }),
        |k| *k == K::Vocabulary(Opcode::Isolate),
    );
}

#[test]
fn an_isolation_catches_in_failure_cleanup() {
    pair(
        |m| isolated(if m { Mutant::Ordinary } else { Mutant::None }),
        |k| *k == K::BlockKind("isolate outside failure cleanup"),
    );
}

#[test]
fn an_isolation_resumes_ordinary_code() {
    pair(
        |m| isolated(if m { Mutant::Unresumed } else { Mutant::None }),
        |k| *k == K::BlockKind("failure path transfer"),
    );
}
