//! Empty-scope bit-root SealArena instruction controls. Retained inventory
//! needs its separately checked semantic event adapter before activation.
use super::sealed_read_tests::program as read_program;
use super::*;
use crate::NoOperations;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::{AbiParameter, ParameterPassing, ScalarExtension};

pub(super) fn program(scalar: bool) -> Program<hirsel_lir::stage::Rc> {
    let mut p = read_program(scalar);
    let mut arena = p.layouts.layouts[1].clone();
    arena.kind = LayoutKind::Arena;
    arena.copy = CopyRecipe::Bits;
    arena.drop.clear();
    p.layouts.layouts.push(arena);
    let unit = &mut p.units[0];
    unit.header.origin = UnitOrigin::Library;
    unit.header.definition = hirsel_syntax::DefPath::core("arena.seal");
    unit.types.push(LirType {
        repr: ValueRepr::Arena,
        layout: LayoutId(2),
    });
    unit.values.extend([
        ValueInfo {
            ty: LirTypeId(2),
            holding: Holding::Plain,
        },
        ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Owned,
        },
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain,
        },
    ]);
    unit.values.push(ValueInfo {
        ty: LirTypeId(2),
        holding: Holding::Plain,
    });
    unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Slot(SlotId(1)),
        projection: PoolRange::EMPTY,
        ty: LirTypeId(2),
    });
    unit.tables
        .arenas
        .push(ExternalInstance { slot: SlotId(1) });
    unit.parameters = vec![ValueId(2), ValueId(4)];
    unit.slots.push(SlotInfo {
        ty: LirTypeId(2),
        kind: SlotKind::Parameter(0),
        scope: ScopeId(0),
    });
    let abi = &mut p.layouts.fn_abis[unit.header.abi.0 as usize];
    abi.parameters = [LayoutId(2), LayoutId(0)]
        .into_iter()
        .enumerate()
        .map(|(i, layout)| AbiParameter {
            source: i as u32,
            convention: Passing::Take,
            passing: ParameterPassing::ByValue,
            layout,
            slots: vec![],
            extension: ScalarExtension::None,
        })
        .collect();
    p
}

#[test]
fn sealed_empty_scope_instruction_publishes_one_owner_without_charging_unit_or_scalar() {
    for (scalar, full) in [(false, false), (false, true), (true, false), (true, true)] {
        let program = program(scalar);
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let (mut f, grant_slot) = prepared(&mut m, scalar);
        m.exec(
            &mut f,
            &Inst::SealArena {
                result: ValueId(3),
                grant: ValueId(5),
                root: ValueId(4),
                shape: SealDescriptorId(0),
                failure: FailureId(0),
                origin: SourceAnchorId(0),
            },
        )
        .expect("the linked empty-scope seal must publish its typed owner");
        let moved = m.take_value(&mut f, ValueId(3)).unwrap();
        assert!(moved.credited);
        let Val::Ptr(control) = moved.val else {
            panic!("owner is a checked control pointer")
        };
        assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, 0);
        m.exit_region(&mut f, 0).unwrap();
        let owner = m.ledger.sealed_control(control).unwrap();
        let loan = m.ledger.begin_sealed_loan(owner).unwrap();
        let root = m.ledger.sealed_bit_root(owner, loan, LayoutId(0)).unwrap();
        assert_eq!(root.len(), if scalar { 8 } else { 0 });
        if scalar {
            assert_eq!(root.read_bytes(0, 8), &73u64.to_le_bytes());
        }
        m.ledger.end_sealed_loan(owner, loan).unwrap();
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
        m.steps = 3;
        m.check_counts().unwrap();
        let mut work = Vec::new();
        m.release_sealed(slot, LayoutId(1), &mut work).unwrap();
        let mut drain = Drain {
            caller: None,
            operation: None,
            work,
            failed: false,
        };
        while let Some(work) = drain.work.pop() {
            assert!(m.step_drain(&mut drain, work).unwrap().is_none());
            m.steps += 1;
            m.check_counts().unwrap();
        }
        m.ledger.free(slot.alloc).unwrap();
        m.ledger.free(grant_slot.alloc).unwrap();
        m.audit().unwrap();
    }
}

fn take_grant(m: &mut Machine<'_, hirsel_lir::stage::Rc>, f: &mut Frame) -> Checked<()> {
    m.exec(
        f,
        &Inst::Address {
            address: AddressId(1),
            failure: FailureId(0),
        },
    )?;
    m.exec(
        f,
        &Inst::Read {
            result: ValueId(5),
            address: AddressId(1),
            reaching: ValueId(2),
            mode: ReadMode::Take,
        },
    )?;
    Ok(())
}

pub(super) fn prepared(m: &mut Machine<'_, hirsel_lir::stage::Rc>, scalar: bool) -> (Frame, Ptr) {
    let mut f = m.frame(0, Role::Entry, None);
    m.ledger.regions.push(crate::ledger::Region {
        storage: crate::ledger::sealed::StorageCustody::Scope,
        instance: RegionInstanceId(0),
        live: true,
        allocations: Default::default(),
        budget: Some(crate::ledger::Budget {
            remaining: 0,
            start: 0,
            exhausted: None,
        }),
    });
    f.regions.push(0);
    let grant_slot = m
        .ledger
        .alloc(Owner::Frame, Some(LayoutId(2)), 8, 8, 8, None);
    m.ledger
        .get_mut(grant_slot.alloc)
        .extent
        .put_cell(0, 8, Cell::Arena(Some(0)));
    f.slots[1] = SlotStore::Own(grant_slot);
    m.define_fresh(&mut f, ValueId(2), Val::Version).unwrap();
    f.versions[1] = Some(ValueId(2));
    take_grant(m, &mut f).unwrap();
    m.define_fresh(
        &mut f,
        ValueId(4),
        if scalar {
            Val::Bits {
                scalar: ScalarKind::U64,
                bits: 73,
            }
        } else {
            Val::Unit
        },
    )
    .unwrap();
    (f, grant_slot)
}

fn seal() -> Inst {
    Inst::SealArena {
        result: ValueId(3),
        grant: ValueId(5),
        root: ValueId(4),
        shape: SealDescriptorId(0),
        failure: FailureId(0),
        origin: SourceAnchorId(0),
    }
}

#[test]
fn sealed_seal_refusals_publish_nothing_and_preserve_scope_guard_and_budget() {
    for mutation in 0..15 {
        let mut p = program(true);
        match mutation {
            9 => p.units[0].values[3].holding = Holding::Plain,
            10 => p.layouts.layouts[1].kind = LayoutKind::SealedRegion { root: LayoutId(2) },
            12 => p.units[0].types[1].repr = ValueRepr::Scalar(ScalarKind::Pointer),
            13 => p.layouts.fn_abis[0].parameters[0].convention = Passing::Borrow,
            _ => {}
        }
        let config = Config::default();
        let mut ops = NoOperations;
        let mut m = Machine::new(&p, &config, &mut ops);
        let (mut f, grant_slot) = prepared(&mut m, true);
        let mut instruction = seal();
        let expected = match mutation {
            0 => {
                m.define_fresh(&mut f, ValueId(5), Val::Arena(None))
                    .unwrap();
                V::TypeConfusion
            }
            1 => {
                m.define_fresh(&mut f, ValueId(5), Val::Arena(Some(999)))
                    .unwrap();
                V::ForgedPointer
            }
            2 => {
                m.ledger.regions[0].live = false;
                V::LiveRegion
            }
            3 => {
                f.values[5].as_mut().unwrap().borrow = Some(Borrow::Caller);
                V::BorrowedStore
            }
            4 => {
                f.values[4].as_mut().unwrap().borrow = Some(Borrow::Caller);
                V::BorrowedStore
            }
            5 => {
                f.loans[0] = Some(grant_slot);
                V::BorrowExpired
            }
            6 | 7 => {
                let data = m
                    .ledger
                    .alloc(Owner::Region(0), Some(LayoutId(0)), 8, 8, 8, None);
                if mutation == 6 {
                    m.ledger.regions[0].allocations.push(data.alloc);
                }
                V::Unsupported
            }
            8 => {
                m.define_fresh(
                    &mut f,
                    ValueId(4),
                    Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: 1,
                    },
                )
                .unwrap();
                V::TypeConfusion
            }
            9 | 10 => V::TypeConfusion,
            11 => {
                if let Inst::SealArena { shape, .. } = &mut instruction {
                    *shape = SealDescriptorId(999);
                }
                V::Malformed
            }
            12 => V::Unsupported,
            13 => V::ForgedPointer,
            14 => {
                f.seal_grants.clear();
                V::ForgedPointer
            }
            _ => unreachable!(),
        };
        let allocations = m.ledger.allocations.len();
        let error = m
            .exec(&mut f, &instruction)
            .err()
            .expect("invalid seal must refuse");
        assert_eq!(error.kind, expected, "mutation {mutation}: {error:?}");
        assert!(
            f.values[3].is_none(),
            "mutation {mutation} published an owner"
        );
        assert_eq!(m.ledger.allocations.len(), allocations);
        assert_eq!(
            m.ledger.regions[0].storage,
            crate::ledger::sealed::StorageCustody::Scope
        );
        assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, 0);
        m.ledger.regions[0].live = true;
        m.exit_region(&mut f, 0)
            .expect("original guard can close after refusal");
        // The unlisted-allocation mutation deliberately corrupts membership;
        // it was refused independently of the candidate scope list.
        if mutation == 7 {
            for index in 0..m.ledger.allocations.len() {
                let id = AllocId(index as u32);
                if m.ledger.get(id).owner == Owner::Region(0)
                    && m.ledger.get(id).state == AllocState::Live
                {
                    m.ledger.free(id).unwrap();
                }
            }
        }
        m.ledger.free(grant_slot.alloc).unwrap();
        m.audit().unwrap();
    }
}

#[test]
fn sealed_seal_consumption_blocks_second_publication_and_allocator_aliases() {
    let p = program(true);
    let config = Config::default();
    let mut ops = NoOperations;
    let mut m = Machine::new(&p, &config, &mut ops);
    let (mut f, grant_slot) = prepared(&mut m, true);
    // A distinct caller/guard alias remains readable for recovery, but cannot
    // allocate once custody transferred. It is not another consumable grant.
    let alias_slot = m
        .ledger
        .alloc(Owner::Frame, Some(LayoutId(2)), 8, 8, 8, None);
    m.ledger
        .get_mut(alias_slot.alloc)
        .extent
        .put_cell(0, 8, Cell::Arena(Some(0)));
    let mut caller = m.frame(0, Role::Callee, None);
    caller.slots[1] = SlotStore::Alias(alias_slot);
    assert_eq!(m.arena_of(&caller, ArenaInstanceId(0)).unwrap(), Some(0));
    m.exec(&mut f, &seal()).unwrap();
    let owner = m.take_value(&mut f, ValueId(3)).unwrap();
    f.values[3] = None;
    let allocations = m.ledger.allocations.len();
    let error = m
        .exec(&mut f, &seal())
        .err()
        .expect("second seal must refuse");
    assert_eq!(error.kind, V::CreditUnderflow);
    assert!(f.values[3].is_none());
    assert_eq!(m.ledger.allocations.len(), allocations);
    assert_eq!(
        m.arena_of(&caller, ArenaInstanceId(0)).unwrap_err().kind,
        V::CreditUnderflow
    );
    let e = m.ledger.check(alias_slot).unwrap().extent.copy(0, 8, None);
    assert!(matches!(
        m.val_from(e, ValueRepr::Arena, LayoutId(2)).unwrap().0,
        Val::Arena(Some(0))
    ));
    assert_eq!(m.ledger.regions[0].budget.unwrap().remaining, 0);
    m.exit_region(&mut f, 0).unwrap();
    let slot = m
        .ledger
        .alloc(Owner::Frame, Some(LayoutId(1)), 8, 8, 8, None);
    m.write_value(slot, LayoutId(1), owner).unwrap();
    let mut work = vec![];
    m.release_sealed(slot, LayoutId(1), &mut work).unwrap();
    let mut drain = Drain {
        caller: None,
        operation: None,
        work,
        failed: false,
    };
    while let Some(work) = drain.work.pop() {
        assert!(m.step_drain(&mut drain, work).unwrap().is_none());
    }
    for ptr in [slot, grant_slot, alias_slot] {
        m.ledger.free(ptr.alloc).unwrap();
    }
    m.audit().unwrap();
}
