//! Relational SSA controls. Full C still checks source, types and custody and
//! refuses public receipt execution; these graphs exercise only pairing facts.
use super::*;
use crate::abi_types::FnAbiId;
use crate::layout_types::{LayoutId, ScalarKind};
use crate::stage::{StageKind, Value};
use hirsel_syntax::ContentHash;

fn unit() -> LirUnit<Value> {
    let mut unit = LirUnit::new(UnitHeader {
        source_count: 1,
        track_caller: false,
        stage: StageKind::Value,
        lir_version: crate::LIR_VERSION,
        origin: UnitOrigin::Source,
        key: crate::objects::UnitKey(ContentHash([0; 32])),
        definition: hirsel_syntax::DefPath::core("receipt_pairing_test"),
        implementation_revision: ContentHash([1; 32]),
        instance: ContentHash([2; 32]),
        target: ContentHash([3; 32]),
        abi: FnAbiId(0),
    });
    unit.types.push(LirType {
        repr: ValueRepr::Scalar(ScalarKind::U64),
        layout: LayoutId(0),
    });
    unit
}
fn value(unit: &mut LirUnit<Value>) -> ValueId {
    let value = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Plain,
    });
    value
}
fn receipt(unit: &mut LirUnit<Value>, original: ValueId) -> ValueId {
    let result = value(unit);
    unit.instructions.push(Inst::PayloadReceipt {
        arena: crate::ir::OptionalValueId::NONE,
        result,
        value: original,
        shape: SealDescriptorId(0),
        binding: PayloadBindingId(0),
        inputs: PoolRange::EMPTY,
    });
    result
}
fn block(unit: &mut LirUnit<Value>, parameters: &[ValueId]) -> BlockId {
    let id = BlockId(unit.blocks.len() as u32);
    let parameters = PoolRange::push(&mut unit.pools.values, parameters.iter().copied());
    unit.blocks.push(Block {
        kind: BlockKind::Body,
        parameters,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    id
}
fn edge(unit: &mut LirUnit<Value>, target: BlockId, arguments: &[ValueId]) {
    let arguments = PoolRange::push(&mut unit.pools.values, arguments.iter().copied());
    unit.tables.edges.push(Edge { target, arguments });
}

#[test]
fn another_value_or_missing_receipt_producer_cannot_supply_a_snapshot() {
    let mut u = unit();
    let a = value(&mut u);
    let b = value(&mut u);
    let r = receipt(&mut u, a);
    let facts = Pairing::new(&u);
    assert!(facts.paired(a, r));
    assert!(!facts.paired(b, r));
    assert!(!facts.paired(a, b));
    u.instructions.clear();
    assert!(!Pairing::new(&u).paired(a, r));
}

#[test]
fn capture_entry_pairs_only_its_original_selector_and_preserves_snapshot_versions() {
    let mut u = unit();
    let a = value(&mut u);
    let b = value(&mut u);
    let ra = value(&mut u);
    let rb = value(&mut u);
    let newer = value(&mut u);
    u.captures = vec![a, b];
    u.tables.receipt_entry = Some(ReceiptEntry {
        contract: ContentHash([4; 32]),
        parameters: vec![],
        captures: vec![
            ReceiptEntrySlot {
                source: 0,
                slot: SlotId(0),
                version: ra,
            },
            ReceiptEntrySlot {
                source: 1,
                slot: SlotId(1),
                version: rb,
            },
        ],
        result: None,
        environment: Some(LayoutId(0)),
    });
    let facts = Pairing::new(&u);
    assert!(facts.paired(a, ra));
    assert!(facts.paired(b, rb));
    assert!(!facts.paired(a, rb));
    assert!(!facts.paired(b, ra));
    assert!(!facts.paired(newer, ra));
    // An equal numeric parameter selector belongs to a separate namespace.
    u.parameters = vec![newer];
    assert!(!Pairing::new(&u).paired(newer, ra));
    u.tables.receipt_entry.as_mut().unwrap().captures.clear();
    assert!(!Pairing::new(&u).paired(a, ra));
}

#[test]
fn immutable_copy_and_borrow_aliases_keep_pairing_without_aliasing_deep_copy() {
    let mut u = unit();
    let original = value(&mut u);
    let r = receipt(&mut u, original);
    let copy = value(&mut u);
    let borrowed_receipt = value(&mut u);
    let deep = value(&mut u);
    u.instructions.extend([
        Inst::CopyValue {
            result: copy,
            value: original,
        },
        Inst::Weaken {
            result: borrowed_receipt,
            value: r,
        },
        Inst::UniqueCopy {
            result: deep,
            value: original,
            construct: ConstructId(0),
            deep: true,
            failure: FailureId(0),
        },
    ]);
    let facts = Pairing::new(&u);
    assert!(facts.paired(copy, borrowed_receipt));
    assert!(!facts.paired(deep, r));
}

#[test]
fn whole_slot_versions_need_the_new_snapshot_after_replacement() {
    let mut u = unit();
    let old = value(&mut u);
    let next = value(&mut u);
    let r = receipt(&mut u, old);
    let version = value(&mut u);
    u.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty: LirTypeId(0),
    });
    u.instructions.push(Inst::Store {
        version,
        value: next,
        address: AddressId(0),
    });
    let fresh = receipt(&mut u, version);
    let read = value(&mut u);
    u.instructions.push(Inst::Read {
        result: read,
        address: AddressId(0),
        reaching: version,
        mode: ReadMode::Bits,
    });
    let facts = Pairing::new(&u);
    assert!(!facts.paired(read, r));
    assert!(facts.paired(read, fresh));
}

#[test]
fn joins_require_per_edge_pairs_even_when_both_origin_sets_are_equal() {
    let mut u = unit();
    let a = value(&mut u);
    let b = value(&mut u);
    let ra = receipt(&mut u, a);
    let rb = receipt(&mut u, b);
    let v = value(&mut u);
    let r = value(&mut u);
    let join = block(&mut u, &[v, r]);
    edge(&mut u, join, &[a, ra]);
    edge(&mut u, join, &[b, rb]);
    assert!(Pairing::new(&u).paired(v, r));
    let first = u.tables.edges[0].arguments.start as usize;
    let second = u.tables.edges[1].arguments.start as usize;
    u.pools.values[first + 1] = rb;
    u.pools.values[second + 1] = ra;
    assert!(!Pairing::new(&u).paired(v, r));
}

#[test]
fn every_incoming_edge_must_preserve_pairing() {
    let mut u = unit();
    let a = value(&mut u);
    let b = value(&mut u);
    let ra = receipt(&mut u, a);
    let v = value(&mut u);
    let r = value(&mut u);
    let join = block(&mut u, &[v, r]);
    edge(&mut u, join, &[a, ra]);
    assert!(Pairing::new(&u).paired(v, r));
    edge(&mut u, join, &[b, ra]);
    assert!(!Pairing::new(&u).paired(v, r));
}

#[test]
fn feedback_preserves_a_grounded_pair_but_cannot_swap_its_receipts() {
    let mut u = unit();
    let a = value(&mut u);
    let b = value(&mut u);
    let ra = receipt(&mut u, a);
    let rb = receipt(&mut u, b);
    let v = value(&mut u);
    let r = value(&mut u);
    let other = value(&mut u);
    let other_r = value(&mut u);
    let join = block(&mut u, &[v, r, other, other_r]);
    edge(&mut u, join, &[a, ra, b, rb]);
    edge(&mut u, join, &[v, r, other, other_r]);
    assert!(Pairing::new(&u).paired(v, r));
    let feedback = u.tables.edges[1].arguments.start as usize;
    u.pools.values[feedback + 1] = other_r;
    u.pools.values[feedback + 3] = r;
    assert!(!Pairing::new(&u).paired(v, r));
}

#[test]
fn a_self_supporting_cycle_or_alias_cycle_is_not_a_producer() {
    let mut u = unit();
    let v = value(&mut u);
    let r = value(&mut u);
    let join = block(&mut u, &[v, r]);
    edge(&mut u, join, &[v, r]);
    assert!(!Pairing::new(&u).paired(v, r));
    u.instructions.push(Inst::Weaken {
        result: r,
        value: r,
    });
    assert!(!Pairing::new(&u).paired(v, r));
}

#[test]
fn projected_payload_147_checks_each_field_variant_index_and_root() {
    use super::super::projection::Projections;
    use crate::sealed_payload_source::{PayloadProducer as P, PayloadProjection as Q};
    let mut u = unit();
    let root = value(&mut u);
    let index = value(&mut u);
    let other_index = value(&mut u);
    let result = value(&mut u);
    let alias = value(&mut u);
    u.parameters = vec![root, index, other_index];
    let projection = PoolRange::push(
        &mut u.pools.projections,
        [
            ProjectionStep::Field(2),
            ProjectionStep::Index(index),
            ProjectionStep::VariantField {
                variant: 1,
                field: 0,
            },
        ],
    );
    u.addresses.push(AddressInfo {
        base: AddressBase::Value(root),
        source_place: None,
        unchecked: vec![],
        projection,
        ty: LirTypeId(0),
    });
    u.instructions.extend([
        Inst::Read {
            result,
            address: AddressId(0),
            reaching: root,
            mode: ReadMode::Bits,
        },
        Inst::CopyValue {
            result: alias,
            value: result,
        },
    ]);
    let mut whole = vec![vec![]; u.values.len()];
    whole[root.index()] = vec![P::Parameter(3)];
    whole[index.index()] = vec![P::Parameter(4)];
    whole[other_index.index()] = vec![P::Parameter(5)];
    let wanted = P::Projected {
        root: Box::new(P::Parameter(3)),
        path: vec![
            Q::Field(2),
            Q::Index(Box::new(P::Parameter(4))),
            Q::VariantField {
                variant: 1,
                field: 0,
            },
        ],
    };
    assert!(Projections::new(&u).matches(alias, &wanted, &whole));
    assert!(!Projections::new(&u).matches(root, &wanted, &whole));
    assert!(!Projections::new(&u).matches(alias, &P::Parameter(3), &whole));
    for step in [ProjectionStep::Field(1), ProjectionStep::Payload] {
        u.pools.projections[0] = step;
        assert!(!Projections::new(&u).matches(alias, &wanted, &whole));
    }
    u.pools.projections[0] = ProjectionStep::Field(2);
    u.pools.projections[1] = ProjectionStep::Index(other_index);
    assert!(!Projections::new(&u).matches(alias, &wanted, &whole));
    u.pools.projections[1] = ProjectionStep::Index(index);
    u.pools.projections[2] = ProjectionStep::VariantField {
        variant: 0,
        field: 0,
    };
    assert!(!Projections::new(&u).matches(alias, &wanted, &whole));
    u.pools.projections[2] = ProjectionStep::VariantField {
        variant: 1,
        field: 0,
    };
    whole[root.index()] = vec![P::Parameter(6)];
    assert!(!Projections::new(&u).matches(alias, &wanted, &whole));
}

#[test]
fn projected_payload_147_composes_reads_and_requires_grounded_joins() {
    use super::super::projection::Projections;
    use crate::sealed_payload_source::{PayloadProducer as P, PayloadProjection as Q};
    let mut u = unit();
    let root = value(&mut u);
    let field = value(&mut u);
    let result = value(&mut u);
    let joined = value(&mut u);
    u.parameters = vec![root];
    for (base, step) in [
        (root, ProjectionStep::Payload),
        (field, ProjectionStep::Field(2)),
    ] {
        let projection = PoolRange::push(&mut u.pools.projections, [step]);
        u.addresses.push(AddressInfo {
            base: AddressBase::Value(base),
            source_place: None,
            unchecked: vec![],
            projection,
            ty: LirTypeId(0),
        });
    }
    u.instructions.extend([
        Inst::Read {
            result: field,
            address: AddressId(0),
            reaching: root,
            mode: ReadMode::Bits,
        },
        Inst::Read {
            result,
            address: AddressId(1),
            reaching: field,
            mode: ReadMode::Bits,
        },
    ]);
    let join = block(&mut u, &[joined]);
    edge(&mut u, join, &[result]);
    edge(&mut u, join, &[joined]);
    let mut whole = vec![vec![]; u.values.len()];
    whole[root.index()] = vec![P::Parameter(0)];
    let wanted = P::Projected {
        root: Box::new(P::Parameter(0)),
        path: vec![Q::Field(0), Q::Field(2)],
    };
    assert!(Projections::new(&u).matches(joined, &wanted, &whole));
    let first = u.tables.edges[0].arguments.start as usize;
    u.pools.values[first] = field;
    assert!(!Projections::new(&u).matches(joined, &wanted, &whole));
    u.pools.values[first] = joined;
    assert!(!Projections::new(&u).matches(joined, &wanted, &whole));
}

#[test]
fn projected_payload_147_requires_every_root_and_index_predecessor_even_with_equal_origin_sets() {
    use super::super::projection::Projections;
    use crate::sealed_payload_source::{PayloadProducer as P, PayloadProjection as Q};
    let mut u = unit();
    let root = value(&mut u);
    let index = value(&mut u);
    let unknown = value(&mut u);
    let joined_root = value(&mut u);
    let joined_index = value(&mut u);
    let result = value(&mut u);
    u.parameters = vec![root, index];
    let join = block(&mut u, &[joined_root, joined_index]);
    edge(&mut u, join, &[root, index]);
    let projection = PoolRange::push(
        &mut u.pools.projections,
        [ProjectionStep::Index(joined_index)],
    );
    u.addresses.push(AddressInfo {
        base: AddressBase::Value(joined_root),
        source_place: None,
        unchecked: vec![],
        projection,
        ty: LirTypeId(0),
    });
    u.instructions.push(Inst::Read {
        result,
        address: AddressId(0),
        reaching: joined_root,
        mode: ReadMode::Bits,
    });
    let mut whole = vec![vec![]; u.values.len()];
    for v in [root, joined_root] {
        whole[v.index()] = vec![P::Parameter(0)];
    }
    for v in [index, joined_index] {
        whole[v.index()] = vec![P::Parameter(1)];
    }
    let wanted = P::Projected {
        root: Box::new(P::Parameter(0)),
        path: vec![Q::Index(Box::new(P::Parameter(1)))],
    };
    assert!(Projections::new(&u).matches(result, &wanted, &whole));
    edge(&mut u, join, &[root, unknown]);
    assert!(!Projections::new(&u).matches(result, &wanted, &whole));
    let second = u.tables.edges[1].arguments.start as usize;
    u.pools.values[second] = unknown;
    u.pools.values[second + 1] = index;
    assert!(!Projections::new(&u).matches(result, &wanted, &whole));
    u.pools.values[second] = root;
    assert!(Projections::new(&u).matches(result, &wanted, &whole));
}
