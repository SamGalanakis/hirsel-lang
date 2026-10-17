//! Active variant cleanup keeps inline finalizers and failure paths explicit.
use crate::rc_support as support;
use hirsel_heapcheck::{Config, NoOperations};
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use support::*;
fn heap_placement() -> hirsel_lir::ir::Placement {
    hirsel_lir::ir::Placement::Fixed(hirsel_lir::ir::StorageOwner::Heap(
        hirsel_lir::ir::TaskDomainId(0),
    ))
}
fn layouts_bool(
    table: &hirsel_lir::layout_types::LayoutTable,
) -> hirsel_lir::layout_types::LayoutId {
    hirsel_lir::layout_types::LayoutId(
        table
            .layouts
            .iter()
            .position(|r| {
                r.kind
                    == hirsel_lir::layout_types::LayoutKind::Scalar(
                        hirsel_lir::layout_types::ScalarKind::Bool,
                    )
            })
            .unwrap() as u32,
    )
}
fn option_finalizer(variant: u8, failing: bool) {
    use hirsel_core_contract::Passing;
    use hirsel_heapcheck::build::{ANCHOR, CLEANUP, Layouts, fixture_unit};
    use hirsel_lir::abi_types::*;
    use hirsel_lir::ir::*;
    use hirsel_lir::layout_types::*;
    use hirsel_lir::stage::{Rc, StageKind};
    use hirsel_lir::testing::UnitBuilder;
    use hirsel_rc::cleanup::Finalizer;
    use std::collections::BTreeMap;
    let mut layouts = Layouts::new(target());
    layouts.scalar(ScalarKind::Bool);
    let scalar = layouts.scalar(ScalarKind::U64);
    let empty = layouts.record(&[]);
    let block_layout = layouts.block(empty);
    let finalizer_path = path("drop_fixture");
    let resource = layouts.resource(&[block_layout, scalar], Some(finalizer_path.clone()));
    let option = if variant >= 3 {
        layouts.niche(0, &[block_layout, resource])
    } else if variant == 0 {
        layouts.variant(&[&[], &[block_layout]])
    } else {
        layouts.variant(&[&[], &[resource]])
    };
    if variant != 0 {
        let data = layouts.get(option).tag.as_ref().unwrap().variants[1]
            .fields
            .clone();
        let scan = data
            .iter()
            .map(|f| ScanStep::Counted { offset: f.offset })
            .collect();
        let tag = layouts.get(option).tag.as_ref().unwrap().clone();
        layouts.get_mut(option).scan = vec![ScanStep::ActiveVariant {
            tag_offset: tag.offset,
            form: tag.form,
            variants: vec![(1, scan)],
        }];
    }
    let (_, mut table) = layouts.finish();
    let mut abi = table.fn_abis[0].clone();
    let passing = Passing::Borrow;
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: passing,
        passing: if passing == Passing::Mut {
            ParameterPassing::ExclusiveAddress
        } else {
            ParameterPassing::ReadOnlyAddress
        },
        layout: resource,
        slots: vec![PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: 8,
            align: 8,
        }],
        extension: ScalarExtension::None,
    }];
    table.fn_abis.push(abi);
    let mut f = UnitBuilder::<Rc>::new(fixture_unit(finalizer_path.clone()));
    f.unit.header.stage = StageKind::Rc;
    f.unit.header.abi = FnAbiId(1);
    f.unit.header.origin = UnitOrigin::Source;
    let body = f.block(BlockKind::Body, &[]);
    f.unit.entry = body;
    let failure_body = f.block(BlockKind::FailureCleanup, &[]);
    f.term(failure_body, Terminator::Propagate);
    let e = f.edge(failure_body, &[]);
    let failure = f.failure(e, None, &[]);
    let self_value = f.plain(ValueRepr::Aggregate, resource);
    f.parameter(self_value);
    let self_slot = f.slot(ValueRepr::Aggregate, resource, SlotKind::Parameter(0));
    let whole = f.address(
        AddressBase::Slot(self_slot),
        &[],
        ValueRepr::Aggregate,
        resource,
    );
    f.push(
        body,
        Inst::Address {
            address: whole,
            failure,
        },
    );
    if failing {
        let condition = f.plain(ValueRepr::Scalar(ScalarKind::Bool), layouts_bool(&table));
        let constant = f.constant(
            ValueRepr::Scalar(ScalarKind::Bool),
            layouts_bool(&table),
            ConstantValue::Bool(false),
        );
        f.push(
            body,
            Inst::Const {
                result: condition,
                constant,
            },
        );
        let edge = f.edge(failure_body, &[]);
        let failure = f.failure(edge, Some("SEM-1"), &[]);
        f.push(body, Inst::Check { condition, failure });
    }
    f.term(body, Terminator::Return(None));
    let finalizer = f.finish_with_layouts(&table);
    let mut b = UnitBuilder::<Rc>::new(fixture_unit(path("main")));
    b.unit.header.stage = StageKind::Rc;
    let prologue = b.block(BlockKind::Body, &[]);
    let entry = b.block(BlockKind::Body, &[]);
    b.unit.entry = prologue;
    let propagated = b.block(BlockKind::FailureCleanup, &[]);
    b.term(propagated, Terminator::Propagate);
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    let slot = b.slot(ValueRepr::Aggregate, option, SlotKind::Local);
    b.push(prologue, Inst::StorageLive { slot });
    let address = b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, option);
    b.push(prologue, Inst::Address { address, failure });
    let e = b.edge(entry, &[]);
    b.term(prologue, Terminator::Jump(e));
    let record_symbol = b.symbol(SymbolKind::Type, path("record"));
    let value = if variant == 2 || variant == 4 {
        let value = b.owned(ValueRepr::Aggregate, option);
        let construct = b.construct(
            ConstructorKind::Variant {
                definition: record_symbol,
                variant: 0,
            },
            option,
            &[],
            heap_placement(),
        );
        let edge = b.edge(propagated, &[]);
        let failure = b.failure(edge, None, &[]);
        b.push(
            entry,
            Inst::Construct {
                result: value,
                construct,
                failure,
            },
        );
        value
    } else {
        let storage = b.owned(ValueRepr::Block, block_layout);
        let heap = Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)));
        let construct = b.construct(
            ConstructorKind::Record(record_symbol),
            empty,
            &[],
            heap.clone(),
        );
        let e = b.edge(propagated, &[]);
        let failure = b.failure(e, None, &[]);
        b.push(
            entry,
            Inst::Construct {
                result: storage,
                construct,
                failure,
            },
        );
        let zero = b.plain(ValueRepr::Scalar(ScalarKind::U64), scalar);
        let constant = b.constant(
            ValueRepr::Scalar(ScalarKind::U64),
            scalar,
            ConstantValue::Integer(0),
        );
        b.push(
            entry,
            Inst::Const {
                result: zero,
                constant,
            },
        );
        let value = b.owned(ValueRepr::Aggregate, resource);
        let construct = b.construct(
            ConstructorKind::Record(record_symbol),
            resource,
            &[(0, storage), (1, zero)],
            heap,
        );
        let e = b.edge(propagated, &[]);
        let failure = b.failure(e, None, &[]);
        b.push(
            entry,
            Inst::Construct {
                result: value,
                construct,
                failure,
            },
        );
        let fields = if variant == 0 {
            // The resource-free twin carries the initial block, rather than a resource.
            // Drop the separately constructed resource before forming its option.
            let temp = b.slot(ValueRepr::Aggregate, resource, SlotKind::Temporary);
            let at = b.address(AddressBase::Slot(temp), &[], ValueRepr::Aggregate, resource);
            let edge = b.edge(propagated, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                prologue,
                Inst::Address {
                    address: at,
                    failure,
                },
            );
            b.push(entry, Inst::StorageLive { slot: temp });
            let version = b.plain(ValueRepr::Aggregate, resource);
            b.push(
                entry,
                Inst::Store {
                    address: at,
                    value,
                    version,
                },
            );
            let edge = b.edge(propagated, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                entry,
                Inst::Drop {
                    address: at,
                    if_live: false,
                    anchor: CLEANUP,
                    failure,
                },
            );
            b.push(
                entry,
                Inst::StorageDead {
                    slot: temp,
                    if_live: false,
                },
            );
            let value = b.owned(ValueRepr::Block, block_layout);
            let construct = b.construct(
                ConstructorKind::Record(record_symbol),
                empty,
                &[],
                heap_placement(),
            );
            let edge = b.edge(propagated, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                entry,
                Inst::Construct {
                    result: value,
                    construct,
                    failure,
                },
            );
            vec![(0, value)]
        } else if variant >= 3 {
            let temp = b.slot(ValueRepr::Aggregate, resource, SlotKind::Temporary);
            let at = b.address(AddressBase::Slot(temp), &[], ValueRepr::Aggregate, resource);
            let edge = b.edge(propagated, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                prologue,
                Inst::Address {
                    address: at,
                    failure,
                },
            );
            b.push(entry, Inst::StorageLive { slot: temp });
            let version = b.plain(ValueRepr::Aggregate, resource);
            b.push(
                entry,
                Inst::Store {
                    address: at,
                    value,
                    version,
                },
            );
            let cleanup = b.block(BlockKind::FailureCleanup, &[]);
            let edge = b.edge(propagated, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                cleanup,
                Inst::Drop {
                    address: at,
                    if_live: false,
                    anchor: CLEANUP,
                    failure,
                },
            );
            let edge = b.edge(propagated, &[]);
            b.term(cleanup, Terminator::Jump(edge));
            let block = b.owned(ValueRepr::Block, block_layout);
            let construct = b.construct(
                ConstructorKind::Record(record_symbol),
                empty,
                &[],
                heap_placement(),
            );
            let edge = b.edge(cleanup, &[]);
            let failure = b.failure(edge, None, &[]);
            b.push(
                entry,
                Inst::Construct {
                    result: block,
                    construct,
                    failure,
                },
            );
            let value = b.owned(ValueRepr::Aggregate, resource);
            b.push(
                entry,
                Inst::Read {
                    result: value,
                    address: at,
                    reaching: version,
                    mode: ReadMode::Take,
                },
            );
            b.push(
                entry,
                Inst::StorageDead {
                    slot: temp,
                    if_live: false,
                },
            );
            vec![(0, block), (1, value)]
        } else {
            vec![(0, value)]
        };
        let result = b.owned(ValueRepr::Aggregate, option);
        let construct = b.construct(
            ConstructorKind::Variant {
                definition: record_symbol,
                variant: 1,
            },
            option,
            &fields,
            heap_placement(),
        );
        let edge = b.edge(propagated, &[]);
        let failure = b.failure(edge, None, &[]);
        b.push(
            entry,
            Inst::Construct {
                result,
                construct,
                failure,
            },
        );
        result
    };
    let version = b.plain(ValueRepr::Aggregate, option);
    b.push(
        entry,
        Inst::Store {
            address,
            value,
            version,
        },
    );
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    b.push(
        entry,
        Inst::Drop {
            address,
            if_live: false,
            anchor: CLEANUP,
            failure,
        },
    );
    b.term(entry, Terminator::Return(None));
    let caller = b.finish_with_layouts(&table);
    let finalizers = Finalizers(BTreeMap::from([(
        resource,
        Finalizer {
            origin: finalizer.header.origin,
            definition: finalizer_path,
            instance: finalizer.header.instance,
            abi: FnAbiId(1),
        },
    )]));
    let result = hirsel_rc::cleanup::lower(caller, &table, &finalizers);
    let caller = result.unwrap();
    let finalizer = hirsel_rc::cleanup::lower(finalizer, &table, &Finalizers::default()).unwrap();
    let manifest = hirsel_core_contract::Manifest::skeleton([]);
    let target = target();
    let inputs = Inputs {
        operations: &(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    for unit in [&caller, &finalizer] {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(violations.is_empty(), "{}\n{violations:#?}", dump(unit));
    }
    if variant == 0 {
        assert!(
            !caller
                .terminators
                .iter()
                .any(|t| matches!(t, Terminator::Switch { .. }))
        );
        assert!(
            caller
                .instructions
                .iter()
                .any(|inst| matches!(inst, Inst::Release { layout, .. } if *layout == option))
        );
    }
    let call = caller
        .tables
        .calls
        .iter()
        .find(|c| matches!(c.callee, Callee::Direct(_)))
        .unwrap();
    let args = call.arguments.get(&caller.pools.call_arguments).unwrap();
    assert_eq!(args[0].passing, passing);
    assert!(matches!(args[0].value, ArgumentValue::Value(_)));
    let program = hirsel_heapcheck::Program {
        target,
        layouts: table,
        units: vec![caller, finalizer],
        statics: vec![],
    };
    let campaign = hirsel_heapcheck::fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    assert!(
        campaign.first_violation().is_none(),
        "{:?}",
        campaign.first_violation()
    );
    let run = campaign.clean;
    run.result.as_ref().unwrap_or_else(|v| panic!("{v}"));
    assert_eq!(
        run.counters.finalizers,
        u64::from(variant != 2 && variant != 4)
    );
    assert_eq!(
        !run.result.unwrap().observation.failures.is_empty(),
        failing
    );
    let _ = ANCHOR;
}

#[test]
fn active_variant_cleanup_finalizes_some_and_leaves_none_empty() {
    for variant in [0, 1, 2, 3, 4] {
        option_finalizer(variant, false);
    }
    for variant in [1, 3] {
        option_finalizer(variant, true);
    }
}
