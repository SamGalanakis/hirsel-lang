//! Actual H scope-exit, write and terminal-audit seams. These component tests
//! do not admit SealArena or construct independent source/SSA authority.
use super::*;
use crate::build::{Layouts, fixture_unit, path};
use crate::ledger::{Region, sealed::StorageCustody};
use crate::program::NoOperations;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::{Ready, StageKind};

fn program() -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let scalar = layouts.scalar(ScalarKind::U64);
    assert_eq!(scalar, LayoutId(0));
    let (target, layouts) = layouts.finish();
    let mut unit = fixture_unit::<Ready>(path("sealed_scope_exit"));
    unit.header.target = target.digest();
    unit.header.stage = StageKind::Ready;
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

fn open_scope(m: &mut Machine<'_, Ready>, frame: &mut Frame) -> Ptr {
    m.ledger.regions.push(Region {
        storage: StorageCustody::Scope,
        instance: RegionInstanceId(0),
        live: true,
        allocations: Default::default(),
        budget: None,
    });
    let data = m
        .ledger
        .alloc(Owner::Region(0), Some(LayoutId(0)), 8, 8, 8, None);
    m.ledger.regions[0].allocations.push(data.alloc);
    m.ledger
        .get_mut(data.alloc)
        .extent
        .write_bytes(0, &73u64.to_le_bytes());
    frame.regions.push(0);
    data
}

#[test]
fn sealed_scope_exit_preserves_adopted_storage_and_read_only_access_under_both_audits() {
    let program = program();
    for full in [false, true] {
        for adopted in [false, true] {
            let config = Config {
                full_recount_every_step: full,
                ..Config::default()
            };
            let mut ops = NoOperations;
            let mut m = Machine::new(&program, &config, &mut ops);
            let mut frame = m.frame(0, Role::Entry, None);
            let data = open_scope(&mut m, &mut frame);
            let owner = adopted.then(|| m.ledger.transfer_closed_scope(0, m.word).unwrap());
            m.steps = 1;
            m.check_counts().unwrap();
            m.exit_region(&mut frame, 0).unwrap();
            assert!(!m.ledger.regions[0].live);
            assert!(frame.regions.is_empty());
            m.steps = 2;
            m.check_counts().unwrap();
            if let Some(owner) = owner {
                let storage = m.ledger.check(data).unwrap();
                assert_eq!(storage.extent.read_bytes(0, 8), &73u64.to_le_bytes());
                assert!(storage.published);
                let place = Resolved {
                    columns: None,
                    ptr: data,
                    layout: LayoutId(0),
                    path: vec![],
                    root_slot: None,
                    flat_content: None,
                    flat_end: None,
                };
                assert_eq!(
                    m.check_writable(&place).unwrap_err().kind,
                    V::WritePublished
                );
                assert_eq!(
                    m.ledger.check(data).unwrap().extent.read_bytes(0, 8),
                    &73u64.to_le_bytes()
                );
                assert_eq!(m.audit().unwrap_err().kind, V::OpenResource);
                let loan = m.ledger.begin_sealed_loan(owner).unwrap();
                assert_eq!(
                    m.ledger.begin_sealed_drop(owner).unwrap_err().kind,
                    V::LoanOutlivesFrame
                );
                m.ledger.end_sealed_loan(owner, loan).unwrap();
                for allocation in m.ledger.begin_sealed_drop(owner).unwrap() {
                    m.ledger.free(allocation).unwrap();
                }
                m.ledger.finish_sealed_drop(owner).unwrap();
            } else {
                assert_eq!(m.ledger.get(data.alloc).state, AllocState::Dead);
                assert_eq!(m.ledger.check(data).unwrap_err().kind, V::RegionEscape);
            }
            m.steps = 3;
            m.check_counts().unwrap();
            m.audit().unwrap();
        }
    }
}

#[test]
fn sealed_scope_exit_custody_corruption_is_caught_before_storage_is_reclaimed() {
    let program = program();
    for full in [false, true] {
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let mut frame = m.frame(0, Role::Entry, None);
        let data = open_scope(&mut m, &mut frame);
        m.ledger.transfer_closed_scope(0, m.word).unwrap();
        m.steps = 1;
        m.check_counts().unwrap();
        m.ledger.regions[0].storage = StorageCustody::Scope;
        m.steps = 2;
        assert_eq!(m.check_counts().unwrap_err().kind, V::RegionEscape);
        assert_eq!(m.ledger.get(data.alloc).state, AllocState::Live);
        assert!(m.ledger.regions[0].live);
        assert_eq!(frame.regions, vec![0]);
    }
}
