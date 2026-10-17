//! Source-free controls for ownership transferred from an entry to its harness.
use hirsel_core_contract::Manifest;
use hirsel_heapcheck::build::{CLEANUP, Layouts, fixture_unit, path};
use hirsel_heapcheck::{Config, NoOperations, Observed, Program, ViolationKind};
use hirsel_lir::abi_types::AbiResult;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Rc;
use hirsel_lir::testing::UnitBuilder;

fn propagates(b: &mut UnitBuilder<Rc>, target: BlockId) -> FailureId {
    let edge = b.edge(target, &[]);
    b.failure(edge, None, &[])
}

fn returning_block(leak: bool, regional: bool) -> Program<Rc> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let number = layouts.scalar(ScalarKind::U64);
    let boolean = layouts.scalar(ScalarKind::Bool);
    let payload = layouts.record(&[number]);
    let block = layouts.block(payload);
    let regional_ref = block;
    layouts.table.fn_abis[0].result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: block,
        slots: vec![hirsel_lir::abi::slot(&layouts.target, ScalarKind::Pointer)],
    });
    let mut abi = layouts.table.fn_abis[0].clone();
    abi.result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: number,
        slots: vec![hirsel_lir::abi::slot(&layouts.target, ScalarKind::Pointer)],
    });
    layouts.table.fn_abis.push(abi);
    let (target, table) = layouts.finish();
    let mut b = UnitBuilder::<Rc>::new(fixture_unit(path("region_return")));
    if !regional {
        b.unit.tables.cleanup_anchors.clear();
    }
    let body = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let repr = ValueRepr::Scalar(ScalarKind::U64);
    let value = b.plain(repr, number);
    let constant = b.constant(repr, number, ConstantValue::Integer(7));
    b.push(
        body,
        Inst::Const {
            result: value,
            constant,
        },
    );
    let mark = regional.then(|| {
        let region = b.region();
        let mark = b.owned(ValueRepr::RegionMark, number);
        b.push(
            body,
            Inst::RegionEnter {
                result: mark,
                region,
            },
        );
        let local = b.plain(ValueRepr::Uncounted, regional_ref);
        let construct = b.construct(
            ConstructorKind::Tuple,
            payload,
            &[(0, value)],
            Placement::Fixed(StorageOwner::LocalRegion(region)),
        );
        b.unit.tables.constructs[construct.index()].destination = DestinationClass::Region;
        let failure = propagates(&mut b, failed);
        b.push(
            body,
            Inst::Construct {
                result: local,
                construct,
                failure,
            },
        );
        b.push(body, Inst::RegionPublish { value: local });
        let slot = b.slot(ValueRepr::Uncounted, regional_ref, SlotKind::Local);
        let address = b.address(
            AddressBase::Slot(slot),
            &[],
            ValueRepr::Uncounted,
            regional_ref,
        );
        let failure = propagates(&mut b, failed);
        b.push(body, Inst::Address { address, failure });
        b.push(body, Inst::StorageLive { slot });
        let version = b.plain(ValueRepr::Uncounted, regional_ref);
        b.push(
            body,
            Inst::Store {
                address,
                value: local,
                version,
            },
        );
        b.push(
            body,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        (mark, region)
    });
    let returned = b.owned(ValueRepr::Block, block);
    let construct = b.construct(
        ConstructorKind::Tuple,
        payload,
        &[(0, value)],
        Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))),
    );
    let failure = propagates(&mut b, failed);
    b.push(
        body,
        Inst::Construct {
            result: returned,
            construct,
            failure,
        },
    );
    if leak {
        b.push(body, Inst::Dup { value: returned });
    }
    if let Some((mark, region)) = mark {
        let end = b.block(BlockKind::FailureCleanup, &[]);
        b.term(end, Terminator::Propagate);
        let drop_return = b.block(BlockKind::FailureCleanup, &[]);
        let failure = propagates(&mut b, end);
        b.push(
            drop_return,
            Inst::Release {
                value: returned,
                layout: payload,
                live: MaskRef::All,
                anchor: CLEANUP,
                failure,
            },
        );
        b.term(drop_return, Terminator::Propagate);
        let exit_failure = propagates(&mut b, drop_return);
        b.push(
            body,
            Inst::RegionExit {
                mark,
                region,
                failure: exit_failure,
            },
        );
        // A construction failure still needs to close the active region.
        let exit_failure = propagates(&mut b, end);
        b.push(
            failed,
            Inst::RegionExit {
                mark,
                region,
                failure: exit_failure,
            },
        );
        let condition = b.plain(ValueRepr::Scalar(ScalarKind::Bool), boolean);
        let constant = b.constant(
            ValueRepr::Scalar(ScalarKind::Bool),
            boolean,
            ConstantValue::Bool(true),
        );
        b.push(
            body,
            Inst::Const {
                result: condition,
                constant,
            },
        );
        let failure = propagates(&mut b, drop_return);
        b.push(body, Inst::Check { condition, failure });
    }
    b.term(body, Terminator::Return(Some(returned)));
    let unit = b.finish_with_layouts(&table);
    if !leak {
        let manifest = Manifest::skeleton([]);
        let violations = hirsel_lir::validate::check(
            &unit,
            &hirsel_lir::validate::Inputs {
                discharge: None,
                operations: &(),
                target: &target,
                layouts: &table,
                manifest: &manifest,
            },
        );
        assert!(violations.is_empty(), "{violations:?}");
    }
    let mut caller = UnitBuilder::<Rc>::new(fixture_unit(path("entry")));
    caller.unit.header.abi = hirsel_lir::abi_types::FnAbiId(1);
    let entry = caller.block(BlockKind::Body, &[]);
    let normal = caller.block(BlockKind::Body, &[]);
    let failed = caller.block(BlockKind::FailureCleanup, &[]);
    caller.term(failed, Terminator::Propagate);
    let failure = propagates(&mut caller, failed);
    let returned = caller.owned(ValueRepr::Block, block);
    let callee = caller.direct(path("region_return"));
    let call = caller.call(callee, &[], Some(returned));
    caller.unit.tables.calls[call.index()].destination = None;
    let normal_edge = caller.edge(normal, &[]);
    caller.term(
        entry,
        Terminator::Call {
            call,
            normal: Some(normal_edge),
            failure,
        },
    );
    let field = caller.address(
        AddressBase::Value(returned),
        &[ProjectionStep::Field(0)],
        repr,
        number,
    );
    let drop_return = caller.block(BlockKind::FailureCleanup, &[]);
    let failure = propagates(&mut caller, failed);
    caller.push(
        drop_return,
        Inst::Release {
            value: returned,
            layout: payload,
            live: MaskRef::All,
            anchor: CLEANUP,
            failure,
        },
    );
    caller.term(drop_return, Terminator::Propagate);
    let failure = propagates(&mut caller, drop_return);
    caller.push(
        normal,
        Inst::Address {
            address: field,
            failure,
        },
    );
    let answer = caller.plain(repr, number);
    caller.push(
        normal,
        Inst::Read {
            result: answer,
            address: field,
            reaching: returned,
            mode: ReadMode::Bits,
        },
    );
    let failure = propagates(&mut caller, failed);
    caller.push(
        normal,
        Inst::Release {
            value: returned,
            layout: payload,
            live: MaskRef::All,
            anchor: CLEANUP,
            failure,
        },
    );
    caller.term(normal, Terminator::Return(Some(answer)));
    let caller = caller.finish_with_layouts(&table);
    let manifest = Manifest::skeleton([]);
    let violations = hirsel_lir::validate::check(
        &caller,
        &hirsel_lir::validate::Inputs {
            discharge: None,
            operations: &(),
            target: &target,
            layouts: &table,
            manifest: &manifest,
        },
    );
    assert!(violations.is_empty(), "{violations:?}");
    Program {
        target,
        layouts: table,
        units: vec![unit, caller],
        statics: vec![],
    }
}

#[test]
fn the_harness_holds_and_releases_a_direct_owned_block_return() {
    let p = returning_block(false, false);
    for full_recount_every_step in [false, true] {
        let run = hirsel_heapcheck::run(
            &p,
            &path("region_return"),
            &[],
            &mut NoOperations,
            &Config {
                full_recount_every_step,
                ..Config::default()
            },
        );
        let audit = run
            .result
            .as_ref()
            .unwrap_or_else(|v| panic!("{v:?}\n{run:?}"));
        assert_eq!(
            audit.observation.returned,
            Some(Observed::Block(Box::new(Observed::Aggregate(vec![Some(
                Observed::Integer {
                    scalar: ScalarKind::U64,
                    bits: 7
                }
            )]))))
        );
        assert_eq!(run.counters.allocations, 1);
        assert_eq!(run.counters.frees, 1);
        assert!(audit.exemptions.is_empty());
    }
}

#[test]
fn returning_a_block_does_not_hide_an_extra_leaked_credit() {
    let p = returning_block(true, false);
    for full_recount_every_step in [false, true] {
        let run = hirsel_heapcheck::run(
            &p,
            &path("region_return"),
            &[],
            &mut NoOperations,
            &Config {
                full_recount_every_step,
                ..Config::default()
            },
        );
        assert_eq!(
            run.violation(),
            Some(ViolationKind::LeakedCredit),
            "{run:?}"
        );
    }
}

#[test]
fn the_harness_holds_a_heap_return_after_the_regions_exit() {
    let p = returning_block(false, true);
    for full_recount_every_step in [false, true] {
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("region_return"),
            &[],
            &mut || Box::new(NoOperations),
            &Config {
                full_recount_every_step,
                compare_campaigns: true,
                ..Config::default()
            },
        );
        assert!(campaign.first_violation().is_none(), "{campaign:?}");
        assert_eq!(campaign.clean.sites, 1);
        assert_eq!(campaign.injected.len(), 1);
        assert_eq!(campaign.clean.counters.allocations, 1);
        assert_eq!(campaign.clean.counters.frees, 1);
        assert!(
            campaign
                .clean
                .trace
                .iter()
                .any(|e| matches!(e, hirsel_heapcheck::Event::RegionExit { .. }))
        );
    }
}

#[test]
fn an_ordinary_scalar_caller_still_releases_the_same_returned_block() {
    let p = returning_block(false, true);
    for full_recount_every_step in [false, true] {
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("entry"),
            &[],
            &mut || Box::new(NoOperations),
            &Config {
                full_recount_every_step,
                compare_campaigns: true,
                ..Config::default()
            },
        );
        assert!(campaign.first_violation().is_none(), "{campaign:?}");
        assert_eq!(campaign.clean.sites, 1);
        assert_eq!(campaign.injected.len(), 1);
        assert_eq!(campaign.clean.counters.allocations, 1);
        assert_eq!(campaign.clean.counters.frees, 1);
        assert_eq!(
            campaign.clean.result.unwrap().observation.returned,
            Some(Observed::Integer {
                scalar: ScalarKind::U64,
                bits: 7
            })
        );
    }
}

#[test]
fn the_terminal_drain_rejects_an_unreleased_block_with_a_clean_twin() {
    for broken in [false, true] {
        let mut p = returning_block(false, false);
        if broken {
            // Deliberately omit the returned wrapper's release program. Its
            // holder reaches the harness, but its block must not become exempt.
            let layout = p.layouts.fn_abis[0].result.as_ref().unwrap().layout;
            p.layouts.layouts[layout.0 as usize].drop.clear();
            let digest = p.layouts.digest();
            for unit in &mut p.units {
                unit.header.layouts = digest;
            }
        }
        for full_recount_every_step in [false, true] {
            let run = hirsel_heapcheck::run(
                &p,
                &path("region_return"),
                &[],
                &mut NoOperations,
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            if broken {
                assert_eq!(
                    run.violation(),
                    Some(ViolationKind::LeakedCredit),
                    "{run:?}"
                );
                assert!(
                    run.result
                        .unwrap_err()
                        .message
                        .contains("left a holder at offset 0")
                );
                assert_eq!(run.counters.frees, 0);
            } else {
                assert!(run.result.is_ok(), "{run:?}");
                assert_eq!(run.counters.frees, 1);
            }
        }
    }
}
