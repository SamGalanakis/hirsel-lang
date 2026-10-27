//! Hand-built validated module inputs, not public source qualification.
use super::*;
use crate::NoOperations;
use crate::build::Layouts;
use hirsel_lir::layout_types::{ScanStep, SizeTerm};
use hirsel_lir::logical_storage::{LogicalBytes, LogicalStorageGraph, StorageClass, StorageFact};
use hirsel_lir::operation::{
    BuiltinType as B, NativeType as T, OperationArgument as A, Primitive as P,
};
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_lir::sealed_payload_source::{
    SealSourceWitness, SourceEvent, SourceEventKind, SourceSeal,
};
use hirsel_lir::stage::Rc;
use hirsel_lir::structural::{Shape, ShapeGraph};
use hirsel_lir::structural_access::AccessPlan;
mod inventory_92;

fn list_type(child: T) -> T {
    T::Builtin(B::List, vec![A::Type(child)])
}
fn sequence(l: &mut Layouts, element: LayoutId, stride: u64) -> (LayoutId, LayoutId) {
    let number = l.scalar(ScalarKind::U64);
    let payload = l.record(&[number, number]);
    let row = l.get_mut(payload);
    row.kind = LayoutKind::Sequence { element };
    row.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Constant(stride),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    row.scan = vec![ScanStep::Sequence {
        count_offset: 0,
        offset: 16,
        stride,
        element,
    }];
    row.drop = vec![DropStep::ElementsReverse {
        count_offset: 0,
        offset: 16,
        stride,
        element,
    }];
    (l.block(payload), payload)
}
fn descriptor(
    l: &Layouts,
    root: LayoutId,
    ty: T,
    shapes: Vec<Shape>,
    facts: Vec<(u64, StorageClass)>,
) -> SealDescriptor {
    let shape = ShapeGraph {
        root: 0,
        nodes: shapes,
    };
    let projection = AccessPlan::derive(ty.clone(), shape.clone(), &l.table, root).unwrap();
    let mut pending = vec![root];
    pending.extend(projection.layouts());
    let index = hirsel_lir::LayoutIndex::new(&l.table);
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if seen.insert(id) && !matches!(l.get(id).kind, LayoutKind::Shared { .. }) {
            pending.extend(l.get(id).references());
            pending.extend(index.implicit(id));
        }
    }
    let d = SealDescriptor {
        root_type: ty,
        root_layout: root,
        projection,
        logical: LogicalStorageGraph {
            shape,
            facts: facts
                .into_iter()
                .map(|(n, class)| StorageFact {
                    bytes: LogicalBytes::Bytes(n),
                    class,
                })
                .collect(),
        },
        access: seen
            .into_iter()
            .map(|layout| StorageAccess {
                layout,
                scan: l.get(layout).scan.clone(),
            })
            .collect(),
    };
    d.check(&l.table).unwrap();
    d
}
struct Fixture {
    p: Program<Rc>,
    bytes: LayoutId,
    byte_storage: LayoutId,
    lists: [LayoutId; 3],
    payloads: [LayoutId; 3],
    shared: LayoutId,
    shared_list: LayoutId,
    shared_payload: LayoutId,
}
fn fixture() -> Fixture {
    let mut p = super::sealed_seal_tests::program(true);
    let mut l = Layouts {
        target: p.target.clone(),
        table: p.layouts,
    };
    let byte = l.scalar(ScalarKind::U8);
    let (_, byte_storage) = sequence(&mut l, byte, 1);
    let owner = l.block(byte_storage);
    let number = LayoutId(0);
    let bytes = l.record(&[owner, number, number]);
    l.get_mut(bytes).kind = LayoutKind::Slice { element: byte };
    l.get_mut(bytes).scan = vec![ScanStep::SliceOwner { offset: 0 }];
    let (lb, pb) = sequence(&mut l, bytes, 24);
    let (ln, pn) = sequence(&mut l, number, 8);
    let (ll, pl) = sequence(&mut l, ln, 8);
    let shared = l.shared(number);
    let (ls, ps) = sequence(&mut l, shared, 8);
    let mut ds = vec![
        descriptor(
            &l,
            lb,
            list_type(T::Primitive(P::Bytes)),
            vec![Shape::Sequence(1), Shape::Leaf],
            vec![(8, StorageClass::Block), (8, StorageClass::Bytes)],
        ),
        descriptor(
            &l,
            ln,
            list_type(T::Primitive(P::U64)),
            vec![Shape::Sequence(1), Shape::Leaf],
            vec![(8, StorageClass::Block), (8, StorageClass::Inline)],
        ),
        descriptor(
            &l,
            ll,
            list_type(list_type(T::Primitive(P::U64))),
            vec![Shape::Sequence(1), Shape::Sequence(2), Shape::Leaf],
            vec![
                (8, StorageClass::Block),
                (8, StorageClass::Block),
                (8, StorageClass::Inline),
            ],
        ),
        descriptor(
            &l,
            ls,
            list_type(T::Builtin(B::Shared, vec![A::Type(T::Primitive(P::U64))])),
            vec![Shape::Sequence(1), Shape::Transparent(2), Shape::Leaf],
            vec![
                (8, StorageClass::Block),
                (8, StorageClass::Shared),
                (8, StorageClass::Inline),
            ],
        ),
    ];
    p.layouts = l.table;
    p.units[0].tables.seal_descriptors.append(&mut ds);
    p.units[0].types.push(LirType {
        repr: ValueRepr::Uncounted,
        layout: lb,
    });
    p.units[0].values.push(ValueInfo {
        ty: LirTypeId(3),
        holding: Holding::Plain,
    });
    p.units[0].tables.edges.push(Edge {
        target: BlockId(0),
        arguments: PoolRange::EMPTY,
    });
    p.units[0].tables.failures.push(FailureEdge {
        cleanup: EdgeId(0),
        rule: Some("MEM-6".into()),
        anchor: SourceAnchorId(0),
        dispose: PoolRange::EMPTY,
    });
    p.units[0].blocks.push(Block {
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        kind: BlockKind::FailureCleanup,
        anchor: SourceAnchorId(0),
    });
    Fixture {
        p,
        bytes,
        byte_storage,
        lists: [lb, ln, ll],
        payloads: [pb, pn, pl],
        shared,
        shared_list: ls,
        shared_payload: ps,
    }
}
fn set_root(p: &mut Program<Rc>, shape: u32) {
    let root = p.units[0].tables.seal_descriptors[shape as usize].root_layout;
    p.layouts.layouts[1].kind = LayoutKind::SealedRegion { root };
    p.units[0].types[1] = LirType {
        layout: root,
        repr: if shape == 0 {
            ValueRepr::Scalar(ScalarKind::U64)
        } else {
            ValueRepr::Block
        },
    };
    if shape == 3 {
        p.units[0].types[3].layout = root;
    }
    p.units[0].values[4].holding = if shape == 0 {
        Holding::Plain
    } else {
        Holding::Owned
    };
}
fn config(p: &mut Program<Rc>, shape: u32, allocations: &[u32], full: bool) -> Config {
    let mut events = vec![SourceEvent {
        anchor: p.units[0].tables.source_anchors[0].clone(),
        kind: SourceEventKind::Seal,
        root_type: p.units[0].tables.seal_descriptors[shape as usize]
            .root_type
            .clone(),
    }];
    for (i, d) in allocations.iter().enumerate() {
        let mut anchor = events[0].anchor.clone();
        anchor.statement = (i + 1) as u32;
        p.units[0].tables.source_anchors.push(anchor.clone());
        events.push(SourceEvent {
            anchor,
            kind: SourceEventKind::Allocate,
            root_type: p.units[0].tables.seal_descriptors[*d as usize]
                .root_type
                .clone(),
        });
    }
    let h = &p.units[0].header;
    let reference = hirsel_lir::invocations::ReferenceBody {
        definition: h.definition.clone(),
        instance: h.instance,
        body: 0,
    };
    let witness = SealSourceWitness::from_expectations(
        h.implementation_revision,
        h.definition.clone(),
        h.implementation_revision,
        h.instance,
        &events,
        &[],
    );
    Config {
        seal_sources: vec![SourceSeal {
            reference,
            witness,
            events,
            payloads: vec![],
        }],
        seal_descriptors: p.units[0].tables.seal_descriptors.clone(),
        full_recount_every_step: full,
        ..Config::default()
    }
}
fn alloc(
    m: &mut Machine<'_, Rc>,
    region: bool,
    layout: LayoutId,
    image: Extent,
    count: u64,
) -> Ptr {
    let ptr = if region {
        m.region_alloc(0, layout, image.len()).unwrap()
    } else {
        m.heap_alloc(layout, image.len(), 8).unwrap()
    };
    m.ledger.get_mut(ptr.alloc).extent.place(0, image);
    if !region {
        m.ledger.get_mut(ptr.alloc).count = Some(Count::Finite(count));
    }
    ptr
}
fn header(count: u64, stride: u64) -> Extent {
    let mut e = Extent::new(16 + count * stride);
    e.write_bytes(0, &count.to_le_bytes());
    e.write_bytes(8, &count.to_le_bytes());
    e
}
fn nums(m: &mut Machine<'_, Rc>, region: bool, payload: LayoutId, ns: &[u64], count: u64) -> Ptr {
    let mut e = header(ns.len() as u64, 8);
    for (i, n) in ns.iter().enumerate() {
        e.write_bytes(16 + i as u64 * 8, &n.to_le_bytes());
    }
    alloc(m, region, payload, e, count)
}
fn byte_list(
    m: &mut Machine<'_, Rc>,
    x: &Fixture,
    region: bool,
    len: u64,
    distinct: bool,
    occurrences: usize,
) -> Ptr {
    let mut e = header(occurrences as u64, 24);
    let mut repeated = None;
    for i in 0..occurrences {
        let ptr = if !distinct && repeated.is_some() {
            repeated.unwrap()
        } else {
            let mut data = header(len, 1);
            data.write_bytes(16, &vec![b'a'; len as usize]);
            let ptr = alloc(
                m,
                false,
                x.byte_storage,
                data,
                if distinct { 1 } else { occurrences as u64 },
            );
            repeated = Some(ptr);
            ptr
        };
        let mut slice = m.slice_image(x.bytes, ptr, 8, len).unwrap();
        if let Some(Cell::Data { counted, .. }) = slice.cells.get_mut(&0) {
            *counted = true;
        }
        e.place(16 + i as u64 * 24, slice);
    }
    alloc(m, region, x.payloads[0], e, 1)
}
fn inventory(m: &mut Machine<'_, Rc>, f: &mut Frame, ptr: Ptr, shape: u32, ordinal: u32) {
    // Direct module fixture supplies the H allocation made at the independently
    // selected source event. Production obtains this binding from Construct.
    m.seal_allocations.insert(
        ptr.alloc,
        (
            f.unit,
            m.unit(f.unit).tables.source_anchors[ordinal as usize].clone(),
        ),
    );
    // Operand comes from a real SSA value; this fixture models its last use.
    f.values[6] = Some(Held {
        val: Val::Ptr(ptr),
        credits: 0,
        borrow: None,
    });
    m.exec(
        f,
        &Inst::ArenaInventory {
            arena: ValueId(5),
            allocation: SourceAnchorId(ordinal),
            value: ValueId(6),
            shape: SealDescriptorId(shape),
            previous: None,
        },
    )
    .unwrap();
    f.values[6] = None;
}
fn prepare(m: &mut Machine<'_, Rc>, cap: u64) -> (Frame, Ptr) {
    let (f, g) = super::sealed_seal_tests::prepared(m, true);
    m.ledger.regions[0].budget.as_mut().unwrap().remaining = cap;
    (f, g)
}
fn root(m: &mut Machine<'_, Rc>, f: &mut Frame, val: Val) {
    // Initial scalar from the common grant fixture carries no holder.
    f.values[4] = None;
    m.define_fresh(f, ValueId(4), val).unwrap();
}
fn seal(m: &mut Machine<'_, Rc>, f: &mut Frame, shape: u32) {
    m.exec(
        f,
        &Inst::SealArena {
            result: ValueId(3),
            grant: ValueId(5),
            root: ValueId(4),
            shape: SealDescriptorId(shape),
            failure: FailureId(0),
            origin: SourceAnchorId(0),
        },
    )
    .unwrap();
}
fn recount(m: &mut Machine<'_, Rc>, f: &Frame) {
    m.stack.push(Activity::Frame(Box::new(f.clone())));
    m.steps += 1;
    m.check_counts().unwrap();
    m.stack.pop();
}
fn drain(m: &mut Machine<'_, Rc>, work: Vec<Work>) {
    let mut d = Drain {
        caller: None,
        operation: None,
        work,
        failed: false,
    };
    while let Some(w) = d.work.pop() {
        assert!(m.step_drain(&mut d, w).unwrap().is_none());
    }
}
fn finish(m: &mut Machine<'_, Rc>, f: &mut Frame, g: Ptr, success: bool) {
    if success {
        m.exit_region(f, 0).unwrap();
        let owner = m.take_value(f, ValueId(3)).unwrap();
        let slot = m
            .ledger
            .alloc(Owner::Frame, Some(LayoutId(1)), 8, 8, 8, None);
        m.write_value(slot, LayoutId(1), owner).unwrap();
        recount(m, f);
        let mut work = vec![];
        m.release_sealed(slot, LayoutId(1), &mut work).unwrap();
        drain(m, work);
        m.ledger.free(slot.alloc).unwrap();
    } else {
        let taken = m.take_value(f, ValueId(4)).unwrap();
        if taken.credited {
            let mut work = vec![];
            match taken.val {
                Val::Ptr(p) if m.counted_target(p) => m.decrement(p, None, &mut work).unwrap(),
                Val::Image(e) => {
                    for c in e.cells.values() {
                        if let Cell::Data {
                            ptr, counted: true, ..
                        } = c
                        {
                            m.decrement(*ptr, None, &mut work).unwrap()
                        }
                    }
                }
                _ => {}
            };
            drain(m, work);
        }
        // Entered cleanup of retained outgoing cells before lexical close.
        let ids = m.ledger.regions[0].allocations.clone();
        let mut work = vec![];
        for id in &ids {
            let a = m.ledger.get(*id);
            if a.state != AllocState::Live {
                continue;
            }
            let base = Ptr {
                alloc: *id,
                generation: a.generation,
                offset: 0,
                path: 0,
            };
            let cells: Vec<_> = a
                .extent
                .cells
                .iter()
                .filter_map(|(o, c)| matches!(c, Cell::Data { counted: true, .. }).then_some(*o))
                .collect();
            for o in cells {
                m.release_cell(base.at(o), None, &mut work).unwrap();
            }
        }
        drain(m, work);
        m.exit_region(f, 0).unwrap();
    }
    m.ledger.free(g.alloc).unwrap();
    recount(m, f);
    m.audit().unwrap();
}

#[test]
fn sealed_graph_b1_b2_heap_and_arena_roots_charge_once_and_read_twice() {
    for full in [false, true] {
        for arena in [false, true] {
            for cap in [7, 8, 15, 16, 17] {
                let mut x = fixture();
                let shape = if arena { 3 } else { 2 };
                set_root(&mut x.p, shape);
                let c = config(&mut x.p, shape, if arena { &[3] } else { &[] }, full);
                let mut ops = NoOperations;
                let mut m = Machine::new(&x.p, &c, &mut ops);
                let (mut f, g) = prepare(&mut m, cap);
                let leaf = nums(
                    &mut m,
                    false,
                    x.payloads[1],
                    if arena { &[7] } else { &[11, 12] },
                    1,
                );
                if arena {
                    if !m.charge(0, 8) {
                        let mut w = vec![];
                        m.decrement(leaf, None, &mut w).unwrap();
                        drain(&mut m, w);
                        finish(&mut m, &mut f, g, false);
                        continue;
                    }
                    let mut e = header(1, 8);
                    e.put_cell(
                        16,
                        8,
                        Cell::Data {
                            ptr: leaf,
                            counted: true,
                            borrow: None,
                        },
                    );
                    let p = alloc(&mut m, true, x.payloads[2], e, 1);
                    inventory(&mut m, &mut f, p, 3, 1);
                    root(&mut m, &mut f, Val::Ptr(p));
                } else {
                    root(&mut m, &mut f, Val::Ptr(leaf));
                }
                recount(&mut m, &f);
                seal(&mut m, &mut f, shape);
                let success = cap >= 16;
                assert_eq!(f.values[3].is_some(), success);
                recount(&mut m, &f);
                if success {
                    let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
                        panic!()
                    };
                    let id = m.ledger.sealed_control(control).unwrap();
                    for _ in 0..2 {
                        let loan = m.ledger.begin_sealed_loan(id).unwrap();
                        let e = m
                            .ledger
                            .sealed_bit_root(id, loan, x.lists[if arena { 2 } else { 1 }])
                            .unwrap();
                        assert!(
                            matches!(e.cells.get(&0),Some(Cell::Data{ptr,..}) if m.ledger.check(*ptr).unwrap().owner==Owner::Region(0))
                        );
                        m.ledger.end_sealed_loan(id, loan).unwrap();
                    }
                }
                finish(&mut m, &mut f, g, success);
            }
        }
    }
}

#[test]
fn sealed_graph_b3_b4_b7_dead_and_live_repeated_or_distinct_leaves() {
    for full in [false, true] {
        for live in [false, true] {
            for distinct in [false, true] {
                for len in [1, 3] {
                    let total = 16 + 2 * len;
                    for cap in [15, 16, total - 1, total] {
                        let mut x = fixture();
                        let shape = if live { 1 } else { 0 };
                        set_root(&mut x.p, shape);
                        let c = config(&mut x.p, shape, &[1], full);
                        let mut ops = NoOperations;
                        let mut m = Machine::new(&x.p, &c, &mut ops);
                        let (mut f, g) = prepare(&mut m, cap);
                        if !m.charge(0, 16) {
                            finish(&mut m, &mut f, g, false);
                            continue;
                        }
                        let retained = byte_list(&mut m, &x, true, len, distinct, 2);
                        inventory(&mut m, &mut f, retained, 1, 1);
                        if live {
                            root(&mut m, &mut f, Val::Ptr(retained));
                        }
                        recount(&mut m, &f);
                        seal(&mut m, &mut f, shape);
                        let success = cap >= total;
                        assert_eq!(f.values[3].is_some(), success, "cap {cap}, len {len}");
                        if !success {
                            assert_eq!(m.failures.last().unwrap().rule, "MEM-6");
                            assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, cap - 16);
                        }
                        recount(&mut m, &f);
                        finish(&mut m, &mut f, g, success);
                    }
                }
            }
        }
    }
}

#[test]
fn sealed_graph_s5_external_heap_alias_survives_evacuation_and_mutation() {
    for full in [false, true] {
        let mut x = fixture();
        set_root(&mut x.p, 2);
        x.p.units[0].values.push(ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Owned,
        });
        let c = config(&mut x.p, 2, &[], full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 16);
        let source = nums(&mut m, false, x.payloads[1], &[11, 12], 2);
        root(&mut m, &mut f, Val::Ptr(source));
        m.define_fresh(&mut f, ValueId(7), Val::Ptr(source))
            .unwrap();
        recount(&mut m, &f);
        seal(&mut m, &mut f, 2);
        recount(&mut m, &f);
        assert_eq!(
            m.ledger.check(source).unwrap().count,
            Some(Count::Finite(1))
        );
        m.ledger
            .get_mut(source.alloc)
            .extent
            .write_bytes(16, &99u64.to_le_bytes());
        let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
            panic!()
        };
        let id = m.ledger.sealed_control(control).unwrap();
        let loan = m.ledger.begin_sealed_loan(id).unwrap();
        let image = m.ledger.sealed_bit_root(id, loan, x.lists[1]).unwrap();
        let Cell::Data { ptr, .. } = image.cells[&0] else {
            panic!()
        };
        assert_ne!(ptr.alloc, source.alloc);
        assert_eq!(m.read_scalar(ptr.at(16), 8).unwrap(), 11);
        m.ledger.end_sealed_loan(id, loan).unwrap();
        let taken = m.take_value(&mut f, ValueId(7)).unwrap();
        assert!(taken.credited);
        let mut work = vec![];
        m.decrement(source, None, &mut work).unwrap();
        drain(&mut m, work);
        finish(&mut m, &mut f, g, true);
    }
}

#[test]
fn sealed_graph_s7_dead_retained_shared_multiplicity_and_live_loan_drop() {
    for full in [false, true] {
        for dead in [false, true] {
            let mut x = fixture();
            let shape = if dead { 0 } else { 4 };
            set_root(&mut x.p, shape);
            x.p.units[0].types[3].layout = x.shared_list;
            let c = config(&mut x.p, shape, &[4], full);
            let mut ops = NoOperations;
            let mut m = Machine::new(&x.p, &c, &mut ops);
            let (mut f, g) = prepare(&mut m, 0);
            let LayoutKind::Shared { payload } = m.record(x.shared).unwrap().kind else {
                panic!()
            };
            let mut bytes = Extent::new(8);
            bytes.write_bytes(0, &73u64.to_le_bytes());
            let shared = alloc(&mut m, false, payload, bytes, 2);
            m.ledger.get_mut(shared.alloc).count = Some(Count::Root(2));
            let mut image = header(2, 8);
            for o in [16, 24] {
                image.put_cell(
                    o,
                    8,
                    Cell::Data {
                        ptr: shared,
                        counted: true,
                        borrow: None,
                    },
                );
            }
            let retained = alloc(&mut m, true, x.shared_payload, image, 1);
            inventory(&mut m, &mut f, retained, 4, 1);
            if !dead {
                root(&mut m, &mut f, Val::Ptr(retained));
            }
            recount(&mut m, &f);
            m.ledger.get_mut(shared.alloc).count = Some(Count::Root(1));
            let refused = m.exec(
                &mut f,
                &Inst::SealArena {
                    result: ValueId(3),
                    grant: ValueId(5),
                    root: ValueId(4),
                    shape: SealDescriptorId(shape),
                    failure: FailureId(0),
                    origin: SourceAnchorId(0),
                },
            );
            assert_eq!(refused.err().unwrap().kind, V::CountMismatch);
            assert!(f.values[3].is_none());
            m.ledger.get_mut(shared.alloc).count = Some(Count::Root(2));
            seal(&mut m, &mut f, shape);
            recount(&mut m, &f);
            assert_eq!(m.ledger.check(shared).unwrap().count, Some(Count::Root(2)));
            let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
                panic!()
            };
            let id = m.ledger.sealed_control(control).unwrap();
            let loan = m.ledger.begin_sealed_loan(id).unwrap();
            assert_eq!(
                m.ledger.begin_sealed_drop(id).unwrap_err().kind,
                V::LoanOutlivesFrame
            );
            m.ledger.end_sealed_loan(id, loan).unwrap();
            finish(&mut m, &mut f, g, true);
            assert_eq!(m.ledger.get(shared.alloc).state, AllocState::Dead);
        }
    }
}

#[test]
fn sealed_graph_independent_missing_edge_owner_grant_loan_and_charge_corruptions_have_twins() {
    for full in [false, true] {
        for mutation in 0..9 {
            let mut x = fixture();
            set_root(&mut x.p, 1);
            let mut c = config(&mut x.p, 1, &[1], full);
            if mutation == 6 {
                x.p.units[0].tables.seal_descriptors[1].logical.facts[1].bytes =
                    LogicalBytes::Bytes(1);
            }
            // Inputs stay independent when the candidate charge is corrupted.
            if mutation == 7 {
                let mut omitted = c.seal_sources[0].events[1].clone();
                omitted.anchor.statement = 999;
                c.seal_sources[0].events.push(omitted);
                let s = &mut c.seal_sources[0];
                s.witness = SealSourceWitness::from_expectations(
                    s.witness.source,
                    s.reference.definition.clone(),
                    s.witness.implementation_revision,
                    s.reference.instance,
                    &s.events,
                    &s.payloads,
                );
            }
            let mut ops = NoOperations;
            let mut m = Machine::new(&x.p, &c, &mut ops);
            let (mut f, g) = prepare(&mut m, 6);
            let retained = byte_list(&mut m, &x, true, 3, false, 2);
            root(&mut m, &mut f, Val::Ptr(retained));
            if mutation != 0 && mutation != 6 {
                inventory(&mut m, &mut f, retained, 1, 1);
            }
            let leaf = match m.ledger.get(retained.alloc).extent.cells[&16] {
                Cell::Data { ptr, .. } => ptr,
                _ => panic!(),
            };
            let outside = m.ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
            let saved = m.ledger.clone();
            let saved_frame = f.clone();
            match mutation {
                1 => m.ledger.get_mut(leaf.alloc).count = Some(Count::Finite(1)),
                2 => {
                    m.ledger.get_mut(retained.alloc).extent.cells.remove(&16);
                }
                3 => m.ledger.get_mut(outside.alloc).extent.put_cell(
                    0,
                    8,
                    Cell::Data {
                        ptr: retained,
                        counted: false,
                        borrow: Some(Borrow::Caller),
                    },
                ),
                4 => m.ledger.get_mut(retained.alloc).owner = Owner::Region(99),
                5 => f.seal_grants.clear(),
                8 => f.loans[0] = Some(retained),
                _ => {}
            }
            let n = m.ledger.allocations.len();
            let before = m.ledger.regions[0].budget.unwrap().remaining;
            let bad = m.exec(
                &mut f,
                &Inst::SealArena {
                    result: ValueId(3),
                    grant: ValueId(5),
                    root: ValueId(4),
                    shape: SealDescriptorId(1),
                    failure: FailureId(0),
                    origin: SourceAnchorId(0),
                },
            );
            assert!(bad.is_err(), "mutation {mutation} accepted");
            assert!(f.values[3].is_none());
            assert_eq!(m.ledger.allocations.len(), n);
            assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, before);
            assert!(m.read_use(&f, ValueId(5)).is_ok());
            m.ledger = saved;
            f = saved_frame;
            recount(&mut m, &f);
            m.ledger.free(outside.alloc).unwrap();
            finish(&mut m, &mut f, g, false);
            // Every malformed vector is paired with the unmodified graph execution
            // in the same two recount modes (B3/B4 above), not a weaker scalar twin.
            assert_eq!(
                m.ledger.regions[0].storage,
                crate::ledger::sealed::StorageCustody::Scope
            );
            let mut twin = fixture();
            set_root(&mut twin.p, 1);
            let c = config(&mut twin.p, 1, &[1], full);
            let mut ops = NoOperations;
            let mut m = Machine::new(&twin.p, &c, &mut ops);
            let (mut f, g) = prepare(&mut m, 6);
            let retained = byte_list(&mut m, &twin, true, 3, false, 2);
            inventory(&mut m, &mut f, retained, 1, 1);
            root(&mut m, &mut f, Val::Ptr(retained));
            seal(&mut m, &mut f, 1);
            recount(&mut m, &f);
            finish(&mut m, &mut f, g, true);
        }
    }
}

#[test]
fn sealed_graph_b8_growth_and_cow_retained_payloads_have_distinct_charges() {
    for full in [false, true] {
        for cow in [false, true] {
            let construction = if cow { 24 } else { 16 };
            let total = if cow { 33 } else { 22 };
            for cap in [total - 1, total] {
                let mut x = fixture();
                set_root(&mut x.p, 1);
                let c = config(&mut x.p, 1, if cow { &[1, 1] } else { &[1] }, full);
                let mut ops = NoOperations;
                let mut m = Machine::new(&x.p, &c, &mut ops);
                let (mut f, g) = prepare(&mut m, cap);
                assert!(m.charge(0, construction));
                let retained = byte_list(&mut m, &x, true, 3, false, 2);
                if cow {
                    let leaf = match m.ledger.get(retained.alloc).extent.cells[&16] {
                        Cell::Data { ptr, .. } => ptr,
                        _ => panic!(),
                    };
                    m.dup(leaf).unwrap();
                    let mut old = header(1, 24);
                    old.place(16, m.ledger.get(retained.alloc).extent.copy(16, 24, None));
                    let old = alloc(&mut m, true, x.payloads[0], old, 1);
                    inventory(&mut m, &mut f, old, 1, 1);
                }
                inventory(&mut m, &mut f, retained, 1, if cow { 2 } else { 1 });
                root(&mut m, &mut f, Val::Ptr(retained));
                recount(&mut m, &f);
                seal(&mut m, &mut f, 1);
                let success = cap == total;
                assert_eq!(f.values[3].is_some(), success);
                if success {
                    assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, 0);
                }
                recount(&mut m, &f);
                finish(&mut m, &mut f, g, success);
            }
        }
    }
}

#[test]
fn sealed_graph_s3_record_variants_list_and_box_preserve_active_edges() {
    for full in [false, true] {
        for active in [false, true] {
            for wrapper in 0..4 {
                let mut x = fixture();
                let mut l = Layouts {
                    target: x.p.target.clone(),
                    table: x.p.layouts,
                };
                let item = l.variant(&[&[], &[x.lists[1]]]);
                let (items, items_payload) = sequence(&mut l, item, 16);
                let extra = l.block(item);
                l.get_mut(extra).kind = LayoutKind::Box { payload: item };
                let record = l.record(&[LayoutId(0), items, extra]);
                let item_type = T::Variant(vec![vec![], vec![list_type(T::Primitive(P::U64))]]);
                let item_shape = vec![
                    Shape::Variant(vec![vec![], vec![1]]),
                    Shape::Sequence(2),
                    Shape::Leaf,
                ];
                let item_facts = vec![
                    (16, StorageClass::Inline),
                    (8, StorageClass::Block),
                    (8, StorageClass::Inline),
                ];
                let di = descriptor(
                    &l,
                    items,
                    list_type(item_type.clone()),
                    vec![
                        Shape::Sequence(1),
                        Shape::Variant(vec![vec![], vec![2]]),
                        Shape::Sequence(3),
                        Shape::Leaf,
                    ],
                    vec![
                        (8, StorageClass::Block),
                        (16, StorageClass::Inline),
                        (8, StorageClass::Block),
                        (8, StorageClass::Inline),
                    ],
                );
                let mut box_shapes = vec![Shape::Box(1)];
                box_shapes.extend(item_shape.into_iter().map(|s| match s {
                    Shape::Variant(_) => Shape::Variant(vec![vec![], vec![2]]),
                    Shape::Sequence(_) => Shape::Sequence(3),
                    s => s,
                }));
                let mut box_facts = vec![(8, StorageClass::Block)];
                box_facts.extend(item_facts);
                let db = descriptor(
                    &l,
                    extra,
                    T::Box(Box::new(item_type.clone())),
                    box_shapes,
                    box_facts,
                );
                let mut dr = descriptor(
                    &l,
                    record,
                    T::Record(vec![
                        (false, T::Primitive(P::U64)),
                        (false, list_type(item_type.clone())),
                        (false, T::Box(Box::new(item_type))),
                    ]),
                    vec![
                        Shape::Fields(vec![1, 2, 5]),
                        Shape::Leaf,
                        Shape::Sequence(3),
                        Shape::Variant(vec![vec![], vec![4]]),
                        Shape::Sequence(1),
                        Shape::Box(3),
                    ],
                    vec![
                        (24, StorageClass::Inline),
                        (8, StorageClass::Inline),
                        (8, StorageClass::Block),
                        (16, StorageClass::Inline),
                        (8, StorageClass::Block),
                        (8, StorageClass::Block),
                    ],
                );
                let record = if wrapper == 0 {
                    record
                } else {
                    let old_shape = dr.logical.shape.nodes.clone();
                    let mut nodes = vec![match wrapper {
                        1 => Shape::Fields(vec![1]),
                        2 => Shape::Transparent(1),
                        _ => Shape::Distinct(1),
                    }];
                    nodes.extend(old_shape.into_iter().map(|node| {
                        match node {
                            Shape::Fields(fields) => {
                                Shape::Fields(fields.into_iter().map(|i| i + 1).collect())
                            }
                            Shape::Sequence(i) => Shape::Sequence(i + 1),
                            Shape::Box(i) => Shape::Box(i + 1),
                            Shape::Variant(arms) => Shape::Variant(
                                arms.into_iter()
                                    .map(|arm| arm.into_iter().map(|i| i + 1).collect())
                                    .collect(),
                            ),
                            n => n,
                        }
                    }));
                    let mut facts = vec![(
                        24,
                        if wrapper == 1 {
                            StorageClass::Inline
                        } else {
                            StorageClass::Transparent
                        },
                    )];
                    facts.extend(dr.logical.facts.iter().map(|f| match f.bytes {
                        LogicalBytes::Bytes(n) => (n, f.class),
                        _ => unreachable!(),
                    }));
                    let root = if wrapper == 1 {
                        l.record(&[record])
                    } else {
                        record
                    };
                    let ty = if wrapper == 1 {
                        T::Tuple(vec![dr.root_type.clone()])
                    } else {
                        T::Named(
                            crate::build::path(if wrapper == 2 {
                                "GenericWrap"
                            } else {
                                "DistinctWrap"
                            }),
                            vec![A::Type(dr.root_type.clone())],
                        )
                    };
                    dr = descriptor(&l, root, ty, nodes, facts);
                    root
                };
                x.p.layouts = l.table;
                x.p.units[0].tables.seal_descriptors.extend([di, db, dr]);
                set_root(&mut x.p, 7);
                x.p.units[0].types[1].repr = ValueRepr::Aggregate;
                x.p.units[0].types[3].layout = items;
                x.p.units[0].types.push(LirType {
                    repr: ValueRepr::Uncounted,
                    layout: extra,
                });
                x.p.units[0].values.push(ValueInfo {
                    ty: LirTypeId(4),
                    holding: Holding::Plain,
                });
                let c = config(&mut x.p, 7, &[5, 6], full);
                let mut ops = NoOperations;
                let mut m = Machine::new(&x.p, &c, &mut ops);
                let (mut f, g) = prepare(&mut m, 32);
                let leaf = nums(
                    &mut m,
                    false,
                    x.payloads[1],
                    &[3, 4],
                    if active { 2 } else { 1 },
                );
                let variant = |yes: bool| {
                    let mut e = Extent::new(16);
                    e.write_bytes(0, &u32::from(yes).to_le_bytes());
                    if yes {
                        e.put_cell(
                            8,
                            8,
                            Cell::Data {
                                ptr: leaf,
                                counted: true,
                                borrow: None,
                            },
                        );
                    }
                    e
                };
                let mut e = header(2, 16);
                e.place(16, variant(!active));
                e.place(32, variant(active));
                let list = alloc(&mut m, true, items_payload, e, 1);
                inventory(&mut m, &mut f, list, 5, 1);
                let boxed = alloc(&mut m, true, item, variant(active), 1);
                m.seal_allocations.insert(
                    boxed.alloc,
                    (f.unit, m.unit(f.unit).tables.source_anchors[2].clone()),
                );
                f.values[7] = Some(Held {
                    val: Val::Ptr(boxed),
                    credits: 0,
                    borrow: None,
                });
                m.exec(
                    &mut f,
                    &Inst::ArenaInventory {
                        arena: ValueId(5),
                        allocation: SourceAnchorId(2),
                        value: ValueId(7),
                        shape: SealDescriptorId(6),
                        previous: None,
                    },
                )
                .unwrap();
                f.values[7] = None;
                let mut root_image = Extent::new(m.size(record).unwrap());
                root_image.write_bytes(0, &9u64.to_le_bytes());
                root_image.put_cell(
                    8,
                    8,
                    Cell::Data {
                        ptr: list,
                        counted: false,
                        borrow: None,
                    },
                );
                root_image.put_cell(
                    16,
                    8,
                    Cell::Data {
                        ptr: boxed,
                        counted: false,
                        borrow: None,
                    },
                );
                root(&mut m, &mut f, Val::Image(root_image));
                recount(&mut m, &f);
                seal(&mut m, &mut f, 7);
                recount(&mut m, &f);
                assert_eq!(
                    m.ledger.regions[0].budget.unwrap().remaining,
                    if active { 0 } else { 16 }
                );
                let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
                    panic!()
                };
                let id = m.ledger.sealed_control(control).unwrap();
                let loan = m.ledger.begin_sealed_loan(id).unwrap();
                let e = m.ledger.sealed_bit_root(id, loan, record).unwrap();
                assert_eq!(e.read_bytes(0, 8), &9u64.to_le_bytes());
                m.ledger.end_sealed_loan(id, loan).unwrap();
                finish(&mut m, &mut f, g, true);
            }
        }
    }
}

#[test]
fn sealed_graph_read_instruction_lends_storage_then_expires_its_loan() {
    for full in [false, true] {
        let mut x = fixture();
        set_root(&mut x.p, 2);
        x.p.units[0].values[1].holding = Holding::Borrowed(LoanId(0));
        let c = config(&mut x.p, 2, &[], full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 16);
        let source = nums(&mut m, false, x.payloads[1], &[11, 12], 1);
        root(&mut m, &mut f, Val::Ptr(source));
        seal(&mut m, &mut f, 2);
        let moved = m.take_value(&mut f, ValueId(3)).unwrap();
        let Val::Ptr(control) = moved.val else {
            panic!()
        };
        let place = m
            .ledger
            .alloc(Owner::Frame, Some(LayoutId(1)), 8, 8, 8, None);
        m.write_value(place, LayoutId(1), moved).unwrap();
        f.slots[0] = SlotStore::Own(place);
        m.define_fresh(&mut f, ValueId(0), Val::Ptr(control))
            .unwrap();
        for _ in 0..2 {
            f.loans[0] = Some(place);
            f.prepared[0] = Some(Located::Frame(0));
            m.exec(
                &mut f,
                &Inst::BorrowSealedRoot {
                    result: ValueId(1),
                    owner: ValueId(0),
                    loan: LoanId(0),
                    shape: SealDescriptorId(2),
                },
            )
            .unwrap();
            let Val::Ptr(view) = m.read_use(&f, ValueId(1)).unwrap() else {
                panic!()
            };
            assert_eq!(m.read_scalar(view.at(16), 8).unwrap(), 11);
            assert_eq!(
                m.ledger
                    .begin_sealed_drop(m.ledger.sealed_control(control).unwrap())
                    .unwrap_err()
                    .kind,
                V::LoanOutlivesFrame
            );
            m.exec(
                &mut f,
                &Inst::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
            )
            .unwrap();
            assert_eq!(
                m.read_use(&f, ValueId(1)).unwrap_err().kind,
                V::BorrowExpired
            );
            f.values[1] = None;
        }
        m.exit_region(&mut f, 0).unwrap();
        let mut work = vec![];
        m.release_sealed(place, LayoutId(1), &mut work).unwrap();
        drain(&mut m, work);
        for ptr in [place, g] {
            m.ledger.free(ptr.alloc).unwrap();
        }
        recount(&mut m, &f);
        m.audit().unwrap();
    }
}

#[test]
fn sealed_graph_deduplicated_heap_diamond_preserves_semantic_shared_roots() {
    for full in [false, true] {
        let mut x = fixture();
        let mut l = Layouts {
            target: x.p.target.clone(),
            table: x.p.layouts,
        };
        let (outer, payload) = sequence(&mut l, x.shared_list, 8);
        let ty = list_type(list_type(T::Builtin(
            B::Shared,
            vec![A::Type(T::Primitive(P::U64))],
        )));
        let d = descriptor(
            &l,
            outer,
            ty,
            vec![
                Shape::Sequence(1),
                Shape::Sequence(2),
                Shape::Transparent(3),
                Shape::Leaf,
            ],
            vec![
                (8, StorageClass::Block),
                (8, StorageClass::Block),
                (8, StorageClass::Shared),
                (8, StorageClass::Inline),
            ],
        );
        x.p.layouts = l.table;
        x.p.units[0].tables.seal_descriptors.push(d);
        x.p.units[0].types[3].layout = outer;
        let c = config(&mut x.p, 0, &[5], full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 16);
        let LayoutKind::Shared { payload: frozen } = m.record(x.shared).unwrap().kind else {
            panic!()
        };
        let mut data = Extent::new(8);
        data.write_bytes(0, &73u64.to_le_bytes());
        let shared = alloc(&mut m, false, frozen, data, 1);
        m.ledger.get_mut(shared.alloc).count = Some(Count::Root(1));
        let mut image = header(1, 8);
        image.put_cell(
            16,
            8,
            Cell::Data {
                ptr: shared,
                counted: true,
                borrow: None,
            },
        );
        let inner = alloc(&mut m, false, x.shared_payload, image, 2);
        let mut image = header(2, 8);
        for o in [16, 24] {
            image.put_cell(
                o,
                8,
                Cell::Data {
                    ptr: inner,
                    counted: true,
                    borrow: None,
                },
            );
        }
        let retained = alloc(&mut m, true, payload, image, 1);
        inventory(&mut m, &mut f, retained, 5, 1);
        recount(&mut m, &f);
        seal(&mut m, &mut f, 0);
        recount(&mut m, &f);
        assert_eq!(m.ledger.check(shared).unwrap().count, Some(Count::Root(2)));
        let cells = &m.ledger.check(retained).unwrap().extent.cells;
        assert!(
            matches!((&cells[&16],&cells[&24]),(Cell::Data{ptr:a,..},Cell::Data{ptr:b,..}) if a.alloc==b.alloc)
        );
        finish(&mut m, &mut f, g, true);
        assert_eq!(m.ledger.get(shared.alloc).state, AllocState::Dead);
    }
}

#[test]
fn sealed_graph_byte_root_keeps_view_offset_and_charges_only_its_length() {
    for full in [false, true] {
        for cap in [1, 2] {
            let mut x = fixture();
            let l = Layouts {
                target: x.p.target.clone(),
                table: x.p.layouts,
            };
            let d = descriptor(
                &l,
                x.bytes,
                T::Primitive(P::Bytes),
                vec![Shape::Leaf],
                vec![(8, StorageClass::Bytes)],
            );
            x.p.layouts = l.table;
            x.p.units[0].tables.seal_descriptors.push(d);
            set_root(&mut x.p, 5);
            x.p.units[0].types[1].repr = ValueRepr::Aggregate;
            let c = config(&mut x.p, 5, &[], full);
            let mut ops = NoOperations;
            let mut m = Machine::new(&x.p, &c, &mut ops);
            let (mut f, g) = prepare(&mut m, cap);
            let mut e = header(5, 1);
            e.write_bytes(16, b"abcde");
            let source = alloc(&mut m, false, x.byte_storage, e, 1);
            let mut e = m.slice_image(x.bytes, source, 10, 2).unwrap();
            if let Cell::Data { counted, .. } = e.cells.get_mut(&0).unwrap() {
                *counted = true;
            }
            root(&mut m, &mut f, Val::Image(e));
            recount(&mut m, &f);
            seal(&mut m, &mut f, 5);
            let success = cap == 2;
            assert_eq!(f.values[3].is_some(), success);
            recount(&mut m, &f);
            if success {
                let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
                    panic!()
                };
                let id = m.ledger.sealed_control(control).unwrap();
                let loan = m.ledger.begin_sealed_loan(id).unwrap();
                let e = m.ledger.sealed_bit_root(id, loan, x.bytes).unwrap();
                assert_eq!(e.read_bytes(8, 8), &10u64.to_le_bytes());
                assert_eq!(e.read_bytes(16, 8), &2u64.to_le_bytes());
                let Cell::Data { ptr, .. } = e.cells[&0] else {
                    panic!()
                };
                assert_eq!(m.ledger.check(ptr).unwrap().extent.read_bytes(18, 2), b"cd");
                m.ledger.end_sealed_loan(id, loan).unwrap();
            }
            finish(&mut m, &mut f, g, success);
        }
    }
}

#[test]
fn sealed_graph_actual_construct_binds_inventory_and_forged_binding_refuses() {
    for full in [false, true] {
        let mut x = fixture();
        x.p.units[0].types[3].layout = x.lists[1];
        x.p.units[0].pools.fields.push(FieldValue {
            field: 0,
            value: ValueId(4),
        });
        x.p.units[0].tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor: ConstructorKind::Sequence,
            layout: x.payloads[1],
            fields: PoolRange::new(0, 1),
            destination: DestinationClass::Region,
            placement: Some(Placement::Fixed(StorageOwner::LocalRegion(
                RegionInstanceId(0),
            ))),
            anchor: SourceAnchorId(1),
        });
        let c = config(&mut x.p, 0, &[2], full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 0);
        m.exec(
            &mut f,
            &Inst::Construct {
                result: ValueId(6),
                construct: ConstructId(0),
                failure: FailureId(0),
            },
        )
        .unwrap();
        let Val::Ptr(ptr) = m.read_use(&f, ValueId(6)).unwrap() else {
            panic!()
        };
        let actual = m.seal_allocations.remove(&ptr.alloc).unwrap();
        assert_eq!(actual.1.statement, 1);
        let event = Inst::ArenaInventory {
            arena: ValueId(5),
            allocation: SourceAnchorId(1),
            value: ValueId(6),
            shape: SealDescriptorId(2),
            previous: None,
        };
        assert_eq!(m.exec(&mut f, &event).err().unwrap().kind, V::Malformed);
        m.seal_allocations.insert(ptr.alloc, actual);
        m.exec(&mut f, &event).unwrap();
        f.values[6] = None;
        recount(&mut m, &f);
        seal(&mut m, &mut f, 0);
        finish(&mut m, &mut f, g, true);
    }
}

#[test]
fn sealed_graph_growth_replaces_retired_backing_without_new_semantic_allocation() {
    for full in [false, true] {
        let mut x = fixture();
        set_root(&mut x.p, 1);
        x.p.units[0].values.push(ValueInfo {
            ty: LirTypeId(3),
            holding: Holding::Plain,
        });
        let mut c = config(&mut x.p, 1, &[1, 1], full);
        let s = &mut c.seal_sources[0];
        s.events[2].kind = SourceEventKind::Replace;
        s.witness = SealSourceWitness::from_expectations(
            s.witness.source,
            s.reference.definition.clone(),
            s.witness.implementation_revision,
            s.reference.instance,
            &s.events,
            &s.payloads,
        );
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 22);
        assert!(m.charge(0, 16));
        let old = byte_list(&mut m, &x, true, 3, false, 1);
        inventory(&mut m, &mut f, old, 1, 1);
        let one = m
            .ledger
            .get_mut(old.alloc)
            .extent
            .take(16, 24, FREE_POISON, None);
        let Cell::Data { ptr: leaf, .. } = one.cells[&0] else {
            panic!()
        };
        m.dup(leaf).unwrap();
        let mut image = header(2, 24);
        image.place(16, one.clone());
        image.place(40, one);
        let grown = alloc(&mut m, true, x.payloads[0], image, 1);
        m.ledger.free(old.alloc).unwrap();
        f.values[6] = Some(Held {
            val: Val::Ptr(grown),
            credits: 0,
            borrow: None,
        });
        f.values[7] = Some(Held {
            val: Val::Ptr(old),
            credits: 0,
            borrow: None,
        });
        m.exec(
            &mut f,
            &Inst::ArenaInventory {
                arena: ValueId(5),
                allocation: SourceAnchorId(2),
                value: ValueId(6),
                shape: SealDescriptorId(1),
                previous: Some(ValueId(7)),
            },
        )
        .unwrap();
        f.values[6] = None;
        f.values[7] = None;
        assert_eq!(m.seal_inventory[&0].len(), 1);
        root(&mut m, &mut f, Val::Ptr(grown));
        recount(&mut m, &f);
        seal(&mut m, &mut f, 1);
        assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, 0);
        recount(&mut m, &f);
        finish(&mut m, &mut f, g, true);
    }
}
