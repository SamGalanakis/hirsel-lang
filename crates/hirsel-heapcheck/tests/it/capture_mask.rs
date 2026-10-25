//! A closure's live word is a distinct logical field beside its resources.
use crate::fixtures;
use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{DropStep, LayoutKind, TagForm, TargetSpec};
use hirsel_lir::stage::Ready;

fn program(empty_resource: bool, overwrite: bool) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let common = common(&mut layouts);
    let resource_fields = if empty_resource {
        vec![]
    } else {
        vec![common.u64]
    };
    let resource = layouts.resource(&resource_fields, None);
    let environment = layouts.record(&[resource, common.u64]);
    let word_offset = layouts.get(environment).fields[1].offset;
    let record = layouts.get_mut(environment);
    record.kind = LayoutKind::Environment { movable: vec![0] };
    record.drop = vec![DropStep::ActiveVariant {
        tag_offset: word_offset,
        form: TagForm::LiveBit { bit: 0 },
        variants: vec![(
            1,
            vec![DropStep::Child {
                offset: 0,
                layout: resource,
            }],
        )],
    }];
    let callable = layouts.callable(Some(environment));
    let (target, table) = layouts.finish();
    let mut main = F::<Ready>::new("main", common);
    let fields = if empty_resource {
        vec![]
    } else {
        vec![(0, main.int(7))]
    };
    let construct = main
        .b
        .construct(ConstructorKind::Tuple, resource, &fields, HEAP);
    let captured = main.b.owned(ValueRepr::Aggregate, resource);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: captured,
        construct,
        failure,
    });
    let closure = main.b.closure(
        path("body"),
        environment,
        &[(captured, CaptureUse::Take)],
        HEAP,
    );
    let value = main.b.owned(ValueRepr::Callable, callable);
    let failure = main.propagates();
    main.inst(Inst::Closure {
        result: value,
        closure,
        failure,
    });
    if overwrite {
        let slot = main.b.slot(U64, common.u64, SlotKind::Local);
        let address = main
            .b
            .address(AddressBase::Slot(slot), &[], U64, common.u64);
        main.inst(Inst::StorageLive { slot });
        for n in [1, 2] {
            let value = main.int(n);
            main.inst(Inst::Store {
                address,
                value,
                version: value,
            });
        }
    }
    main.release(value, callable);
    main.ret(None);
    let mut body = F::<Ready>::new("body", common);
    body.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![main.finish(&table), body.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn live_word_initialization_does_not_overlap_a_captured_resource() {
    for empty in [false, true] {
        for full in [false, true] {
            let run = run(
                &program(empty, false),
                &path("main"),
                &[],
                &mut Printer,
                &Config {
                    full_recount_every_step: full,
                    ..Config::default()
                },
            );
            run.result
                .unwrap_or_else(|error| panic!("empty={empty} full={full}: {error}"));
        }
    }
}

#[test]
fn an_actual_store_over_initialized_storage_is_still_rejected() {
    let error = run(
        &program(false, true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    )
    .result
    .unwrap_err();
    assert_eq!(error.kind, ViolationKind::StoreOverInitialized);
    assert!(
        error.message.ends_with("+0"),
        "the real overwrite, not the live word: {error}"
    );
}

/// A once callback that takes its boxed capture (`let mut tx = take sender`
/// inside `arena.with`) and releases it; the caller then releases the
/// callable. With `clear`, the body clears the capture's live bit as the RC
/// import's capture take does; without it the environment still claims the
/// moved capture, so its release would free the box a second time.
fn taken_capture(clear: bool) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let common = common(&mut layouts);
    let boxed = common.boxed;
    let environment = layouts.record(&[boxed, common.u64]);
    let word_offset = layouts.get(environment).fields[1].offset;
    let record = layouts.get_mut(environment);
    record.kind = LayoutKind::Environment { movable: vec![0] };
    record.drop = vec![DropStep::ActiveVariant {
        tag_offset: word_offset,
        form: TagForm::LiveBit { bit: 0 },
        variants: vec![(1, vec![DropStep::Release { offset: 0 }])],
    }];
    let callable = layouts.callable(Some(environment));
    let (target, table) = layouts.finish();

    let mut body = F::<Ready>::new("body", common);
    let capture = body.b.slot(BLOCK, boxed, SlotKind::Capture(0));
    let word = body.b.slot(U64, common.u64, SlotKind::Capture(1));
    let (held, live) = (body.b.unit.captures[0], body.b.unit.captures[1]);
    let at = body
        .b
        .address(AddressBase::Slot(capture), &[], BLOCK, boxed);
    let failure = body.propagates();
    body.inst(Inst::Address {
        address: at,
        failure,
    });
    let taken = body.b.owned(BLOCK, boxed);
    body.inst(Inst::Read {
        result: taken,
        address: at,
        reaching: held,
        mode: ReadMode::Take,
    });
    if clear {
        let at = body
            .b
            .address(AddressBase::Slot(word), &[], U64, common.u64);
        let failure = body.propagates();
        body.inst(Inst::Address {
            address: at,
            failure,
        });
        let old = body.b.plain(U64, common.u64);
        body.inst(Inst::Read {
            result: old,
            address: at,
            reaching: live,
            mode: ReadMode::Bits,
        });
        let new = body.b.plain(U64, common.u64);
        body.inst(Inst::UpdateMask {
            result: new,
            mask: old,
            field: 0,
            initialized: false,
        });
        let version = body.b.plain(U64, common.u64);
        body.inst(Inst::Store {
            address: at,
            value: new,
            version,
        });
    }
    let n = body.unbox(taken);
    body.print(n);
    body.release(taken, boxed);
    body.ret(None);

    let mut main = F::<Ready>::new("main", common);
    let five = main.int(5);
    let sender = main.boxed(five);
    let closure = main.b.closure(
        path("body"),
        environment,
        &[(sender, CaptureUse::Take)],
        HEAP,
    );
    let value = main.b.owned(ValueRepr::Callable, callable);
    let failure = main.propagates();
    main.inst(Inst::Closure {
        result: value,
        closure,
        failure,
    });
    main.call(Callee::Indirect(value), &[], None);
    main.release(value, callable);
    main.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![main.finish(&table), body.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn a_taken_capture_is_released_once_and_a_stale_live_bit_is_refused() {
    let run = |clear| {
        run(
            &taken_capture(clear),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        )
    };
    let once = run(true);
    let audit = once
        .result
        .as_ref()
        .unwrap_or_else(|error| panic!("the cleared take is clean: {error}"));
    assert_eq!(audit.observation.output, b"5\n");
    assert_eq!((once.counters.allocations, once.counters.frees), (2, 2));
    let error = run(false)
        .result
        .expect_err("the environment still owns the taken box");
    assert_eq!(error.kind, ViolationKind::UninitializedScan, "{error}");
    assert!(
        error.message.contains("capture live bit disagrees"),
        "{error}"
    );
}
