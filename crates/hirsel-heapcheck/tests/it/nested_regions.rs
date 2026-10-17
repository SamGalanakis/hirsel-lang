//! Source-free controls for headerless nested region payloads.
use hirsel_core_contract::Manifest;
use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
use hirsel_heapcheck::{Config, NoOperations, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Rc;
use hirsel_lir::testing::UnitBuilder;

fn fails(b: &mut UnitBuilder<Rc>, target: BlockId) -> FailureId {
    let edge = b.edge(target, &[]);
    b.failure(edge, None, &[])
}

fn nested(heap_child: bool, escape: bool) -> (Program<Rc>, ValueId) {
    fixture(heap_child, escape, false)
}

fn fixture(heap_child: bool, escape: bool, finalizer: bool) -> (Program<Rc>, ValueId) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let n = l.scalar(ScalarKind::U64);
    let leaf = if finalizer {
        l.resource(&[n], Some(path("finalize")))
    } else {
        l.record(&[n])
    };
    let leaf_ref = l.block(leaf);
    let tuple = l.record(&[leaf_ref, n]);
    let outer = l.record(&[tuple]);
    // Flatten this zero-offset inline field as the production layout does.
    let tuple_record = l.get(tuple).clone();
    l.get_mut(outer).scan = tuple_record.scan;
    l.get_mut(outer).drop = tuple_record.drop;
    let outer_ref = l.block(outer);
    if escape {
        l.table.fn_abis[0].result = Some(hirsel_lir::abi_types::AbiResult {
            borrowed_parameters: vec![],
            layout: tuple,
            slots: vec![hirsel_lir::abi::slot(&l.target, ScalarKind::Pointer)],
        });
    }
    let (target, layouts) = l.finish();
    let mut b = UnitBuilder::<Rc>::new(fixture_unit(path("main")));
    b.unit.tables.cleanup_anchors.clear();
    let body = b.block(BlockKind::Body, &[]);
    let cleanup = b.block(BlockKind::FailureCleanup, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(cleanup, Terminator::Propagate);
    b.term(failed, Terminator::Propagate);
    let r = b.region();
    let mark = b.owned(ValueRepr::RegionMark, n);
    b.push(
        body,
        Inst::RegionEnter {
            result: mark,
            region: r,
        },
    );
    let failure = fails(&mut b, failed);
    b.push(
        cleanup,
        Inst::RegionExit {
            mark,
            region: r,
            failure,
        },
    );
    let constant = b.constant(
        ValueRepr::Scalar(ScalarKind::U64),
        n,
        ConstantValue::Integer(7),
    );
    let seven = b.plain(ValueRepr::Scalar(ScalarKind::U64), n);
    b.push(
        body,
        Inst::Const {
            result: seven,
            constant,
        },
    );
    let local = Placement::Fixed(StorageOwner::LocalRegion(r));
    let child = if heap_child {
        b.owned(ValueRepr::Block, leaf_ref)
    } else {
        b.plain(ValueRepr::Uncounted, leaf_ref)
    };
    let construct = b.construct(
        ConstructorKind::Tuple,
        leaf,
        &[(0, seven)],
        if heap_child {
            Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))
        } else {
            local
        },
    );
    b.unit.tables.constructs[construct.index()].destination = DestinationClass::Inferred;
    b.unit.tables.constructs[construct.index()].arena_no_drop = !heap_child;
    let failure = fails(&mut b, cleanup);
    b.push(
        body,
        Inst::Construct {
            result: child,
            construct,
            failure,
        },
    );
    if !heap_child {
        b.push(body, Inst::RegionPublish { value: child });
    }
    let data = if heap_child {
        b.owned(ValueRepr::Aggregate, tuple)
    } else {
        b.plain(ValueRepr::Uncounted, tuple)
    };
    let construct = b.construct(
        ConstructorKind::Tuple,
        tuple,
        &[(0, child), (1, seven)],
        Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))),
    );
    let failure = fails(&mut b, cleanup);
    b.push(
        body,
        Inst::Construct {
            result: data,
            construct,
            failure,
        },
    );
    let holder = b.plain(ValueRepr::Uncounted, outer_ref);
    let construct = b.construct(ConstructorKind::Tuple, outer, &[(0, data)], local);
    b.unit.tables.constructs[construct.index()].arena_no_drop = true;
    b.unit.tables.constructs[construct.index()].destination = DestinationClass::Inferred;
    let failure = fails(&mut b, cleanup);
    b.push(
        body,
        Inst::Construct {
            result: holder,
            construct,
            failure,
        },
    );
    b.push(body, Inst::RegionPublish { value: holder });
    let failure = fails(&mut b, failed);
    b.push(
        body,
        Inst::RegionExit {
            mark,
            region: r,
            failure,
        },
    );
    b.term(body, Terminator::Return(escape.then_some(data)));
    let unit = b.finish_with_layouts(&layouts);
    (
        Program {
            target,
            layouts,
            units: vec![unit],
            statics: vec![],
        },
        data,
    )
}

fn check(p: &Program<Rc>) -> Vec<hirsel_lir::validate::ViolationKind> {
    hirsel_lir::validate::check(
        &p.units[0],
        &hirsel_lir::validate::Inputs {
            operations: &(),
            target: &p.target,
            layouts: &p.layouts,
            manifest: &Manifest::skeleton([]),
        },
    )
    .into_iter()
    .map(|v| v.kind)
    .collect()
}

#[test]
fn a_region_container_cannot_hide_a_heap_child() {
    let (good, _) = nested(false, false);
    assert!(check(&good).is_empty(), "{:?}", check(&good));
    let (bad, _) = nested(true, false);
    assert!(
        check(&bad).contains(&hirsel_lir::validate::ViolationKind::Type(
            "region construction field"
        )),
        "{:?}",
        check(&bad)
    );
    for audit in [false, true] {
        let config = Config {
            full_recount_every_step: audit,
            ..Config::default()
        };
        let good = hirsel_heapcheck::run(&good, &path("main"), &[], &mut NoOperations, &config);
        assert!(good.result.is_ok(), "{:?}", good.result);
        let bad = hirsel_heapcheck::run(&bad, &path("main"), &[], &mut NoOperations, &config);
        assert_eq!(
            bad.violation(),
            Some(ViolationKind::LeakedCredit),
            "{:?}",
            bad.result
        );
    }
}

#[test]
fn an_inline_nested_value_still_cannot_escape_its_region() {
    let (good, _) = nested(false, false);
    assert!(check(&good).is_empty(), "{:?}", check(&good));
    let (bad, value) = nested(false, true);
    assert!(
        check(&bad).contains(&hirsel_lir::validate::ViolationKind::RegionEscape(value)),
        "{:?}",
        check(&bad)
    );
}

#[test]
fn a_region_payload_cannot_elide_user_finalization() {
    let (good, _) = nested(false, false);
    assert!(check(&good).is_empty(), "{:?}", check(&good));
    let (bad, _) = fixture(false, false, true);
    assert!(
        check(&bad).contains(&hirsel_lir::validate::ViolationKind::Type(
            "region construction finalizer"
        )),
        "{:?}",
        check(&bad)
    );
}
