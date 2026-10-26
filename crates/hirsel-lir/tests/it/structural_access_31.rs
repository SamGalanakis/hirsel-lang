fn physical_plan(
    graph: hirsel_lir::structural::ShapeGraph,
    layout: hirsel_lir::LayoutId,
) -> hirsel_lir::structural_access::AccessPlan {
    hirsel_lir::structural_access::AccessPlan::derive(
        hirsel_lir::operation::NativeType::Tuple(vec![]),
        graph,
        &crate::support::table(),
        layout,
    )
    .unwrap()
}

#[test]
fn static_depth_bounds_cover_all_paths_without_counting_width_or_transparency() {
    use hirsel_lir::structural::{Shape as S, ShapeGraph};
    let graph = ShapeGraph {
        root: 0,
        nodes: vec![
            S::Variant(vec![vec![], vec![1, 1, 2]]),
            S::Sequence(3),
            S::Map { key: 3, value: 4 },
            S::Leaf,
            S::Transparent(1),
        ],
    };
    assert_eq!(graph.static_depth_bound(), Some(3));
    for n in [1087, 1088, 1089, 4096] {
        let mut nodes: Vec<_> = (0..n).map(|i| S::Distinct(i + 1)).collect();
        nodes.extend([S::Transparent(n + 1), S::Fields(vec![])]);
        assert_eq!(
            ShapeGraph { root: 0, nodes }.static_depth_bound(),
            Some(u64::from(n))
        );
    }
    for nodes in [
        vec![S::Box(0)],
        vec![S::Sequence(1), S::Variant(vec![vec![], vec![0]])],
        vec![S::Transparent(0)],
        vec![S::Box(9)],
    ] {
        assert_eq!(ShapeGraph { root: 0, nodes }.static_depth_bound(), None);
    }
    // Unreachable recursion cannot deepen a value of the root type.
    assert_eq!(
        ShapeGraph {
            root: 0,
            nodes: vec![S::Leaf, S::Box(1)]
        }
        .static_depth_bound(),
        Some(0)
    );
}

#[test]
fn structural_access_rejects_erased_distinct_edge_and_forged_logical_binding() {
    use hirsel_lir::structural::{Shape, ShapeGraph};
    let table = crate::support::table();
    let plan = physical_plan(
        ShapeGraph {
            root: 0,
            nodes: vec![Shape::Distinct(1), Shape::Fields(vec![2, 2]), Shape::Leaf],
        },
        crate::support::l::PAY,
    );
    assert!(plan.check(&table).is_ok());
    let mut erased = plan.clone();
    erased.edges[plan.root.index()] = 0;
    assert!(erased.check(&table).is_err());
    let mut forged = plan;
    forged.logical_nodes[forged.root.index()] = 1;
    assert!(forged.check(&table).is_err());
}

#[test]
fn structural_access_shared_boundary_preserves_inner_distinct_edge() {
    use hirsel_lir::codec::CodecNode;
    use hirsel_lir::structural::{Shape, ShapeGraph};
    let table = crate::support::table();
    let plan = physical_plan(
        ShapeGraph {
            root: 0,
            nodes: vec![
                Shape::Transparent(1),
                Shape::Box(2),
                Shape::Distinct(3),
                Shape::Fields(vec![4, 4]),
                Shape::Leaf,
            ],
        },
        crate::support::l::SHARED,
    );
    assert!(plan.check(&table).is_ok());
    assert!(matches!(
        plan.nodes[plan.root.index()],
        CodecNode::Box { .. }
    ));
    assert_eq!(plan.edges[plan.root.index()], 0);
    let distinct = plan.logical_nodes.iter().position(|&s| s == 2).unwrap();
    assert_eq!(plan.edges[distinct], 1);
    assert!(plan.program(&table, &|id| Some(id.0)).is_ok());
}

#[test]
fn structural_access_flat_framing_does_not_erase_distinct_payload() {
    use hirsel_lir::structural::{Shape, ShapeGraph};
    let table = crate::support::table();
    let plan = physical_plan(
        ShapeGraph {
            root: 0,
            nodes: vec![Shape::Distinct(1), Shape::Fields(vec![2, 2]), Shape::Leaf],
        },
        crate::support::l::FLAT_ITEM_BLK,
    );
    assert!(plan.check(&table).is_ok());
    assert_eq!(plan.edges[plan.root.index()], 0);
    let distinct = plan
        .logical_nodes
        .iter()
        .enumerate()
        .find(|(i, s)| **s == 0 && plan.edges[*i] == 1)
        .unwrap()
        .0;
    let mut erased = plan.clone();
    erased.edges[distinct] = 0;
    assert!(erased.check(&table).is_err());
    assert!(plan.program(&table, &|id| Some(id.0)).is_ok());
}

#[test]
fn structural_access_independent_binding_rejects_type_laundering() {
    use hirsel_layout::{
        layout::{LayoutBuilder, Owners},
        structural_access,
    };
    use hirsel_lir::operation::{NativeType, Primitive as NativePrimitive};
    use hirsel_types::*;
    let types = crate::support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let root = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let owners = Owners::default();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, &owners);
    builder.layout(&types, root).unwrap();
    let table = builder.finish();
    let plan = structural_access::derive(&types, root, &mut |_| Ok(None), &owners, &target, &table)
        .unwrap();
    structural_access::validate(
        &plan,
        &types,
        root,
        &mut |_| Ok(None),
        &owners,
        &target,
        &table,
    )
    .unwrap();
    let encoded = hirsel_codec::to_text(&plan);
    assert_eq!(
        hirsel_codec::from_text::<hirsel_lir::structural_access::AccessPlan>(&encoded).unwrap(),
        plan
    );
    let mut forged = plan;
    forged.source = NativeType::Primitive(NativePrimitive::U64);
    // Physical self-consistency is insufficient to establish the source type.
    assert!(forged.check(&table).is_ok());
    assert!(
        structural_access::validate(
            &forged,
            &types,
            root,
            &mut |_| Ok(None),
            &owners,
            &target,
            &table
        )
        .is_err()
    );
}

#[test]
fn structural_access_empty_distinct_payload_still_has_an_edge() {
    use hirsel_lir::layout_types::*;
    use hirsel_lir::structural::{Shape, ShapeGraph};
    let mut table = crate::support::table();
    let mut wrapper = table.layouts[crate::support::l::UNIT.0 as usize].clone();
    wrapper.kind = LayoutKind::Aggregate;
    wrapper.fields.push(FieldLayout {
        ordinal: 0,
        offset: 0,
        layout: crate::support::l::UNIT,
        unaligned: false,
    });
    let root = LayoutId(table.layouts.len() as u32);
    table.layouts.push(wrapper);
    let plan = hirsel_lir::structural_access::AccessPlan::derive(
        hirsel_lir::operation::NativeType::Tuple(vec![]),
        ShapeGraph {
            root: 0,
            nodes: vec![Shape::Distinct(1), Shape::Fields(vec![])],
        },
        &table,
        root,
    )
    .unwrap();
    assert!(plan.check(&table).is_ok());
    assert_eq!(plan.edges[plan.root.index()], 1);
    let mut erased = plan;
    erased.edges[erased.root.index()] = 0;
    assert!(erased.check(&table).is_err());
}

#[test]
fn structural_access_soa_keeps_the_distinct_row_before_projecting_columns() {
    use hirsel_lir::{
        layout_types::*,
        structural::{Shape, ShapeGraph},
    };
    let mut table = crate::support::table();
    let mut columns = table.layouts[crate::support::l::SEQ.0 as usize].clone();
    columns.kind = LayoutKind::SoaColumns {
        element: crate::support::l::PAY,
    };
    let payload = LayoutId(table.layouts.len() as u32);
    table.layouts.push(columns);
    let mut block = table.layouts[crate::support::l::SEQBLK.0 as usize].clone();
    block.kind = LayoutKind::Block { payload };
    let root = LayoutId(table.layouts.len() as u32);
    table.layouts.push(block);
    let plan = hirsel_lir::structural_access::AccessPlan::derive(
        hirsel_lir::operation::NativeType::Tuple(vec![]),
        ShapeGraph {
            root: 0,
            nodes: vec![
                Shape::Sequence(1),
                Shape::Distinct(2),
                Shape::Fields(vec![3, 3]),
                Shape::Leaf,
            ],
        },
        &table,
        root,
    )
    .unwrap();
    assert!(plan.check(&table).is_ok());
    let row = plan.logical_nodes.iter().position(|&s| s == 1).unwrap();
    assert_eq!(plan.edges[row], 1);
    assert!(plan.program(&table, &|id| Some(id.0)).is_ok());
}

#[test]
fn access_only_program_needs_no_output_or_u64_control_type() {
    use hirsel_layout::{
        layout::{LayoutBuilder, Owners},
        structural_access,
    };
    use hirsel_types::*;
    let types = crate::support::session();
    let root = types.intern(Type::unit());
    let owners = Owners::default();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, &owners);
    builder.layout(&types, root).unwrap();
    let table = builder.finish();
    assert!(!table.layouts.iter().any(|r| matches!(
        r.kind,
        hirsel_lir::layout_types::LayoutKind::Scalar(hirsel_lir::layout_types::ScalarKind::U64)
    )));
    let plan = structural_access::derive(&types, root, &mut |_| Ok(None), &owners, &target, &table)
        .unwrap();
    let program = plan.program(&table, &|_| Some(0)).unwrap();
    assert_eq!(program[1], plan.nodes.len() as u64);
}

fn keyed_call_prefix() -> hirsel_lir::ir::LirUnit<hirsel_lir::stage::Value> {
    use crate::support::{ENTRY, Lir, l};
    use hirsel_lir::{ir::*, stage::Value};
    let mut fixture = Lir::<Value>::new(&[], None);
    let failure = fixture.fails();
    let key = fixture.builder.plain(
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        l::U64,
    );
    let taken = fixture.builder.plain(
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        l::U64,
    );
    let hash = fixture.builder.plain(
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        l::U64,
    );
    fixture.builder.unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty: fixture.builder.unit.values[key.index()].ty,
    });
    fixture.builder.push(ENTRY, Inst::FrameEnter { failure });
    fixture.builder.push(
        ENTRY,
        Inst::Read {
            result: key,
            address: AddressId(0),
            reaching: key,
            mode: ReadMode::Borrow,
        },
    );
    fixture.builder.push(
        ENTRY,
        Inst::Hash {
            result: hash,
            value: key,
            layout: l::U64,
            failure,
        },
    );
    fixture.builder.push(
        ENTRY,
        Inst::Read {
            result: taken,
            address: AddressId(0),
            reaching: key,
            mode: ReadMode::Take,
        },
    );
    let arguments = PoolRange::push(
        &mut fixture.builder.unit.pools.call_arguments,
        [CallArgument {
            parameter: 1,
            passing: hirsel_core_contract::Passing::Take,
            value: ArgumentValue::Value(taken),
        }],
    );
    let operation = OperationRef(fixture.builder.unit.tables.operations.len() as u32);
    fixture
        .builder
        .unit
        .tables
        .operations
        .push(hirsel_core_contract::OperationId {
            definition: hirsel_syntax::DefPath::core("map.insert"),
            declaration_digest: hirsel_syntax::ContentHash([0; 32]),
        });
    fixture.builder.unit.tables.calls.push(CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: false,
        callee: Callee::Operation(operation),
        abi: hirsel_lir::FnAbiId(0),
        operation_arguments: vec![],
        arguments,
        result: None,
        destination: None,
        declared: hirsel_lir::effect::Effects::ALL,
        anchor: SourceAnchorId(0),
    });
    fixture.builder.term(
        ENTRY,
        Terminator::Call {
            call: CallId(0),
            normal: None,
            failure,
        },
    );
    fixture.finish().0
}

#[test]
fn keyed_prefix_rejects_transfer_before_preflight_and_a_different_key() {
    use hirsel_lir::structural_access::check_key_order;
    let good = keyed_call_prefix();
    assert!(check_key_order(&good).is_ok());
    let mut transfer_first = good.clone();
    let hash = transfer_first
        .instructions
        .iter()
        .position(|i| matches!(i, hirsel_lir::ir::Inst::Hash { .. }))
        .unwrap();
    let take = transfer_first
        .instructions
        .iter()
        .position(|i| {
            matches!(
                i,
                hirsel_lir::ir::Inst::Read {
                    mode: hirsel_lir::ir::ReadMode::Take,
                    ..
                }
            )
        })
        .unwrap();
    transfer_first.instructions.swap(hash, take);
    assert!(check_key_order(&transfer_first).is_err());
    let mut wrong_key = good;
    wrong_key.addresses.push(wrong_key.addresses[0].clone());
    wrong_key.addresses[1].base = hirsel_lir::ir::AddressBase::Slot(hirsel_lir::ir::SlotId(1));
    let taken = wrong_key
        .instructions
        .iter_mut()
        .find(|i| {
            matches!(
                i,
                hirsel_lir::ir::Inst::Read {
                    mode: hirsel_lir::ir::ReadMode::Take,
                    ..
                }
            )
        })
        .unwrap();
    if let hirsel_lir::ir::Inst::Read { address, .. } = taken {
        *address = hirsel_lir::ir::AddressId(1);
    }
    assert!(check_key_order(&wrong_key).is_err());
}

#[test]
fn keyed_prefix_rejects_preflight_before_invocation_admission() {
    let mut forged = keyed_call_prefix();
    let hash = forged
        .instructions
        .iter()
        .position(|i| matches!(i, hirsel_lir::ir::Inst::Hash { .. }))
        .unwrap();
    let admission = forged
        .instructions
        .iter()
        .position(|i| matches!(i, hirsel_lir::ir::Inst::FrameEnter { .. }))
        .unwrap();
    forged.instructions.swap(hash, admission);
    assert!(hirsel_lir::structural_access::check_key_order(&forged).is_err());
}

#[test]
fn keyed_prefix_checks_direct_library_body_dispatch_too() {
    use hirsel_lir::ir::*;
    let mut unit = keyed_call_prefix();
    let id = SymbolId(unit.tables.symbols.len() as u32);
    unit.tables.symbols.push(Symbol {
        kind: SymbolKind::Function,
        definition: hirsel_syntax::DefPath::core("map.insert"),
        instance: None,
    });
    unit.tables.calls[0].callee = Callee::Direct(id);
    assert!(hirsel_lir::structural_access::check_key_order(&unit).is_ok());
    let hash = unit
        .instructions
        .iter()
        .position(|i| matches!(i, Inst::Hash { .. }))
        .unwrap();
    let take = unit
        .instructions
        .iter()
        .position(|i| {
            matches!(
                i,
                Inst::Read {
                    mode: ReadMode::Take,
                    ..
                }
            )
        })
        .unwrap();
    unit.instructions.swap(hash, take);
    assert!(hirsel_lir::structural_access::check_key_order(&unit).is_err());
}

#[test]
fn keyed_prefix_survives_a_cfg_split_but_every_path_must_preflight() {
    use hirsel_lir::ir::*;
    use hirsel_lir::structural_access::check_key_order;
    let mut unit = keyed_call_prefix();
    let entry = unit.entry.index();
    let original = unit.blocks[entry].instructions;
    let take = original
        .get(&unit.instructions)
        .unwrap()
        .iter()
        .position(|inst| {
            matches!(
                inst,
                Inst::Read {
                    mode: ReadMode::Take,
                    ..
                }
            )
        })
        .unwrap();
    let call = BlockId(unit.blocks.len() as u32);
    let mut continuation = unit.blocks[entry].clone();
    continuation.instructions =
        PoolRange::new(original.start + take as u32, original.len - take as u32);
    unit.blocks[entry].instructions.len = take as u32;
    unit.blocks.push(continuation);
    unit.terminators.push(unit.terminators[entry].clone());
    let edge = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(Edge {
        target: call,
        arguments: PoolRange::EMPTY,
    });
    unit.terminators[entry] = Terminator::Jump(edge);
    assert!(check_key_order(&unit).is_ok());
    // An admitted arm which bypasses only the key Hash cannot borrow the
    // other arm's successful proof at their common Call.
    let mut bypass = unit.clone();
    let preflight = BlockId(bypass.blocks.len() as u32);
    let mut row = bypass.blocks[entry].clone();
    let hash = row
        .instructions
        .get(&bypass.instructions)
        .unwrap()
        .iter()
        .position(|inst| matches!(inst, Inst::Hash { .. }))
        .unwrap();
    row.instructions = PoolRange::new(original.start + hash as u32, take as u32 - hash as u32);
    bypass.blocks[entry].instructions.len = hash as u32;
    bypass.blocks.push(row);
    bypass.terminators.push(Terminator::Jump(edge));
    let yes = EdgeId(bypass.tables.edges.len() as u32);
    bypass.tables.edges.push(Edge {
        target: preflight,
        arguments: PoolRange::EMPTY,
    });
    bypass.terminators[entry] = Terminator::Branch {
        condition: ValueId(0),
        yes,
        no: edge,
    };
    assert!(check_key_order(&bypass).is_err());
    // A failed Hash carries no successful proof into a resumed Call either.
    let mut failed = unit;
    let failure = failed.blocks[entry]
        .instructions
        .get(&failed.instructions)
        .unwrap()
        .iter()
        .find_map(|inst| match inst {
            Inst::Hash { failure, .. } => Some(*failure),
            _ => None,
        })
        .unwrap();
    let cleanup = EdgeId(failed.tables.edges.len() as u32);
    failed.tables.edges.push(Edge {
        target: call,
        arguments: PoolRange::EMPTY,
    });
    let failed_hash = FailureId(failed.tables.failures.len() as u32);
    let mut row = failed.tables.failures[failure.index()].clone();
    row.cleanup = cleanup;
    failed.tables.failures.push(row);
    for inst in &mut failed.instructions {
        if let Inst::Hash { failure, .. } = inst {
            *failure = failed_hash;
        }
    }
    assert!(check_key_order(&failed).is_err());
}
