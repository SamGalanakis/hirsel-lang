//! LIR schema revision 34 (MEM-6): an Io read into an arena List is charged
//! before its effect. An `ArenaPush` with a count prepares that many appended
//! elements in any unit; its count is a u64. The mutation is paired with its
//! valid twin and is rejected by the rule it names alone, so with that rule
//! removed C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use support::*;

/// A source unit's `ArenaPush` of a count through a `mut` List block
/// parameter, as import places one before `stdin.read`; the count is a u64
/// parameter, or a bool one.
fn reserve(boolean: bool) -> Vec<K> {
    let count_layout = if boolean { l::BOOL } else { l::U64 };
    let mut t = Lir::<Value>::new(
        &[(Passing::Mut, l::SEQBLK), (Passing::Borrow, count_layout)],
        None,
    );
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(slot);
    t.param(first);
    let count = t.plain(count_layout);
    t.param(count);
    let address = t.at(ENTRY, slot, &[], &[]);
    let version = t.version_of(slot);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::ArenaPush {
            address,
            count: Some(count),
            version,
            failure,
        },
    );
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn an_arena_push_counts_its_appends_in_a_u64() {
    pair(&reserve, |k| *k == K::Type("arena push count"));
    let mutant = reserve(true);
    assert!(
        mutant.iter().all(|k| *k == K::Type("arena push count")),
        "the mutant breaks another rule: {mutant:#?}"
    );
}
