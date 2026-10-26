//! ReleaseSealed dispatch over the same typed owner and actual frame place.
use super::sealed_read_tests::{prepared, program};
use super::*;
use crate::NoOperations;

#[test]
fn sealed_drop_dispatch_retires_bit_owner_once_after_its_read_loan() {
    for full in [false, true] {
        let program = program(true);
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let (mut f, control, _) = prepared(&mut m, true);
        let slot = f.loans[0].unwrap();
        m.exec(
            &mut f,
            &Inst::BorrowSealedRoot {
                result: ValueId(1),
                owner: ValueId(0),
                loan: LoanId(0),
                shape: SealDescriptorId(0),
            },
        )
        .unwrap();
        m.exec(
            &mut f,
            &Inst::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        )
        .unwrap();
        let mut drain = Drain {
            caller: None,
            operation: None,
            work: vec![Work::Step {
                base: slot,
                logical_base: slot,
                layout: LayoutId(1),
                step: DropStep::ReleaseSealed { offset: 0 },
                dead: vec![],
                child_mask: None,
            }],
            failed: false,
        };
        while let Some(work) = drain.work.pop() {
            assert!(
                m.step_drain(&mut drain, work)
                    .expect("sealed release dispatch")
                    .is_none()
            );
            m.steps += 1;
            m.check_counts().unwrap();
        }
        assert_eq!(m.ledger.get(control.alloc).state, AllocState::Dead);
        assert!(!m.ledger.check(slot).unwrap().extent.any_init(0, 8));
        m.ledger.free(slot.alloc).unwrap();
        m.audit().unwrap();
    }
}

fn drain_all(m: &mut Machine<'_, hirsel_lir::stage::Rc>, mut work: Vec<Work>) {
    let mut drain = Drain {
        caller: None,
        operation: None,
        work: vec![],
        failed: false,
    };
    drain.work.append(&mut work);
    while let Some(work) = drain.work.pop() {
        assert!(m.step_drain(&mut drain, work).unwrap().is_none());
        m.steps += 1;
        m.check_counts().unwrap();
    }
}

#[test]
fn sealed_drop_dispatch_refuses_a_live_root_loan_without_consuming_the_owner() {
    let program = program(true);
    let config = Config::default();
    let mut ops = NoOperations;
    let mut m = Machine::new(&program, &config, &mut ops);
    let (mut f, control, _) = prepared(&mut m, true);
    let slot = f.loans[0].unwrap();
    m.exec(
        &mut f,
        &Inst::BorrowSealedRoot {
            result: ValueId(1),
            owner: ValueId(0),
            loan: LoanId(0),
            shape: SealDescriptorId(0),
        },
    )
    .unwrap();
    let mut work = Vec::new();
    assert_eq!(
        m.release_sealed(slot, LayoutId(1), &mut work)
            .unwrap_err()
            .kind,
        V::LoanOutlivesFrame
    );
    assert!(work.is_empty());
    assert_eq!(m.read_ptr(slot).unwrap(), control);
    assert_eq!(
        m.ledger.check(control).unwrap().count,
        Some(Count::Finite(1))
    );
    m.exec(
        &mut f,
        &Inst::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    )
    .unwrap();
    m.release_sealed(slot, LayoutId(1), &mut work).unwrap();
    drain_all(&mut m, work);
    assert_eq!(
        m.release_sealed(slot, LayoutId(1), &mut Vec::new())
            .unwrap_err()
            .kind,
        V::UninitializedRead
    );
    m.ledger.free(slot.alloc).unwrap();
    m.audit().unwrap();
}

#[test]
fn sealed_drop_dispatch_releases_repeated_shared_cells_before_freeing_any_adopted_storage() {
    for full in [false, true] {
        let mut program = program(true);
        let mut row = program.layouts.layouts[0].clone();
        row.kind = LayoutKind::Aggregate;
        row.size = SizeSpec::Fixed(24);
        program.layouts.layouts.push(row);
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        m.ledger.regions.push(crate::ledger::Region {
            storage: crate::ledger::sealed::StorageCustody::Scope,
            instance: RegionInstanceId(0),
            live: true,
            allocations: Default::default(),
            budget: None,
        });
        // Allocate the child before its parent: freeing in allocation order
        // requires clearing the later parent's uncounted edge first.
        let child = m
            .ledger
            .alloc(Owner::Region(0), Some(LayoutId(0)), 8, 8, 8, None);
        let parent = m
            .ledger
            .alloc(Owner::Region(0), Some(LayoutId(2)), 24, 24, 8, None);
        let shared = m.ledger.alloc(
            Owner::Heap,
            Some(LayoutId(0)),
            8,
            8,
            8,
            Some(Count::Root(2)),
        );
        for p in [child, shared] {
            m.ledger
                .get_mut(p.alloc)
                .extent
                .write_bytes(0, &73u64.to_le_bytes());
        }
        m.ledger.regions[0].allocations.push(child.alloc);
        m.ledger.regions[0].allocations.push(parent.alloc);
        for (offset, ptr, counted) in [(0, child, false), (8, shared, true), (16, shared, true)] {
            m.ledger.get_mut(parent.alloc).extent.put_cell(
                offset,
                8,
                Cell::Data {
                    ptr,
                    counted,
                    borrow: None,
                },
            );
        }
        let mut root = Extent::new(8);
        root.write_bytes(0, &7u64.to_le_bytes());
        let owner = m
            .ledger
            .transfer_closed_scope_with_bit_root(0, 8, &program.layouts, LayoutId(0), root)
            .unwrap();
        let control = m.ledger.alloc(
            Owner::Heap,
            Some(LayoutId(1)),
            8,
            8,
            8,
            Some(Count::Finite(1)),
        );
        m.ledger.bind_sealed_control(control, owner).unwrap();
        m.ledger.regions[0].live = false;
        let slot = m
            .ledger
            .alloc(Owner::Frame, Some(LayoutId(1)), 8, 8, 8, None);
        m.write_value(
            slot,
            LayoutId(1),
            Moved {
                val: Val::Ptr(control),
                credited: true,
                borrow: None,
            },
        )
        .unwrap();
        let mut work = Vec::new();
        m.release_sealed(slot, LayoutId(1), &mut work).unwrap();
        drain_all(&mut m, work);
        for p in [child, parent, shared, control] {
            assert_eq!(m.ledger.get(p.alloc).state, AllocState::Dead);
        }
        assert_eq!(
            m.ledger.counters.releases, 3,
            "one owner control plus two Shared occurrences"
        );
        m.ledger.free(slot.alloc).unwrap();
        m.audit().unwrap();
    }
}

#[test]
fn sealed_owner_nested_in_an_inline_record_uses_its_field_layout_at_drop() {
    use hirsel_lir::layout_types::FieldLayout;
    let mut program = program(true);
    let mut row = program.layouts.layouts[1].clone();
    row.kind = LayoutKind::Aggregate;
    row.size = SizeSpec::Fixed(16);
    row.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: LayoutId(0),
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 8,
            layout: LayoutId(1),
            unaligned: false,
        },
    ];
    row.drop = vec![DropStep::ReleaseSealed { offset: 8 }];
    program.layouts.layouts.push(row);
    let config = Config {
        full_recount_every_step: true,
        ..Config::default()
    };
    let mut ops = NoOperations;
    let mut m = Machine::new(&program, &config, &mut ops);
    let (mut f, control, _) = prepared(&mut m, true);
    let old = f.loans[0].unwrap();
    m.exec(
        &mut f,
        &Inst::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    )
    .unwrap();
    let _moved = m.take_part(old, 8, &[]);
    m.ledger.free(old.alloc).unwrap();
    let record = m
        .ledger
        .alloc(Owner::Frame, Some(LayoutId(2)), 16, 16, 8, None);
    m.ledger
        .get_mut(record.alloc)
        .extent
        .write_bytes(0, &9u64.to_le_bytes());
    m.write_value(
        record.at(8),
        LayoutId(1),
        Moved {
            val: Val::Ptr(control),
            credited: true,
            borrow: None,
        },
    )
    .unwrap();
    drain_all(
        &mut m,
        vec![Work::DropAt {
            at: record,
            layout: LayoutId(2),
            mask: None,
        }],
    );
    assert_eq!(m.ledger.get(control.alloc).state, AllocState::Dead);
    m.ledger.free(record.alloc).unwrap();
    m.audit().unwrap();
}
