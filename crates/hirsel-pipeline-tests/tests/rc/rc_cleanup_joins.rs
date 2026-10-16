//! Hand-built joins keep resources and failure suffixes independent of source lowering.
use crate::rc_support as support;
use hirsel_core_contract::Passing;
use hirsel_heapcheck::{Config, NoOperations, build::Layouts, build::UnitBuilder};
use hirsel_lir::{
    abi_types::*,
    ir::*,
    layout_types::*,
    stage::{Rc, Ready, StageKind},
    validate::Inputs,
};
use hirsel_rc::cleanup::{Finalizer, Finalizers};
use std::collections::BTreeMap;
use support::*;

fn heap() -> Placement {
    Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))
}
fn constant(
    b: &mut UnitBuilder<Rc>,
    at: BlockId,
    layout: LayoutId,
    kind: ScalarKind,
    value: ConstantValue,
) -> ValueId {
    let result = b.plain(ValueRepr::Scalar(kind), layout);
    let constant = b.constant(ValueRepr::Scalar(kind), layout, value);
    b.push(at, Inst::Const { result, constant });
    result
}
fn jump(b: &mut UnitBuilder<Rc>, from: BlockId, to: BlockId, values: &[ValueId]) {
    let edge = b.edge(to, values);
    b.end(from, Terminator::Jump(edge));
}
fn fresh_failure(b: &mut UnitBuilder<Rc>, template: FailureId) -> FailureId {
    let f = b.unit.tables.failures[template.index()].clone();
    let e = b.unit.tables.edges[f.cleanup.index()].clone();
    let args = e
        .arguments
        .get(&b.unit.pools.values)
        .unwrap_or(&[])
        .to_vec();
    let cleanup = b.edge(e.target, &args);
    let failure = FailureId(b.unit.tables.failures.len() as u32);
    b.unit.tables.failures.push(FailureEdge { cleanup, ..f });
    failure
}
fn drop_slot(b: &mut UnitBuilder<Rc>, at: BlockId, address: AddressId, failure: FailureId) {
    let failure = fresh_failure(b, failure);
    b.push(
        at,
        Inst::Drop {
            address,
            if_live: true,
            anchor: hirsel_heapcheck::build::CLEANUP,
            failure,
        },
    );
}

struct Fixture {
    table: LayoutTable,
    units: Vec<LirUnit<Rc>>,
    finalizers: Finalizers,
}

/// Each arm transfers a different subset of the three resources. The loop
/// transfers field zero once, then breaks through a join with its first entry.
fn fixture(choice: u32, looped: bool, fail_arm: bool, fail_finalizer: bool) -> Fixture {
    let mut layouts = Layouts::new(target());
    let boolean = layouts.scalar(ScalarKind::Bool);
    let number = layouts.scalar(ScalarKind::U64);
    let finalizer_path = path("drop_fixture");
    let resource = layouts.resource(&[number], Some(finalizer_path.clone()));
    let record = layouts.record(&[resource, resource, resource]);
    // The scalar-only resources own finalizers, with no pointer holders to scan.
    layouts.get_mut(record).scan.clear();
    let (_, mut table) = layouts.finish();
    let mut abi = table.fn_abis[0].clone();
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: Passing::Borrow,
        passing: ParameterPassing::ReadOnlyAddress,
        layout: resource,
        slots: vec![PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: 8,
            align: 8,
        }],
        extension: ScalarExtension::None,
    }];
    table.fn_abis.push(abi);
    let mut f = UnitBuilder::<Rc>::new(finalizer_path.clone());
    f.unit.header.stage = StageKind::Rc;
    f.unit.header.abi = FnAbiId(1);
    let body = f.block(BlockKind::Body, &[]);
    f.unit.entry = body;
    let failure = f.block(BlockKind::FailureCleanup, &[]);
    f.end(failure, Terminator::Propagate);
    let self_value = f.plain(ValueRepr::Aggregate, resource);
    f.parameter(self_value);
    f.slot(ValueRepr::Aggregate, resource, SlotKind::Parameter(0));
    if fail_finalizer {
        let condition = constant(
            &mut f,
            body,
            boolean,
            ScalarKind::Bool,
            ConstantValue::Bool(false),
        );
        let edge = f.edge(failure, &[]);
        let failure = f.failure(edge, Some("SEM-1"), &[]);
        f.push(body, Inst::Check { condition, failure });
    }
    f.end(body, Terminator::Return(None));
    let finalizer = hirsel_rc::insert::Editor::new(f.finish(&table)).finish();
    let finalizers = Finalizers(BTreeMap::from([(
        resource,
        Finalizer {
            origin: UnitOrigin::Source,
            definition: finalizer_path,
            instance: finalizer.header.instance,
            abi: FnAbiId(1),
        },
    )]));

    let mut b = UnitBuilder::<Rc>::new(path("main"));
    b.unit.header.stage = StageKind::Rc;
    let entry = b.block(BlockKind::Body, &[]);
    b.unit.entry = entry;
    let propagated = b.block(BlockKind::FailureCleanup, &[]);
    b.end(propagated, Terminator::Propagate);
    let edge = b.edge(propagated, &[]);
    let empty_failure = b.failure(edge, None, &[]);
    let root = b.slot(ValueRepr::Aggregate, record, SlotKind::Local);
    let mut slots = vec![root];
    let mut addresses = vec![b.address(AddressBase::Slot(root), &[], ValueRepr::Aggregate, record)];
    for _ in 0..4 {
        let slot = b.slot(ValueRepr::Aggregate, resource, SlotKind::Temporary);
        slots.push(slot);
        addresses.push(b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, resource));
    }
    let fields: Vec<_> = (0..3)
        .map(|i| {
            b.address(
                AddressBase::Slot(root),
                &[ProjectionStep::Field(i)],
                ValueRepr::Aggregate,
                resource,
            )
        })
        .collect();
    // The one failure chain works both before construction and after a partial
    // transfer, with temporaries before the remaining named record.
    let mut tail = propagated;
    for (i, address) in addresses
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 3 && (*i != 4 || fail_arm))
    {
        let _ = i;
        let block = b.block(BlockKind::FailureCleanup, &[]);
        let e = b.edge(tail, &[]);
        let failure = b.failure(e, None, &[]);
        drop_slot(&mut b, block, *address, failure);
        jump(&mut b, block, tail, &[]);
        tail = block;
    }
    let edge = b.edge(tail, &[]);
    let cleanup = b.failure(edge, Some("SEM-1"), &[]);
    for (&slot, &address) in slots.iter().zip(&addresses) {
        b.push(entry, Inst::StorageLive { slot });
        let failure = fresh_failure(&mut b, empty_failure);
        b.push(entry, Inst::Address { address, failure });
    }
    for &address in &fields {
        let failure = fresh_failure(&mut b, empty_failure);
        b.push(entry, Inst::Address { address, failure });
    }
    let symbol = b.symbol(SymbolKind::Type, path("record"));
    let mut versions = Vec::new();
    for i in 0..3 {
        let scalar = constant(
            &mut b,
            entry,
            number,
            ScalarKind::U64,
            ConstantValue::Integer(i as u128),
        );
        let value = b.owned(ValueRepr::Aggregate, resource);
        let construct = b.construct(
            ConstructorKind::Record(symbol),
            resource,
            &[(0, scalar)],
            heap(),
        );
        let failure = fresh_failure(&mut b, cleanup);
        b.push(
            entry,
            Inst::Construct {
                result: value,
                construct,
                failure,
            },
        );
        let version = b.plain(ValueRepr::Aggregate, resource);
        b.push(
            entry,
            Inst::Store {
                address: addresses[i + 1],
                value,
                version,
            },
        );
        versions.push(version);
    }
    let mut values = Vec::new();
    for i in 0..3 {
        let value = b.owned(ValueRepr::Aggregate, resource);
        b.push(
            entry,
            Inst::Read {
                result: value,
                address: addresses[i + 1],
                reaching: versions[i],
                mode: ReadMode::Take,
            },
        );
        values.push((i as u32, value));
    }
    let value = b.owned(ValueRepr::Aggregate, record);
    let construct = b.construct(ConstructorKind::Record(symbol), record, &values, heap());
    let failure = fresh_failure(&mut b, cleanup);
    b.push(
        entry,
        Inst::Construct {
            result: value,
            construct,
            failure,
        },
    );
    let version = b.plain(ValueRepr::Aggregate, record);
    b.push(
        entry,
        Inst::Store {
            address: addresses[0],
            value,
            version,
        },
    );
    let after = b.block(BlockKind::Body, &[]);
    let transfer = |b: &mut UnitBuilder<Rc>, at, field| {
        let value = b.owned(ValueRepr::Aggregate, resource);
        b.push(
            at,
            Inst::Read {
                result: value,
                address: fields[field],
                reaching: version,
                mode: ReadMode::Take,
            },
        );
        b.push(at, Inst::StorageLive { slot: slots[4] });
        let version = b.plain(ValueRepr::Aggregate, resource);
        b.push(
            at,
            Inst::Store {
                address: addresses[4],
                value,
                version,
            },
        );
        if fail_arm {
            let condition = constant(b, at, boolean, ScalarKind::Bool, ConstantValue::Bool(false));
            let failure = fresh_failure(b, cleanup);
            b.push(at, Inst::Check { condition, failure });
        }
        drop_slot(b, at, addresses[4], cleanup);
    };
    if looped {
        let flag = b.plain(ValueRepr::Scalar(ScalarKind::Bool), boolean);
        let header = b.block(BlockKind::Body, &[flag]);
        let body = b.block(BlockKind::Body, &[]);
        let intact = b.block(BlockKind::Body, &[]);
        let first = constant(
            &mut b,
            entry,
            boolean,
            ScalarKind::Bool,
            ConstantValue::Bool(choice == 0),
        );
        jump(&mut b, entry, header, &[first]);
        let yes = b.edge(body, &[]);
        let no = b.edge(intact, &[]);
        b.end(
            header,
            Terminator::Branch {
                condition: flag,
                yes,
                no,
            },
        );
        transfer(&mut b, body, 0);
        jump(&mut b, body, after, &[]);
        let keep = constant(
            &mut b,
            intact,
            boolean,
            ScalarKind::Bool,
            ConstantValue::Bool(choice == 1),
        );
        let feedback = b.block(BlockKind::Body, &[]);
        let yes = b.edge(after, &[]);
        let no = b.edge(feedback, &[]);
        b.end(
            intact,
            Terminator::Branch {
                condition: keep,
                yes,
                no,
            },
        );
        let next = constant(
            &mut b,
            feedback,
            boolean,
            ScalarKind::Bool,
            ConstantValue::Bool(true),
        );
        jump(&mut b, feedback, header, &[next]);
    } else {
        let arms: Vec<_> = (0..3).map(|_| b.block(BlockKind::Body, &[])).collect();
        let selected = constant(
            &mut b,
            entry,
            number,
            ScalarKind::U64,
            ConstantValue::Integer(choice.into()),
        );
        let edges: Vec<_> = arms.iter().map(|&a| b.edge(a, &[])).collect();
        let switches = PoolRange::push(
            &mut b.unit.pools.switch_arms,
            [
                SwitchArm {
                    value: 0,
                    edge: edges[0],
                },
                SwitchArm {
                    value: 1,
                    edge: edges[1],
                },
            ],
        );
        b.end(
            entry,
            Terminator::Switch {
                value: selected,
                arms: switches,
                otherwise: edges[2],
            },
        );
        for (arm, moved) in arms.into_iter().zip([vec![0], vec![1, 2], vec![0, 2]]) {
            for field in moved {
                transfer(&mut b, arm, field);
            }
            jump(&mut b, arm, after, &[]);
        }
    }
    drop_slot(&mut b, after, addresses[0], cleanup);
    b.end(after, Terminator::Return(None));
    Fixture {
        table: table.clone(),
        units: vec![
            hirsel_rc::insert::Editor::new(b.finish(&table)).finish(),
            finalizer,
        ],
        finalizers,
    }
}

fn prove(fixture: Fixture) {
    let target = target();
    let manifest = hirsel_core_contract::Manifest::skeleton([]);
    let inputs = Inputs {
        operations: &Default::default(),
        target: &target,
        layouts: &fixture.table,
        manifest: &manifest,
    };
    for unit in &fixture.units {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(violations.is_empty(), "Rc {violations:?}\n{}", dump(unit));
    }
    let units: Vec<LirUnit<Ready>> = fixture
        .units
        .into_iter()
        .map(|u| hirsel_rc::cleanup::lower(u, &fixture.table, &fixture.finalizers).unwrap())
        .collect();
    for unit in &units {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(
            violations.is_empty(),
            "Ready {violations:?}\n{}",
            dump(unit)
        );
    }
    assert!(units[0].tables.init_flags.iter().any(|f| f.path.len > 0));
    let program = hirsel_heapcheck::Program {
        target,
        layouts: fixture.table,
        units,
        statics: vec![],
    };
    for full_recount_every_step in [false, true] {
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config {
                full_recount_every_step,
                ..Config::default()
            },
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        assert_eq!(campaign.clean.counters.finalizers, 3);
    }
}

#[test]
fn three_predecessors_transfer_three_different_sets() {
    for choice in 0..3 {
        prove(fixture(choice, false, false, false));
    }
}
#[test]
fn a_part_transferred_in_a_loop_is_absent_on_break() {
    for choice in 0..3 {
        prove(fixture(choice, true, false, false));
    }
}
#[test]
fn a_failure_cleans_a_partially_transferred_region() {
    for choice in 0..3 {
        prove(fixture(choice, false, true, false));
    }
}
#[test]
fn finalizer_parts_owned_on_one_edge_run_once_even_when_they_fail() {
    for choice in 0..3 {
        prove(fixture(choice, false, false, true));
    }
}

#[test]
fn a_forged_field_flag_is_rejected_independently_by_c_and_h() {
    prove(fixture(0, false, false, false));
    let fixture = fixture(0, false, false, false);
    let mut units: Vec<LirUnit<Ready>> = fixture
        .units
        .into_iter()
        .map(|u| hirsel_rc::cleanup::lower(u, &fixture.table, &fixture.finalizers).unwrap())
        .collect();
    for constant in &mut units[0].tables.constants {
        if let ConstantValue::Bool(value) = &mut constant.value {
            *value = !*value;
        }
    }
    let target = target();
    let manifest = hirsel_core_contract::Manifest::skeleton([]);
    let violations = hirsel_lir::validate::check(
        &units[0],
        &Inputs {
            operations: &Default::default(),
            target: &target,
            layouts: &fixture.table,
            manifest: &manifest,
        },
    );
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_lir::validate::ViolationKind::InitFlag { .. })),
        "{violations:?}"
    );
    let program = hirsel_heapcheck::Program {
        target,
        layouts: fixture.table,
        units,
        statics: vec![],
    };
    for full_recount_every_step in [false, true] {
        let run = hirsel_heapcheck::run(
            &program,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config {
                full_recount_every_step,
                ..Config::default()
            },
        );
        let violation = run.result.unwrap_err();
        assert_eq!(violation.kind, hirsel_heapcheck::ViolationKind::Malformed);
        assert!(
            violation.message.contains("field drop flag"),
            "{violation:?}"
        );
    }
}

#[test]
fn three_way_inline_indices_use_independent_field_flags() {
    for choice in 0..3 {
        let mut fixture = fixture(choice, false, false, false);
        let unit = &mut fixture.units[0];
        let record = unit.types[unit.slots[0].ty.index()].layout;
        let resource = fixture.table.layouts[record.0 as usize].fields[0].layout;
        let SizeSpec::Fixed(stride) = fixture.table.layouts[resource.0 as usize].size else {
            panic!("inline resource must be fixed");
        };
        let shape = &mut fixture.table.layouts[record.0 as usize];
        shape.kind = LayoutKind::Inline {
            element: resource,
            count: 3,
        };
        shape.fields.clear();
        shape.drop = vec![DropStep::RepeatReverse {
            offset: 0,
            count: 3,
            stride,
            element: resource,
        }];
        let ty = unit
            .types
            .iter()
            .position(|t| t.repr == ValueRepr::Scalar(ScalarKind::U64))
            .unwrap();
        let indices: Vec<_> = (0..3)
            .map(|index| {
                let result = ValueId(unit.values.len() as u32);
                unit.values.push(ValueInfo {
                    ty: LirTypeId(ty as u32),
                    holding: Holding::Plain,
                });
                let constant = ConstantId(unit.tables.constants.len() as u32);
                unit.tables.constants.push(ConstantData {
                    ty: LirTypeId(ty as u32),
                    value: ConstantValue::Integer(index),
                });
                (result, constant)
            })
            .collect();
        let at = unit.blocks[unit.entry.index()].instructions.start;
        unit.instructions.splice(
            at as usize..at as usize,
            indices
                .iter()
                .map(|&(result, constant)| Inst::Const { result, constant }),
        );
        for (n, block) in unit.blocks.iter_mut().enumerate() {
            if n == unit.entry.index() {
                block.instructions.len += 3;
            } else if block.instructions.start >= at {
                block.instructions.start += 3;
            }
        }
        for address in &mut unit.addresses {
            if address.base == AddressBase::Slot(SlotId(0)) && address.projection.len == 1 {
                let step = &mut unit.pools.projections[address.projection.start as usize];
                if let ProjectionStep::Field(index) = *step {
                    *step = ProjectionStep::Index(indices[index as usize].0);
                }
            }
        }
        for construct in &mut unit.tables.constructs {
            if construct.layout == record {
                construct.constructor = ConstructorKind::Sequence;
            }
        }
        let digest = fixture.table.digest();
        for unit in &mut fixture.units {
            unit.header.layouts = digest;
        }
        prove(fixture);
    }
}
