//! Revision-8 empty storage transports a version marker without reading content.
use crate::fixtures;
use fixtures::run;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_heapcheck::build::{Layouts, UnitBuilder, path};
use hirsel_heapcheck::{Config, NoOperations, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Ready;

fn control(initialized: u8) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let word = layouts.scalar(ScalarKind::U64);
    let record = layouts.record(&[word, word]);
    let (target, table) = layouts.finish();
    let mut b = UnitBuilder::<Ready>::new(path("main"));
    let entry = b.block(BlockKind::Body, &[]);
    let unused = b.block(BlockKind::Body, &[]);
    let slot = b.slot(ValueRepr::Aggregate, record, SlotKind::Local);
    let address = b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, record);
    b.push(entry, Inst::StorageLive { slot });
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.end(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    if initialized != 0 {
        let value = b.plain(ValueRepr::Scalar(ScalarKind::U64), word);
        let constant = b.constant(
            ValueRepr::Scalar(ScalarKind::U64),
            word,
            ConstantValue::Integer(1),
        );
        b.push(
            entry,
            Inst::Const {
                result: value,
                constant,
            },
        );
        for field in 0..initialized {
            let part = b.address(
                AddressBase::Slot(slot),
                &[ProjectionStep::Field(field as u32)],
                ValueRepr::Scalar(ScalarKind::U64),
                word,
            );
            b.push(
                entry,
                Inst::Address {
                    address: part,
                    failure,
                },
            );
            let version = b.plain(ValueRepr::Aggregate, record);
            b.push(
                entry,
                Inst::Store {
                    address: part,
                    value,
                    version,
                },
            );
        }
    }
    // This version is declared by an unexecuted mut call. The frame has no
    // runtime value for it; only storage-free edges may carry its marker.
    let future = b.plain(ValueRepr::Aggregate, record);
    let op = b.operation(OperationId {
        definition: path("unused_mut"),
        declaration_digest: Default::default(),
    });
    let call = b.call(
        op,
        &[(
            Passing::Mut,
            ArgumentValue::Address {
                address,
                version: future,
            },
        )],
        None,
    );
    let phi = b.plain(ValueRepr::Aggregate, record);
    let joined = b.block(BlockKind::Body, &[phi]);
    let normal = b.edge(joined, &[future]);
    b.end(
        unused,
        Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        },
    );
    b.end(entry, Terminator::Jump(normal));
    b.push(
        joined,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    b.end(joined, Terminator::Return(None));
    Program {
        target,
        layouts: table.clone(),
        units: vec![b.finish(&table)],
        statics: vec![],
    }
}
#[test]
fn an_undefined_version_can_cross_only_empty_storage() {
    let empty = control(0);
    run(
        &empty,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    )
    .result
    .unwrap();
    for initialized in [1, 2] {
        let program = control(initialized);
        let violation = run(
            &program,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config::default(),
        )
        .result
        .unwrap_err();
        assert_eq!(violation.kind, ViolationKind::Malformed);
        assert!(
            violation
                .message
                .contains("undefined version of initialized storage"),
            "{violation}"
        );
    }
}
