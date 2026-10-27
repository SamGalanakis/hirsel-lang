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
