use super::*;

fn seal_inst(shape: u32) -> Inst {
    Inst::SealArena {
        result: ValueId(3),
        grant: ValueId(5),
        root: ValueId(4),
        shape: SealDescriptorId(shape),
        failure: FailureId(0),
        origin: SourceAnchorId(0),
    }
}

fn corruption(which: usize) {
    let mut violations = vec![];
    for full in [false, true] {
        let mut x = fixture();
        let shape = if which == 0 { 4 } else { 0 };
        set_root(&mut x.p, shape);
        if which == 0 {
            x.p.units[0].types[3].layout = x.shared_list;
        }
        x.p.units[0].blocks[0].instructions =
            PoolRange::push(&mut x.p.units[0].instructions, [seal_inst(shape)]);
        let c = config(&mut x.p, shape, &[if which == 0 { 4 } else { 1 }], full);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let (mut f, g) = prepare(&mut m, 6);
        let (retained, shared) = if which == 0 {
            let LayoutKind::Shared { payload } = m.record(x.shared).unwrap().kind else {
                panic!()
            };
            let mut bytes = Extent::new(8);
            bytes.write_bytes(0, &73u64.to_le_bytes());
            let shared = alloc(&mut m, false, payload, bytes, 2);
            m.ledger.get_mut(shared.alloc).count = Some(Count::Root(2));
            let mut image = header(2, 8);
            for offset in [16, 24] {
                image.put_cell(
                    offset,
                    8,
                    Cell::Data {
                        ptr: shared,
                        counted: true,
                        borrow: None,
                    },
                );
            }
            (
                alloc(&mut m, true, x.shared_payload, image, 1),
                Some(shared),
            )
        } else {
            (byte_list(&mut m, &x, true, 3, false, 2), None)
        };
        inventory(&mut m, &mut f, retained, if which == 0 { 4 } else { 1 }, 1);
        if which == 0 {
            root(&mut m, &mut f, Val::Ptr(retained));
        }
        recount(&mut m, &f);
        let clean_ledger = m.ledger.clone();
        let clean_inventory = m.seal_inventory.clone();
        let clean_frame = f.clone();
        let clean_holders = m.holders.clone();
        let clean_physical = m.physical.clone();
        match which {
            0 => m.ledger.get_mut(shared.unwrap().alloc).count = Some(Count::Root(1)),
            1 => {
                // A real physical child outside the declared sequence extent.
                // H must inspect cells independently of semantic projection.
                let extra = nums(&mut m, false, x.payloads[1], &[99], 1);
                m.ledger.get_mut(retained.alloc).extent.put_cell(
                    0,
                    8,
                    Cell::Data {
                        ptr: extra,
                        counted: true,
                        borrow: None,
                    },
                );
            }
            2 => {
                let outside = m.ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
                m.ledger.get_mut(outside.alloc).extent.put_cell(
                    0,
                    8,
                    Cell::Data {
                        ptr: retained,
                        counted: false,
                        borrow: Some(Borrow::Caller),
                    },
                );
            }
            5 => {
                m.seal_inventory
                    .get_mut(&0)
                    .unwrap()
                    .remove(&retained.alloc);
            }
            3 | 4 => {
                seal(&mut m, &mut f, 0);
                recount(&mut m, &f);
                let Val::Ptr(control) = m.read_use(&f, ValueId(3)).unwrap() else {
                    panic!()
                };
                if which == 3 {
                    m.ledger.get_mut(control.alloc).generation += 1;
                } else {
                    // The sender still owns its SSA credit when the receiving
                    // task acquires another. The ordinary holder recount must
                    // reject this even if the control's count is unchanged.
                    let mut receiver = m.frame(0, Role::Entry, None);
                    m.define_fresh(&mut receiver, ValueId(3), Val::Ptr(control))
                        .unwrap();
                    m.tasks = Some(Box::new(super::super::tasks::sealed_owner_test_task(
                        receiver,
                    )));
                }
            }
            _ => unreachable!(),
        }
        let bad = if which == 3 || which == 4 {
            m.stack.push(Activity::Frame(Box::new(f.clone())));
            m.steps += 1;
            m.check_counts().unwrap_err()
        } else {
            m.stack.push(Activity::Frame(Box::new(f.clone())));
            m.step().unwrap_err()
        };
        assert_ne!(bad.kind, V::InternalAudit, "{bad:?}");
        assert_ne!(bad.kind, V::Unsupported, "{bad:?}");
        if which < 3 || which == 5 {
            assert_eq!(bad.site.as_ref().unwrap().statement, 0);
        }
        violations.push(bad);
        m.tasks = None;
        m.stack.clear();
        m.ledger = clean_ledger;
        m.seal_inventory = clean_inventory;
        m.holders = clean_holders;
        m.physical = clean_physical;
        f = clean_frame;
        seal(&mut m, &mut f, shape);
        recount(&mut m, &f);
        finish(&mut m, &mut f, g, true);
    }
    assert_eq!(violations[0], violations[1]);
}

#[test]
fn seal_inventory_92_uncredited_shared_root_has_an_accepted_control() {
    corruption(0);
}
#[test]
fn seal_inventory_92_untracked_outgoing_heap_cell_has_an_accepted_control() {
    corruption(1);
}
#[test]
fn seal_inventory_92_external_live_view_has_an_accepted_control() {
    corruption(2);
}
#[test]
fn seal_inventory_92_stale_owner_generation_has_an_accepted_control() {
    corruption(3);
}
#[test]
fn seal_inventory_92_two_task_owners_have_an_accepted_control() {
    corruption(4);
}
#[test]
fn seal_inventory_92_missing_retained_journal_has_an_accepted_control() {
    corruption(5);
}

#[test]
fn seal_inventory_92_static_correspondence_rejects_erased_event_pairs_before_execution() {
    let mut x = fixture();
    let c = config(&mut x.p, 0, &[1], false);
    let code = [
        seal_inst(0),
        Inst::ArenaInventory {
            receipt: hirsel_lir::ir::OptionalValueId::NONE,
            arena: ValueId(5),
            allocation: SourceAnchorId(1),
            value: ValueId(6),
            shape: SealDescriptorId(1),
            previous: None,
        },
    ];
    x.p.units[0].blocks[0].instructions = PoolRange::push(&mut x.p.units[0].instructions, code);
    let mut ops = NoOperations;
    Machine::new(&x.p, &c, &mut ops)
        .check_seal_sources()
        .unwrap();
    let original = x.p.clone();
    for mutation in 0..4 {
        x.p = original.clone();
        match mutation {
            0 => {
                x.p.units[0].blocks[0].instructions.len = 1;
            }
            1 => {
                x.p.units[0].blocks[0].instructions.len = 0;
            }
            2 => {
                x.p.units[0].header.implementation_revision =
                    hirsel_syntax::ContentHash::of_bytes(b"wrong revision");
            }
            3 => {
                x.p.units[0].blocks[0].instructions =
                    PoolRange::push(&mut x.p.units[0].instructions, [seal_inst(0), seal_inst(0)]);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            Machine::new(&x.p, &c, &mut ops)
                .check_seal_sources()
                .unwrap_err()
                .kind,
            V::Malformed
        );
    }
}

#[test]
fn seal_inventory_92_uncalled_body_inventory_does_not_belong_to_an_empty_scope() {
    let mut x = fixture();
    let mut c = config(&mut x.p, 0, &[], false);
    x.p.units[0].blocks[0].instructions =
        PoolRange::push(&mut x.p.units[0].instructions, [seal_inst(0)]);
    let mut unused = x.p.units[0].clone();
    unused.header.definition = crate::build::path("uncalled_inventory");
    unused.tables.source_anchors[0].statement = 77;
    let event = SourceEvent {
        anchor: unused.tables.source_anchors[0].clone(),
        kind: SourceEventKind::Allocate,
        root_type: unused.tables.seal_descriptors[1].root_type.clone(),
    };
    unused.blocks[0].instructions = PoolRange::push(
        &mut unused.instructions,
        [Inst::ArenaInventory {
            receipt: hirsel_lir::ir::OptionalValueId::NONE,
            arena: ValueId(5),
            allocation: SourceAnchorId(0),
            value: ValueId(6),
            shape: SealDescriptorId(1),
            previous: None,
        }],
    );
    c.seal_sources.push(SourceSeal {
        environments: None,
        reference: hirsel_lir::invocations::ReferenceBody {
            definition: unused.header.definition.clone(),
            instance: unused.header.instance,
            body: 0,
        },
        witness: SealSourceWitness::from_expectations(
            unused.header.implementation_revision,
            unused.header.definition.clone(),
            unused.header.implementation_revision,
            unused.header.instance,
            &[event.clone()],
            &[],
        ),
        events: vec![event],
        payloads: vec![],
    });
    x.p.units.push(unused);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    m.check_seal_sources().unwrap();
    m.seal_sources_checked = true;
    let (mut f, g) = prepare(&mut m, 0);
    seal(&mut m, &mut f, 0);
    recount(&mut m, &f);
    finish(&mut m, &mut f, g, true);
}
