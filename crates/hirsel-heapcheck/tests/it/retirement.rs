//! Retired storage must preserve storage-version diagnostics when a slot reopens.
use crate::fixtures::run;
use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
use hirsel_heapcheck::{Config, NoOperations, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Ready;
use hirsel_lir::testing::UnitBuilder;

fn reopened_slot(stale: bool) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let word = layouts.scalar(ScalarKind::U64);
    let (target, layouts) = layouts.finish();
    let repr = ValueRepr::Scalar(ScalarKind::U64);
    let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
    let entry = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    let slot = b.slot(repr, word, SlotKind::Local);
    let address = b.address(AddressBase::Slot(slot), &[], repr, word);
    let constant = b.constant(repr, word, ConstantValue::Integer(7));
    let value = b.plain(repr, word);
    let old = b.plain(repr, word);
    let current = b.plain(repr, word);
    b.push(
        entry,
        Inst::Const {
            result: value,
            constant,
        },
    );
    for version in [old, current] {
        b.push(entry, Inst::StorageLive { slot });
        b.push(entry, Inst::Address { address, failure });
        b.push(
            entry,
            Inst::Store {
                address,
                value,
                version,
            },
        );
        if version == old {
            b.push(
                entry,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
        }
    }
    let result = b.plain(repr, word);
    b.push(
        entry,
        Inst::Read {
            result,
            address,
            reaching: if stale { old } else { current },
            mode: ReadMode::Borrow,
        },
    );
    b.push(
        entry,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    b.term(entry, Terminator::Return(None));
    Program {
        invocations: Default::default(),
            invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        units: vec![b.finish_with_layouts(&layouts)],
        layouts,
        statics: vec![],
    }
}

#[test]
fn stale_version_after_slot_reopening_keeps_its_diagnostic() {
    let stale = run(
        &reopened_slot(true),
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    )
    .result
    .unwrap_err();
    assert_eq!(stale.kind, ViolationKind::Malformed);
    assert_eq!(
        stale.message,
        "read names ValueId(1) but SlotId(0) holds ValueId(2)"
    );
    run(
        &reopened_slot(false),
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    )
    .result
    .unwrap();
}
