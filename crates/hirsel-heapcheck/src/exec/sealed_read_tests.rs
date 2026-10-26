//! Direct instruction-adapter controls over H-owned root bytes and real loans.
//! Source correspondence and public seal production remain separate boundaries.
use super::*;
use crate::NoOperations;
use crate::build::{Layouts, fixture_unit, path};
use crate::ledger::{Region, sealed::StorageCustody};
use hirsel_lir::TargetSpec;
use hirsel_lir::logical_storage::{LogicalBytes, LogicalStorageGraph, StorageClass, StorageFact};
use hirsel_lir::operation::NativeType;
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_lir::stage::Rc;
use hirsel_lir::structural::{Shape, ShapeGraph};
use hirsel_lir::structural_access::AccessPlan;

pub(super) fn program(scalar: bool) -> Program<Rc> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let root = if scalar {
        layouts.scalar(ScalarKind::U64)
    } else {
        layouts.unit()
    };
    let owner = layouts.scalar(ScalarKind::Pointer);
    let record = layouts.get_mut(owner);
    record.kind = LayoutKind::SealedRegion { root };
    record.copy = hirsel_lir::layout_types::CopyRecipe::NotCopyable;
    record.drop = vec![DropStep::ReleaseSealed { offset: 0 }];
    let native = if scalar {
        NativeType::Primitive(hirsel_lir::operation::Primitive::U64)
    } else {
        NativeType::Tuple(vec![])
    };
    let graph = ShapeGraph {
        root: 0,
        nodes: vec![if scalar {
            Shape::Leaf
        } else {
            Shape::Fields(vec![])
        }],
    };
    let descriptor = SealDescriptor {
        root_type: native.clone(),
        root_layout: root,
        logical: LogicalStorageGraph {
            shape: graph.clone(),
            facts: vec![StorageFact {
                bytes: LogicalBytes::Bytes(if scalar { 8 } else { 0 }),
                class: StorageClass::Inline,
            }],
        },
        projection: AccessPlan::derive(native, graph, &layouts.table, root).unwrap(),
        access: vec![StorageAccess {
            layout: root,
            scan: vec![],
        }],
    };
    descriptor.check(&layouts.table).unwrap();
    let (target, layouts) = layouts.finish();
    let mut unit = fixture_unit::<Rc>(path("sealed_root_read"));
    unit.header.target = target.digest();
    unit.types = vec![
        LirType {
            repr: ValueRepr::Handle,
            layout: owner,
        },
        LirType {
            repr: if scalar {
                ValueRepr::Scalar(ScalarKind::U64)
            } else {
                ValueRepr::Unit
            },
            layout: root,
        },
    ];
    unit.values = vec![
        ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Borrowed(LoanId(0)),
        },
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        },
    ];
    unit.slots.push(SlotInfo {
        ty: LirTypeId(0),
        kind: SlotKind::Local,
        scope: ScopeId(0),
    });
    unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty: LirTypeId(0),
    });
    unit.tables.loans.push(LoanInfo {
        address: AddressId(0),
        mutable: true,
    });
    unit.tables.seal_descriptors.push(descriptor);
    Program {
        target,
        layouts,
        units: vec![unit],
        statics: vec![],
        invocation_bodies: vec![],
        pretransfer_adapters: vec![],
        invocations: Default::default(),
    }
}

pub(super) fn prepared(
    m: &mut Machine<'_, Rc>,
    scalar: bool,
) -> (Frame, Ptr, crate::ledger::sealed::SealedId) {
    let mut f = m.frame(0, Role::Entry, None);
    m.ledger.regions.push(Region {
        storage: StorageCustody::Scope,
        instance: RegionInstanceId(0),
        live: true,
        allocations: Default::default(),
        budget: None,
    });
    let mut root = Extent::new(if scalar { 8 } else { 0 });
    if scalar {
        root.write_bytes(0, &73u64.to_le_bytes());
    }
    let owner = m
        .ledger
        .transfer_closed_scope_with_bit_root(0, 8, &m.program.layouts, LayoutId(0), root)
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
    f.slots[0] = SlotStore::Own(slot);
    f.loans[0] = Some(slot);
    f.prepared[0] = Some(Located::Frame(0));
    m.define_fresh(&mut f, ValueId(0), Val::Ptr(control))
        .unwrap();
    (f, control, owner)
}

#[test]
fn sealed_root_read_instruction_uses_h_owned_bits_under_the_active_owner_loan() {
    for (scalar, full) in [(false, false), (false, true), (true, false), (true, true)] {
        let program = program(scalar);
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let (mut f, control, owner) = prepared(&mut m, scalar);
        m.steps = 1;
        m.check_counts().unwrap();
        m.exec(
            &mut f,
            &Inst::BorrowSealedRoot {
                result: ValueId(1),
                owner: ValueId(0),
                loan: LoanId(0),
                shape: SealDescriptorId(0),
            },
        )
        .expect("the linked read adapter must materialize its typed root");
        m.steps = 2;
        m.check_counts().unwrap();
        assert_eq!(
            m.ledger.begin_sealed_drop(owner).unwrap_err().kind,
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
            m.read_use(&f, ValueId(1)).unwrap(),
            if scalar {
                Val::Bits {
                    scalar: ScalarKind::U64,
                    bits: 73,
                }
            } else {
                Val::Unit
            }
        );
        for allocation in m.ledger.begin_sealed_drop(owner).unwrap() {
            m.ledger.free(allocation).unwrap();
        }
        let slot = match f.slots[0] {
            SlotStore::Own(ptr) => ptr,
            _ => unreachable!(),
        };
        let _moved = m.take_part(slot, 8, &[]);
        m.ledger.free(control.alloc).unwrap();
        m.ledger.finish_sealed_drop(owner).unwrap();
        m.ledger.audit_sealed_owners().unwrap();
        m.ledger.free(slot.alloc).unwrap();
        m.steps = 3;
        m.check_counts().unwrap();
        m.audit().unwrap();
    }
}

#[test]
fn sealed_read_instruction_refuses_inactive_or_nonexclusive_owner_loans() {
    let mut program = program(true);
    for exclusive in [false, true] {
        program.units[0].tables.loans[0].mutable = exclusive;
        let config = Config::default();
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let mut f = m.frame(0, Role::Entry, None);
        let error = m
            .exec(
                &mut f,
                &Inst::BorrowSealedRoot {
                    result: ValueId(1),
                    owner: ValueId(0),
                    loan: LoanId(0),
                    shape: SealDescriptorId(0),
                },
            )
            .err()
            .expect("owner loan must exist and be exclusive");
        assert_eq!(error.kind, V::BorrowExpired);
    }
}

#[test]
fn sealed_read_instruction_checks_actual_owner_pointer_and_generation() {
    for mutation in 0..7 {
        let program = program(true);
        let config = Config::default();
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let (mut f, control, _) = prepared(&mut m, true);
        let expected = match mutation {
            0 => {
                let other = m.ledger.alloc(
                    Owner::Heap,
                    Some(LayoutId(1)),
                    8,
                    8,
                    8,
                    Some(Count::Finite(1)),
                );
                m.define_fresh(&mut f, ValueId(0), Val::Ptr(other)).unwrap();
                V::ViewDetached
            }
            1 => {
                m.ledger.free(control.alloc).unwrap();
                V::UseAfterFree
            }
            2 => {
                m.ledger.get_mut(control.alloc).generation += 1;
                V::StaleGeneration
            }
            3 => {
                m.ledger.get_mut(control.alloc).count = Some(Count::Finite(2));
                V::CreditUnderflow
            }
            4 => {
                let other = m.ledger.alloc(
                    Owner::Heap,
                    Some(LayoutId(1)),
                    8,
                    8,
                    8,
                    Some(Count::Finite(1)),
                );
                let slot = f.loans[0].unwrap();
                m.ledger.get_mut(slot.alloc).extent.put_cell(
                    0,
                    8,
                    Cell::Data {
                        ptr: other,
                        counted: true,
                        borrow: None,
                    },
                );
                m.define_fresh(&mut f, ValueId(0), Val::Ptr(other)).unwrap();
                V::ForgedPointer
            }
            5 => {
                f.loans[0] = None;
                V::BorrowExpired
            }
            _ => {
                m.ledger.get_mut(control.alloc).layout = Some(LayoutId(0));
                V::TypeConfusion
            }
        };
        let error = m
            .exec(
                &mut f,
                &Inst::BorrowSealedRoot {
                    result: ValueId(1),
                    owner: ValueId(0),
                    loan: LoanId(0),
                    shape: SealDescriptorId(0),
                },
            )
            .err()
            .expect("mutated sealed read must be rejected");
        assert_eq!(error.kind, expected, "mutation {mutation}");
        assert!(f.values[1].is_none(), "a failed read must publish no root");
    }
}

#[test]
fn sealed_owner_transport_requires_checked_pointer_cells_and_preserves_the_credit() {
    let program = program(true);
    let config = Config::default();
    let mut ops = NoOperations;
    let mut m = Machine::new(&program, &config, &mut ops);
    let (f, control, _) = prepared(&mut m, true);
    let slot = f.loans[0].unwrap();
    let image = m.ledger.check(slot).unwrap().extent.copy(0, 8, None);
    let (val, borrow, credited) = m.val_from(image, ValueRepr::Handle, LayoutId(1)).unwrap();
    assert_eq!(val, Val::Ptr(control));
    assert!(borrow.is_none());
    assert!(credited);
    let mut forged = Extent::new(8);
    forged.write_bytes(0, &7u64.to_le_bytes());
    assert_eq!(
        m.val_from(forged, ValueRepr::Handle, LayoutId(1))
            .unwrap_err()
            .kind,
        V::ForgedPointer
    );
    let out = m
        .ledger
        .alloc(Owner::Frame, Some(LayoutId(1)), 8, 8, 8, None);
    assert_eq!(
        m.write_value(
            out,
            LayoutId(1),
            Moved {
                val: Val::Handle(7),
                credited: true,
                borrow: None,
            }
        )
        .unwrap_err()
        .kind,
        V::ForgedPointer
    );
    assert!(!m.ledger.check(out).unwrap().extent.any_init(0, 8));
}
