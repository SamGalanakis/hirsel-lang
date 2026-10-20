//! Placement over real driver-derived programs: destinations become storage
//! owners, frame-local blocks move into the unit's region, a `noalloc`
//! function never reaches the heap, and the region rules the validator owns
//! reject every mutation of a placed region.
use crate::rc_support as support;

use hirsel_heapcheck::{Config, NoOperations};
use hirsel_lir::ir::{Inst, LirUnit, Placement, StorageOwner, Terminator, ValueRepr};
use hirsel_lir::stage::{Placed, Rc, Stage};
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceId, Program};
use hirsel_rc::placement::{Options, PlaceError};
use support::*;

const P: &str = "type P { a: u64, b: u64 }\n";

struct Placed_ {
    program: Program,
    table: hirsel_lir::layout_types::LayoutTable,
    units: Vec<LirUnit<Placed>>,
    proofs: Vec<Proofs>,
}

fn place(source: &str, regions: bool) -> Result<Placed_, PlaceError> {
    let (program, lowered) = lower_valid(&format!("{P}{source}"));
    let mut units = Vec::new();
    let mut proofs = Vec::new();
    for (i, u) in lowered.units.into_iter().enumerate() {
        let id = InstanceId(i as u32);
        let options = Options::for_instance(regions, &lowered.instances, id);
        units.push(hirsel_rc::placement::place(u, &lowered.table, &options)?);
        proofs.push(Proofs::for_instance(&lowered.instances, id));
    }
    Ok(Placed_ {
        program,
        table: lowered.table,
        units,
        proofs,
    })
}

fn check<S: Stage>(p: &Placed_, unit: &LirUnit<S>) -> Vec<ViolationKind> {
    let manifest = manifest(&p.program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &p.program.operation_registry(),
        target: &target,
        layouts: &p.table,
        manifest: &manifest,
    };
    hirsel_lir::validate::check(unit, &inputs)
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

fn named<'a>(p: &'a Placed_, name: &str) -> &'a LirUnit<Placed> {
    p.units
        .iter()
        .find(|u| u.header.definition.0.name == name)
        .unwrap_or_else(|| panic!("no unit {name}"))
}

/// Insert and run in the heap checker; the audit must be clean.
fn run(p: &Placed_) -> hirsel_heapcheck::Run {
    let units: Vec<LirUnit<Rc>> = p
        .units
        .iter()
        .zip(&p.proofs)
        .map(|(u, proofs)| {
            hirsel_rc::insert::insert(u.clone(), &p.table, proofs).expect("insertion")
        })
        .collect();
    for u in &units {
        assert_eq!(check(p, u), Vec::new(), "{}", dump(u));
    }
    let program = hirsel_heapcheck::Program {
        target: target(),
        layouts: p.table.clone(),
        units,
        statics: Vec::new(),
    };
    let run = hirsel_heapcheck::run(
        &program,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    run
}

fn placements<S>(unit: &LirUnit<S>) -> Vec<Placement> {
    unit.tables
        .constructs
        .iter()
        .filter_map(|c| c.placement)
        .collect()
}

fn region(p: Placement) -> bool {
    matches!(p, Placement::Fixed(StorageOwner::LocalRegion(_)))
}

const LOCAL: &str = "fn main() -> u64 { let t: box P = box P(a: 1, b: 2); let u = copy t; 3 }";

#[test]
fn a_frame_local_block_lives_in_the_region() {
    let p = place(LOCAL, true).unwrap();
    let main = named(&p, "main");
    assert_eq!(check(&p, main), Vec::new());
    assert!(placements(main).into_iter().any(region));
    let run = run(&p);
    assert_eq!(run.counters.allocations, 0);
}

#[test]
fn regions_off_every_local_block_is_a_counted_heap_block() {
    let p = place(LOCAL, false).unwrap();
    let main = named(&p, "main");
    assert_eq!(check(&p, main), Vec::new());
    assert!(!placements(main).into_iter().any(region));
    assert!(main.tables.regions.is_empty());
    let run = run(&p);
    assert_eq!(run.counters.allocations, 1);
}

/// A block that leaves the frame stays on the heap with regions on; one
/// that does not, in the same unit, still moves.
#[test]
fn an_escaping_block_stays_on_the_heap() {
    let source = "fn mk(n: u64) -> box P { let k: box P = box P(a: n, b: n); let c = copy k; box P(a: n, b: 1) }\n\
                  fn main() -> u64 { let t = mk(2); 3 }";
    let p = place(source, true).unwrap();
    let mk = named(&p, "mk");
    assert_eq!(check(&p, mk), Vec::new());
    let owners = placements(mk);
    assert!(owners.iter().copied().any(region));
    assert!(
        owners
            .iter()
            .any(|o| matches!(o, Placement::Fixed(StorageOwner::Heap(_))))
    );
    let run = run(&p);
    assert_eq!(run.counters.allocations, 1);
}

/// A call into a callee with a hidden destination names one explicitly.
#[test]
fn every_call_destination_is_explicit() {
    let source = "fn mk(n: u64) -> box P { box P(a: n, b: 1) }\n\
                  fn main() -> u64 { let t = mk(2); 3 }";
    let p = place(source, true).unwrap();
    for u in &p.units {
        assert!(
            u.tables
                .calls
                .iter()
                .all(|c| c.destination != Some(hirsel_lir::ir::DestinationClass::Inferred))
        );
        assert_eq!(check(&p, u), Vec::new());
    }
}

const NOALLOC: &str = "@noalloc\n\
     fn work(n: u64) -> u64 { let t: box P = box P(a: n, b: 2); let u = copy t; n + 1 }\n\
     fn main() -> u64 { work(2) }";

/// MEM-5: region placement does not count as allocation, so a source
/// `noalloc` function keeps its region even with regions off, and never
/// acquires a heap block.
#[test]
fn a_noalloc_function_never_acquires_a_heap_allocation() {
    for regions in [true, false] {
        let p = place(NOALLOC, regions).unwrap();
        let work = named(&p, "work");
        assert!(
            placements(work).into_iter().any(region),
            "regions {regions}"
        );
        let run = run(&p);
        assert_eq!(run.counters.allocations, 0, "regions {regions}");
    }
}

/// The accepted twin of every mutation below.
fn region_unit() -> (Placed_, LirUnit<Placed>) {
    let p = place(LOCAL, true).unwrap();
    let main = named(&p, "main").clone();
    assert_eq!(check(&p, &main), Vec::new());
    (p, main)
}

fn find(unit: &LirUnit<Placed>, f: impl Fn(&Inst) -> bool) -> usize {
    unit.instructions
        .iter()
        .position(f)
        .expect("the instruction")
}

const ESCAPE: &str = "fn mk(n: u64) -> box P { let k: box P = box P(a: n, b: n); let c = copy k; box P(a: n, b: 1) }\n\
                      fn main() -> u64 { let t = mk(2); 3 }";

/// A region value that reaches a return escapes its region; the twin returns
/// the heap block it was built beside.
#[test]
fn a_region_value_returned_is_an_escape() {
    let p = place(ESCAPE, true).unwrap();
    let mut mk = named(&p, "mk").clone();
    assert_eq!(check(&p, &mk), Vec::new());
    let at = find(&mk, |i| matches!(i, Inst::RegionPublish { .. }));
    let Inst::RegionPublish { value } = mk.instructions[at] else {
        unreachable!()
    };
    let ret = mk
        .terminators
        .iter()
        .position(|t| matches!(t, Terminator::Return(_)))
        .unwrap();
    mk.terminators[ret] = Terminator::Return(Some(value));
    let kinds = check(&p, &mk);
    assert!(
        kinds.contains(&ViolationKind::RegionEscape(value)),
        "{kinds:?}"
    );
}

/// Publishing a counted heap block, not a region block, is rejected.
#[test]
fn publishing_a_counted_block_is_rejected() {
    let p = place(LOCAL, false).unwrap();
    let mut main = named(&p, "main").clone();
    assert_eq!(check(&p, &main), Vec::new());
    let at = find(
        &main,
        |i| matches!(i, Inst::Construct { result, .. } if main.types[main.values[result.index()].ty.index()].repr == ValueRepr::Block),
    );
    let Inst::Construct { result, .. } = main.instructions[at] else {
        unreachable!()
    };
    main.instructions[at + 1] = Inst::RegionPublish { value: result };
    let kinds = check(&p, &main);
    assert!(
        kinds.contains(&ViolationKind::Type("region publication")),
        "{kinds:?}"
    );
}

/// Delete one instruction, keeping every block's range.
fn remove(unit: &mut LirUnit<Placed>, at: usize) {
    unit.instructions.remove(at);
    let at = at as u32;
    for b in &mut unit.blocks {
        let r = &mut b.instructions;
        if r.start > at {
            r.start -= 1;
        } else if at < r.start + r.len {
            r.len -= 1;
        }
    }
}

/// A region block built in a region never entered.
#[test]
fn a_construction_outside_its_region_is_rejected() {
    let (p, mut main) = region_unit();
    let enter = find(&main, |i| matches!(i, Inst::RegionEnter { .. }));
    let Inst::RegionEnter { region, .. } = main.instructions[enter] else {
        unreachable!()
    };
    remove(&mut main, enter);
    let kinds = check(&p, &main);
    assert!(
        kinds.contains(&ViolationKind::RegionState(region)),
        "{kinds:?}"
    );
}

/// A return path that never exits leaves the region entered at frame exit.
#[test]
fn a_return_without_its_region_exit_is_rejected() {
    let (p, mut main) = region_unit();
    let last = main
        .instructions
        .iter()
        .rposition(|i| matches!(i, Inst::RegionExit { .. }))
        .unwrap();
    let Inst::RegionExit { region, .. } = main.instructions[last] else {
        unreachable!()
    };
    // The exit on the normal path: the one whose block returns.
    let block = main
        .blocks
        .iter()
        .enumerate()
        .filter(|(b, _)| matches!(main.terminators[*b], Terminator::Return(_)))
        .flat_map(|(_, blk)| blk.instructions.start..blk.instructions.start + blk.instructions.len)
        .map(|i| i as usize)
        .find(|&i| matches!(main.instructions[i], Inst::RegionExit { .. }))
        .unwrap_or(last);
    // Keep the exit's failure edge named, by an operation that does nothing
    // to the region.
    let Inst::RegionExit { failure, .. } = main.instructions[block] else {
        unreachable!()
    };
    main.instructions[block] = Inst::FrameEnter { failure };
    let kinds = check(&p, &main);
    assert!(
        kinds.contains(&ViolationKind::RegionState(region)),
        "{kinds:?}"
    );
}
#[test]
fn returned_temporary_content_keeps_region_credits_consistent() {
    let source =
        include_str!("../../../../conformance/MEM/MEM-10-returned-temporary-content-run.hir");
    for regions in [false, true] {
        let p = place(source, regions).unwrap();
        for unit in &p.units {
            assert_eq!(check(&p, unit), Vec::new(), "{}", dump(unit));
        }
    }
}

#[test]
fn a_failing_return_construction_keeps_the_region_active_until_preparation_finishes() {
    use hirsel_heapcheck::build::{CLEANUP, Layouts, fixture_unit, path};
    use hirsel_lir::abi_types::AbiResult;
    use hirsel_lir::ir::*;
    use hirsel_lir::stage::Value;
    use hirsel_lir::testing::UnitBuilder;
    let mut layouts = Layouts::new(target());
    layouts.scalar(hirsel_lir::layout_types::ScalarKind::Bool);
    let number = layouts.scalar(hirsel_lir::layout_types::ScalarKind::U64);
    let empty = layouts.record(&[number]);
    let block_layout = layouts.block(empty);
    layouts.table.fn_abis[0].result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: block_layout,
        slots: vec![hirsel_lir::abi::slot(
            &layouts.target,
            hirsel_lir::layout_types::ScalarKind::Pointer,
        )],
    });
    let mut abi = layouts.table.fn_abis[0].clone();
    abi.result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: number,
        slots: vec![hirsel_lir::abi::slot(
            &layouts.target,
            hirsel_lir::layout_types::ScalarKind::Pointer,
        )],
    });
    layouts.table.fn_abis.push(abi);
    let (_, table) = layouts.finish();
    let mut builder = UnitBuilder::<Value>::new(fixture_unit(path("region_return")));
    let entry = builder.block(BlockKind::Body, &[]);
    let body = builder.block(BlockKind::Body, &[]);
    let failed = builder.block(BlockKind::FailureCleanup, &[]);
    let end = builder.block(BlockKind::FailureCleanup, &[]);
    builder.term(end, Terminator::Propagate);
    let end_edge = builder.edge(end, &[]);
    let empty_failure = builder.failure(end_edge, None, &[]);
    let cleanup = builder.edge(failed, &[]);
    let failure = builder.failure(cleanup, None, &[]);
    let cleanup = builder.edge(failed, &[]);
    let return_failure = builder.failure(cleanup, None, &[]);
    let end_edge = builder.edge(end, &[]);
    let drop_failure = builder.failure(end_edge, None, &[]);
    let slot = builder.slot(ValueRepr::Block, block_layout, SlotKind::Local);
    let address = builder.address(AddressBase::Slot(slot), &[], ValueRepr::Block, block_layout);
    builder.push(
        entry,
        Inst::Address {
            address,
            failure: empty_failure,
        },
    );
    let into = builder.edge(body, &[]);
    builder.term(entry, Terminator::Jump(into));
    let operand = builder.plain(
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        number,
    );
    let literal = builder.constant(
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        number,
        ConstantValue::Integer(7),
    );
    builder.push(
        body,
        Inst::Const {
            result: operand,
            constant: literal,
        },
    );
    let local = builder.owned(ValueRepr::Block, block_layout);
    let version = builder.plain(ValueRepr::Block, block_layout);
    let returned = builder.owned(ValueRepr::Block, block_layout);
    let heap = Placement::Fixed(hirsel_rc::placement::TASK_HEAP);
    let construction = builder.construct(ConstructorKind::Tuple, empty, &[(0, operand)], heap);
    builder.unit.tables.constructs[construction.index()].destination = DestinationClass::Inferred;
    let result_construction =
        builder.construct(ConstructorKind::Tuple, empty, &[(0, operand)], heap);
    for inst in [
        Inst::StorageLive { slot },
        Inst::Construct {
            result: local,
            construct: construction,
            failure,
        },
        Inst::Store {
            address,
            value: local,
            version,
        },
        Inst::Drop {
            address,
            if_live: false,
            anchor: CLEANUP,
            failure: drop_failure,
        },
        Inst::StorageDead {
            slot,
            if_live: false,
        },
        Inst::Construct {
            result: returned,
            construct: result_construction,
            failure: return_failure,
        },
    ] {
        builder.push(body, inst);
    }
    builder.term(body, Terminator::Return(Some(returned)));
    let end_edge = builder.edge(end, &[]);
    builder.term(failed, Terminator::Jump(end_edge));
    for site in &mut builder.unit.tables.constructs {
        site.placement = None;
    }
    let value = builder.finish_with_layouts(&table);
    let rows = hirsel_core_contract::Manifest::skeleton([]);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &(),
        target: &target,
        layouts: &table,
        manifest: &rows,
    };
    let violations = hirsel_lir::validate::check(&value, &inputs);
    assert!(violations.is_empty(), "{violations:?}\n{}", dump(&value));
    let placed = hirsel_rc::placement::place(
        value,
        &table,
        &Options {
            regions: true,
            noalloc: false,
        },
    )
    .unwrap();
    assert!(
        placed
            .tables
            .constructs
            .iter()
            .any(|site| site.placement.is_some_and(region))
    );
    let violations = hirsel_lir::validate::check(&placed, &inputs);
    assert!(violations.is_empty(), "{violations:?}\n{}", dump(&placed));
    let rc = hirsel_rc::insert::insert(placed, &table, &Proofs::default()).unwrap();
    let violations = hirsel_lir::validate::check(&rc, &inputs);
    assert!(violations.is_empty(), "{violations:?}\n{}", dump(&rc));
    // Invoke the block-returning fixture from an ordinary scalar entry.
    let mut caller = UnitBuilder::<Rc>::new(fixture_unit(path("entry")));
    caller.unit.header.stage = hirsel_lir::stage::StageKind::Rc;
    caller.unit.header.abi = hirsel_lir::abi_types::FnAbiId(1);
    let entry = caller.block(BlockKind::Body, &[]);
    let normal = caller.block(BlockKind::Body, &[]);
    let failed = caller.block(BlockKind::FailureCleanup, &[]);
    caller.term(failed, Terminator::Propagate);
    let failure_edge = caller.edge(failed, &[]);
    let failure = caller.failure(failure_edge, None, &[]);
    let slot = caller.slot(ValueRepr::Block, block_layout, SlotKind::Temporary);
    let whole = caller.address(AddressBase::Slot(slot), &[], ValueRepr::Block, block_layout);
    let repr = ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64);
    let field = caller.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        repr,
        number,
    );
    let preparation = caller.edge(failed, &[]);
    let preparation = caller.failure(preparation, None, &[]);
    caller.push(
        entry,
        Inst::Address {
            address: whole,
            failure: preparation,
        },
    );
    caller.push(entry, Inst::StorageLive { slot });
    let returned = caller.owned(ValueRepr::Block, block_layout);
    let version = caller.plain(ValueRepr::Block, block_layout);
    let answer = caller.plain(repr, number);
    let callee = caller.direct(path("region_return"));
    let call = caller.call(callee, &[], Some(returned));
    caller.unit.tables.calls[call.index()].destination = None;
    let into = caller.edge(normal, &[]);
    caller.term(
        entry,
        Terminator::Call {
            call,
            normal: Some(into),
            failure,
        },
    );
    caller.push(
        normal,
        Inst::Store {
            address: whole,
            value: returned,
            version,
        },
    );
    // The field is located in the stored block, so it is prepared after the
    // store; its failure drops the slot.
    let dropping = caller.block(BlockKind::FailureCleanup, &[]);
    let failure_edge = caller.edge(failed, &[]);
    let failure = caller.failure(failure_edge, None, &[]);
    caller.push(
        dropping,
        Inst::Drop {
            address: whole,
            if_live: false,
            anchor: CLEANUP,
            failure,
        },
    );
    let onward = caller.edge(failed, &[]);
    caller.term(dropping, Terminator::Jump(onward));
    let preparation = caller.edge(dropping, &[]);
    let preparation = caller.failure(preparation, None, &[]);
    caller.push(
        normal,
        Inst::Address {
            address: field,
            failure: preparation,
        },
    );
    caller.push(
        normal,
        Inst::Read {
            result: answer,
            address: field,
            reaching: version,
            mode: ReadMode::Bits,
        },
    );
    let failure_edge = caller.edge(failed, &[]);
    let failure = caller.failure(failure_edge, None, &[]);
    caller.push(
        normal,
        Inst::Drop {
            address: whole,
            if_live: false,
            anchor: CLEANUP,
            failure,
        },
    );
    caller.term(normal, Terminator::Return(Some(answer)));
    let caller = caller.finish_with_layouts(&table);
    let violations = hirsel_lir::validate::check(&caller, &inputs);
    assert!(violations.is_empty(), "{violations:?}\n{}", dump(&caller));
    let program = hirsel_heapcheck::Program {
        target,
        layouts: table,
        units: vec![caller, rc],
        statics: vec![],
    };
    for full_recount_every_step in [false, true] {
        let config = Config {
            full_recount_every_step,
            ..Config::default()
        };
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("entry"),
            &[],
            &mut || Box::new(NoOperations),
            &config,
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}\n{:?}",
            campaign.first_violation(),
            campaign.clean
        );
    }
}
