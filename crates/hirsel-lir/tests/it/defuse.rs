//! Hand-built C controls for whole-resource finalizer suppression (#97).
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Ready;
use support::*;

fn defuse(missing: bool, twice: bool, partial: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::BLK)], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let slot = t.slot(l::RESCNT, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot });
    let at = t.at(ENTRY, slot, &[], &[Undo::Release(x)]);
    let field = t.at(
        ENTRY,
        slot,
        &[ProjectionStep::Field(1)],
        &[Undo::Release(x)],
    );
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, n), (1, x)],
        placement,
    );
    let r = t.owned(l::RESCNT);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct,
            failure,
        },
    );
    let version = t.store(ENTRY, at, r, slot);
    if partial {
        let field_value = t.read(ENTRY, field, version, ReadMode::Take, None);
        t.release(ENTRY, field_value, &[]);
    }
    if !missing {
        t.push(ENTRY, Inst::Defuse { address: at });
        if twice {
            t.push(ENTRY, Inst::Defuse { address: at });
        }
    }
    let field_value = (!partial).then(|| t.read(ENTRY, field, version, ReadMode::Take, None));
    t.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    if let Some(field_value) = field_value {
        t.release(ENTRY, field_value, &[]);
    }
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn field_distribution_requires_explicit_defuse() {
    pair(
        |bad| defuse(bad, false, false),
        |k| matches!(k, K::FinalizeState(_)),
    );
}
#[test]
fn a_resource_cannot_be_defused_twice() {
    pair(
        |bad| defuse(false, bad, false),
        |k| matches!(k, K::FinalizeState(_)),
    );
}
#[test]
fn defuse_requires_a_complete_live_resource() {
    pair(
        |bad| defuse(false, false, bad),
        |k| matches!(k, K::MovedPart(_)),
    );
}

#[test]
fn defuse_requires_a_layout_with_a_finalizer() {
    pair(
        |mutant| {
            let mut t = Lir::<Ready>::new(&[], None);
            let slot = t.slot(l::I64, SlotKind::Local, ROOT);
            t.push(ENTRY, Inst::StorageLive { slot });
            let at = t.at(ENTRY, slot, &[], &[]);
            let n = t.int(ENTRY);
            t.store(ENTRY, at, n, slot);
            if mutant {
                t.push(ENTRY, Inst::Defuse { address: at });
            }
            t.push(
                ENTRY,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
            t.ret(ENTRY, None);
            t.check()
        },
        |k| matches!(k, K::Type("finalization of a layout without finalizer")),
    );
}

/// A fatal continuation has no cleanup obligations, so only the region rule
/// rejects this retained FieldsPending / Live join. The accepted twin empties
/// both predecessors before their first join.
fn retained_join(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::BLK)], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let slot = t.slot(l::RESCNT, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot });
    let at = t.at(ENTRY, slot, &[], &[Undo::Release(x)]);
    let field = t.at(
        ENTRY,
        slot,
        &[ProjectionStep::Field(1)],
        &[Undo::Release(x)],
    );
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, n), (1, x)],
        placement,
    );
    let r = t.owned(l::RESCNT);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct,
            failure,
        },
    );
    let version = t.store(ENTRY, at, r, slot);
    let yes = t.block(BlockKind::Body, &[]);
    let no = t.block(BlockKind::Body, &[]);
    let joined = t.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    t.push(yes, Inst::Defuse { address: at });
    if !mutant {
        let anchor = t.cleanup_anchor();
        t.push(
            no,
            Inst::FinalizeBegin {
                address: at,
                anchor,
            },
        );
        t.push(
            no,
            Inst::FinalizeEnd {
                address: at,
                anchor,
            },
        );
        for block in [yes, no] {
            let value = t.read(block, field, version, ReadMode::Take, None);
            t.push(
                block,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
            t.release(block, value, &[]);
        }
    }
    t.jump(yes, joined, &[]);
    t.jump(no, joined, &[]);
    t.term(
        joined,
        Terminator::InvariantFault {
            reason: "fixture terminal guard".into(),
        },
    );
    t.check()
}

#[test]
fn a_defused_shell_must_be_empty_before_its_first_join() {
    pair(retained_join, |k| matches!(k, K::FinalizeState(_)));
}

#[test]
fn a_defused_shell_cannot_be_transferred_as_a_whole_resource() {
    pair(
        |mutant| {
            let mut t = Lir::<Ready>::new(&[], Some(l::RES));
            let slot = t.slot(l::RES, SlotKind::Local, ROOT);
            t.push(ENTRY, Inst::StorageLive { slot });
            let at = t.at(ENTRY, slot, &[], &[]);
            let n = t.int(ENTRY);
            let symbol = t.symbol(SymbolKind::Type);
            let placement = t.heap();
            let construct = t.construct_site(
                ConstructorKind::Record(symbol),
                l::RES,
                &[(0, n)],
                placement,
            );
            let r = t.owned(l::RES);
            let failure = t.fails();
            t.push(
                ENTRY,
                Inst::Construct {
                    result: r,
                    construct,
                    failure,
                },
            );
            let version = t.store(ENTRY, at, r, slot);
            if mutant {
                t.push(ENTRY, Inst::Defuse { address: at });
            }
            let result = t.read(ENTRY, at, version, ReadMode::Take, None);
            t.push(
                ENTRY,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
            t.ret(ENTRY, Some(result));
            t.check()
        },
        |k| matches!(k, K::FinalizeState(_)),
    );
}
