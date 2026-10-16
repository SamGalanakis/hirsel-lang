//! LIR schema revision 8: drop flags. A flag is a bool block parameter that is true exactly
//! when its slot holds content on entry; each argument is a constant matching the slot's
//! state on its edge or the slot's current flag, a branch on the current flag decides the
//! slot per edge, and a whole `Replace` of a live slot hands its flag to the displaced
//! slot. Each rule has its rejected mutation and accepted twin built by one function.
use crate::support;

use hirsel_lir::ir::*;
use hirsel_lir::stage::Ready;
use support::*;

/// A slot stored at entry and taken on one arm only, so it holds content at
/// the join on one path. The join's flag is `true` from the arm that kept the
/// content and `false` from the one that took it (`true` in the mutant); a
/// branch on it then takes the content only where it is there.
fn flag_argument(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let v = t.int(ENTRY);
    let first = t.store(ENTRY, a, v, s);
    let (took, kept) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    t.branch(ENTRY, (took, &[]), (kept, &[]));
    let p = t.flag(s);
    let join = t.block(BlockKind::Body, &[p]);
    t.read(took, a, first, ReadMode::Take, None);
    let taken = t.bool_const(took, mutant);
    t.jump(took, join, &[taken]);
    let there = t.bool_const(kept, true);
    t.jump(kept, join, &[there]);
    let (full, empty, end) = (
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
    );
    t.branch_on(join, p, (full, &[]), (empty, &[]));
    t.read(full, a, first, ReadMode::Take, None);
    t.jump(full, end, &[]);
    t.jump(empty, end, &[]);
    t.push(
        end,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(end, None);
    t.check()
}

#[test]
fn a_flag_argument_is_a_constant_matching_its_edge() {
    pair(flag_argument, |k| matches!(k, K::InitFlag { .. }));
}

/// The flag stays current where nothing changed the slot (the empty arm
/// forwards it), and ends where the slot changed: the full arm took the
/// content, so forwarding the old flag there (the mutant) is stale.
fn stale_flag(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let v = t.int(ENTRY);
    let first = t.store(ENTRY, a, v, s);
    let (took, kept) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    t.branch(ENTRY, (took, &[]), (kept, &[]));
    let p = t.flag(s);
    let join = t.block(BlockKind::Body, &[p]);
    t.read(took, a, first, ReadMode::Take, None);
    let taken = t.bool_const(took, false);
    t.jump(took, join, &[taken]);
    let there = t.bool_const(kept, true);
    t.jump(kept, join, &[there]);
    let (full, empty) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    let q = t.flag(s);
    let end = t.block(BlockKind::Body, &[q]);
    t.branch_on(join, p, (full, &[]), (empty, &[]));
    t.read(full, a, first, ReadMode::Take, None);
    let now = if mutant { p } else { t.bool_const(full, false) };
    t.jump(full, end, &[now]);
    t.jump(empty, end, &[p]);
    t.push(
        end,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(end, None);
    t.check()
}

#[test]
fn a_flag_is_stale_once_its_slot_changes() {
    pair(stale_flag, |k| matches!(k, K::InitFlag { .. }));
}

/// A `Replace` that may displace nothing hands the slot's flag to the
/// displaced slot, so a branch on it decides the displaced content. Taking
/// that content where the flag is false (the mutant) reads what is not there.
fn displaced_flag(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let v = t.int(ENTRY);
    let first = t.store(ENTRY, a, v, s);
    let (took, kept) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    t.branch(ENTRY, (took, &[]), (kept, &[]));
    let p = t.flag(s);
    let join = t.block(BlockKind::Body, &[p]);
    t.read(took, a, first, ReadMode::Take, None);
    let taken = t.bool_const(took, false);
    t.jump(took, join, &[taken]);
    let there = t.bool_const(kept, true);
    t.jump(kept, join, &[there]);
    t.push(join, Inst::StorageLive { slot: d });
    let ad = t.at(join, d, &[], &[]);
    let w = t.int(join);
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.push(
        join,
        Inst::Replace {
            address: a,
            value: w,
            displaced: d,
            version: second,
            displaced_version,
            if_live: true,
            failure,
        },
    );
    let (full, empty, end) = (
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
    );
    t.branch_on(join, p, (full, &[]), (empty, &[]));
    let reader = if mutant { empty } else { full };
    t.read(reader, ad, displaced_version, ReadMode::Take, None);
    t.jump(full, end, &[]);
    t.jump(empty, end, &[]);
    for slot in [d, s] {
        t.push(
            end,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    t.ret(end, None);
    t.check()
}

#[test]
fn a_replace_hands_its_slot_s_flag_to_the_displaced_slot() {
    pair(displaced_flag, |k| matches!(k, K::Uninitialized(_)));
}

/// A flag is a block parameter: a constant named as one is refused.
fn flag_parameter(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let c = t.bool_const(ENTRY, false);
    if mutant {
        t.unit.tables.init_flags.push(InitFlag {
            path: PoolRange::EMPTY,
            value: c,
            slot: s,
            environment: None,
        });
    }
    t.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_flag_is_a_bool_block_parameter() {
    pair(flag_parameter, |k| matches!(k, K::InitFlag { .. }));
}

/// A version join on an edge where the slot holds nothing names no content:
/// the arm that took it passes the version it took, and nothing reads it.
/// Where the arm stored again (the mutant), the slot holds content and the
/// stale version is refused.
fn empty_edge_version(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let v = t.int(ENTRY);
    let first = t.store(ENTRY, a, v, s);
    let (took, kept) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    t.branch(ENTRY, (took, &[]), (kept, &[]));
    let p = t.flag(s);
    let q = t.version_of(s);
    let join = t.block(BlockKind::Body, &[p, q]);
    t.read(took, a, first, ReadMode::Take, None);
    let held = if mutant {
        let w = t.int(took);
        t.store(took, a, w, s);
        true
    } else {
        false
    };
    let flag = t.bool_const(took, held);
    t.jump(took, join, &[flag, first]);
    let there = t.bool_const(kept, true);
    t.jump(kept, join, &[there, first]);
    let (full, empty, end) = (
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
    );
    t.branch_on(join, p, (full, &[]), (empty, &[]));
    t.read(full, a, q, ReadMode::Take, None);
    t.jump(full, end, &[]);
    t.jump(empty, end, &[]);
    t.push(
        end,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(end, None);
    t.check()
}

#[test]
fn an_empty_edge_s_version_names_no_content() {
    pair(empty_edge_version, |k| matches!(k, K::VersionJoin { .. }));
}

/// Content stored on one arm only reaches the join under the version that
/// arm stored last, which dominates neither the join nor the read the flag
/// guards: a reaching version is a name the ownership check pins, not an
/// operand. Naming the arm's earlier version (the mutant) is stale.
fn undominated_reaching(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let (stored, skipped) = (t.block(BlockKind::Body, &[]), t.block(BlockKind::Body, &[]));
    t.branch(ENTRY, (stored, &[]), (skipped, &[]));
    let p = t.flag(s);
    let join = t.block(BlockKind::Body, &[p]);
    let v = t.int(stored);
    let first = t.store(stored, a, v, s);
    t.read(stored, a, first, ReadMode::Take, None);
    let w = t.int(stored);
    let second = t.store(stored, a, w, s);
    let there = t.bool_const(stored, true);
    t.jump(stored, join, &[there]);
    let absent = t.bool_const(skipped, false);
    t.jump(skipped, join, &[absent]);
    let (full, empty, end) = (
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
        t.block(BlockKind::Body, &[]),
    );
    t.branch_on(join, p, (full, &[]), (empty, &[]));
    t.read(
        full,
        a,
        if mutant { first } else { second },
        ReadMode::Take,
        None,
    );
    t.jump(full, end, &[]);
    t.jump(empty, end, &[]);
    t.push(
        end,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(end, None);
    t.check()
}

#[test]
fn a_reaching_version_need_not_dominate_its_read() {
    pair(undominated_reaching, |k| {
        matches!(k, K::StaleVersion { .. })
    });
}
