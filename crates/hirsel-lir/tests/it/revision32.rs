//! LIR schema revision 32 (MEM-6): a List in a bounded arena keeps its
//! budget. `IsWritable` tests a block for a write, `Share` marks the blocks a
//! semantic copy holds, and `ArenaPush` prepares one `list.push`. Each
//! mutation is paired with its valid twin and is rejected by the rule it names
//! alone, so with that rule removed C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Rc, Value};
use support::*;

/// `pair`, and the mutant breaks no rule but the expected one.
fn only(case: impl Fn(bool) -> Vec<K>, expected: K) {
    pair(&case, |k| *k == expected);
    let mutant = case(true);
    assert!(
        mutant.iter().all(|k| *k == expected),
        "the mutant breaks another rule: {mutant:#?}"
    );
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Test {
    Valid,
    /// The tested value is a u64, which holds no count.
    Uncounted,
    /// The result is a u64.
    Result,
}

/// `IsWritable` of a borrowed List block parameter, or of a u64 parameter.
fn writable(case: Test) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::SEQBLK), (Passing::Borrow, l::U64)],
        None,
    );
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let scalar = t.plain(l::U64);
    t.param(scalar);
    let at = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(at, false);
    t.begin(ENTRY, loan);
    let block = t.read(ENTRY, at, version, ReadMode::Borrow, Some(loan));
    let value = if case == Test::Uncounted {
        scalar
    } else {
        block
    };
    let result = t.plain(if case == Test::Result {
        l::U64
    } else {
        l::BOOL
    });
    t.builder.push(ENTRY, Inst::IsWritable { result, value });
    t.end(ENTRY, loan);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_writability_test_reads_a_block_into_a_bool() {
    only(
        |m| writable(if m { Test::Uncounted } else { Test::Valid }),
        K::Type("writability test of an uncounted value"),
    );
    only(
        |m| writable(if m { Test::Result } else { Test::Valid }),
        K::Type("writability test result"),
    );
}

/// `Share` of a borrowed List block parameter, or of a u64 one.
fn share(scalar: bool) -> Vec<K> {
    let layout = if scalar { l::U64 } else { l::SEQBLK };
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, layout)], None);
    let value = t.plain(layout);
    t.param(value);
    t.builder.push(ENTRY, Inst::Share { value });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_share_names_a_value_that_holds_a_block() {
    only(share, K::Type("share of a value that holds no block"));
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Push {
    Valid,
    /// A source unit's push preparation.
    Source,
    /// A place holding a record block, not a List block.
    Record,
}

/// `ArenaPush` through a `mut` parameter of a List block, as `list.push`
/// begins.
fn push(case: Push) -> Vec<K> {
    let layout = if case == Push::Record {
        l::BLK
    } else {
        l::SEQBLK
    };
    let mut t = Lir::<Value>::new(&[(Passing::Mut, layout)], None);
    if case != Push::Source {
        t.builder.unit.header.origin = UnitOrigin::Library;
    }
    let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(slot);
    t.param(first);
    let address = t.at(ENTRY, slot, &[], &[]);
    let version = t.version_of(slot);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::ArenaPush {
            address,
            count: None,
            version,
            failure,
        },
    );
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn only_a_library_body_prepares_an_arena_push() {
    only(
        |m| push(if m { Push::Source } else { Push::Valid }),
        K::Vocabulary(Opcode::ArenaPush),
    );
}

#[test]
fn an_arena_push_names_a_place_that_holds_a_list_block() {
    only(
        |m| push(if m { Push::Record } else { Push::Valid }),
        K::Type("arena push of a place that holds no List block"),
    );
}
