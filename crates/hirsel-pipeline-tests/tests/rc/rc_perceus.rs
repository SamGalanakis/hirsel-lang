//! Perceus insertion over real driver-derived programs: every stage passes its
//! validator, and the heap executor runs the generated Rc code with an empty
//! terminal ledger in every configuration.
use crate::rc_support as support;

use hirsel_heapcheck::{Config, NoOperations, Observation};
use hirsel_lir::ir::LirUnit;
use hirsel_lir::stage::{Rc, Stage};
use hirsel_lir::validate::Inputs;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceId, Program};
use hirsel_rc::placement::Options;
use support::*;

#[derive(Clone, Copy, Debug)]
struct Mode {
    regions: bool,
    optimize: bool,
}

const MODES: [Mode; 3] = [
    Mode {
        regions: true,
        optimize: true,
    },
    Mode {
        regions: true,
        optimize: false,
    },
    Mode {
        regions: false,
        optimize: true,
    },
];

struct Built {
    rc: Vec<LirUnit<Rc>>,
    table: hirsel_lir::layout_types::LayoutTable,
}

fn check_stage<S: Stage>(
    program: &Program,
    table: &hirsel_lir::layout_types::LayoutTable,
    units: &[LirUnit<S>],
    source: &str,
) {
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: table,
        manifest: &manifest,
    };
    for u in units {
        let v = hirsel_lir::validate::check(u, &inputs);
        assert!(
            v.is_empty(),
            "{:?} unit {}: {v:?}\n{source}\n{}",
            S::KIND,
            u.header.definition.0.name,
            dump(u)
        );
    }
}

/// Lower, place, insert (and elide when optimizing), checking every stage.
fn build(source: &str, mode: Mode) -> Built {
    let (program, lowered) = lower_valid(source);
    let table = lowered.table.clone();
    let placed: Vec<_> = lowered
        .units
        .into_iter()
        .enumerate()
        .map(|(i, u)| {
            let options =
                Options::for_instance(mode.regions, &lowered.instances, InstanceId(i as u32));
            hirsel_rc::placement::place(u, &table, &options).expect("placement")
        })
        .collect();
    check_stage(&program, &table, &placed, source);
    let mut rc = Vec::new();
    for (i, u) in placed.iter().enumerate() {
        let proofs = Proofs::for_instance(&lowered.instances, InstanceId(i as u32));
        let mut r = hirsel_rc::insert::insert(u.clone(), &table, &proofs).expect("insertion");
        if mode.optimize {
            r = hirsel_rc::elide::elide(r, &table).expect("elision");
        }
        rc.push(r);
    }
    check_stage(&program, &table, &rc, source);
    Built { rc, table }
}

fn execute(built: &Built) -> hirsel_heapcheck::Run {
    let program = hirsel_heapcheck::Program {
        target: target(),
        layouts: built.table.clone(),
        units: built.rc.clone(),
        statics: Vec::new(),
    };
    hirsel_heapcheck::run(
        &program,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    )
}

/// Run `source` in every mode; every audit is clean and every observation
/// equal. Returns the observation and each mode's built program.
fn run_all(source: &str) -> (Observation, Vec<Built>) {
    let mut seen: Option<Observation> = None;
    let mut builds = Vec::new();
    for mode in MODES {
        let built = build(source, mode);
        let run = execute(&built);
        let audit = run.result.unwrap_or_else(|v| {
            let dumps: Vec<String> = built.rc.iter().map(dump).collect();
            panic!("{mode:?}: {v}\n{source}\n{}", dumps.join("\n"))
        });
        if let Some(first) = &seen {
            assert!(
                first.equivalent(&audit.observation),
                "{source}\n{mode:?}: {:?} != {first:?}",
                audit.observation
            );
        } else {
            seen = Some(audit.observation);
        }
        builds.push(built);
    }
    (seen.unwrap(), builds)
}

fn returned_u64(o: &Observation) -> u128 {
    match o.returned {
        Some(hirsel_heapcheck::Observed::Integer { bits, .. }) => bits,
        ref other => panic!("not an integer: {other:?}"),
    }
}

const P: &str = "type P { a: u64, b: u64 }\ntype Q { b: box P, n: u64 }\n";

/// `run_all` over `P`'s declarations and `body`, expecting a returned integer.
fn returns(body: &str, expected: u128) {
    let (o, _) = run_all(&format!("{P}{body}"));
    assert_eq!(returned_u64(&o), expected);
}

/// `run_all`, expecting the program to fail with `rule`.
fn fails(body: &str, rule: &str) {
    let (o, _) = run_all(&format!("{P}{body}"));
    assert_eq!(o.returned, None);
    assert_eq!(o.failures.len(), 1);
    assert_eq!(o.failures[0].rule, rule);
}

#[test]
fn a_box_copied_and_dropped_balances() {
    returns(
        "fn main() -> u64 { let t: box P = box P(a: 1, b: 2); let u = copy t; 3 }",
        3,
    );
}

/// A holder whose last use precedes a failing call: released before the
/// call, so the failure cleanup has nothing of it left to drop.
#[test]
fn a_holder_released_before_a_later_failure() {
    fails(
        "fn sub(x: u64, y: u64) -> u64 { x - y }\n\
         fn main() -> u64 { let t: box P = box P(a: 1, b: 2); let u = copy t; let z = sub(1, 2); z }",
        "SEM-1",
    );
}

/// A holder live across a failing call: only the failure path needs it
/// after the call's cleanup edge, and the cleanup drops it.
#[test]
fn a_holder_only_failure_cleanup_still_needs() {
    fails(
        "fn sub(x: u64, y: u64) -> u64 { x - y }\n\
         fn main() -> u64 { let t: box P = box P(a: 1, b: 2); let z = sub(1, 2); let u = copy t; z }",
        "SEM-1",
    );
}

#[test]
fn a_local_result_shared_twice() {
    returns(
        "fn mk() -> box P { box P(a: 1, b: 2) }\n\
         fn main() -> u64 { let t = mk(); let u = copy t; let v = copy t; 3 }",
        3,
    );
}

/// A counted field copied out of a record before the record's fields are
/// written and the field replaced.
#[test]
fn a_counted_projection_copied_before_mutation() {
    returns(
        "fn main() -> u64 { let mut q = Q(b: box P(a: 1, b: 2), n: 1); let c = copy q.b; q.n = 5; q.b = box P(a: 3, b: 4); q.n }",
        5,
    );
}

/// #66: the `mut` argument's storage is lent after every argument is
/// evaluated, so the later argument reads it first.
#[test]
fn a_mut_argument_read_by_a_later_argument() {
    returns(
        "fn add(mut n: u64, extra: u64) -> () { n = n + extra }\n\
         fn main() -> u64 { let mut q = Q(b: box P(a: 1, b: 2), n: 1); add(mut q.n, q.n + 2); q.n }",
        4,
    );
}

#[test]
fn a_flat_copy_stays_a_copy() {
    returns(
        "fn main() -> u64 { let a = P(a: 1, b: 2); let b = copy a; b.a + a.b }",
        3,
    );
}

/// A borrowed call at the holder's last use: the holder ends after the
/// call's loan, not before it, and the borrowed parameter is never released.
#[test]
fn a_borrowed_call_at_last_use() {
    returns(
        "fn peek(q: Q) -> u64 { q.n }\n\
         fn main() -> u64 { let q = Q(b: box P(a: 1, b: 2), n: 7); peek(q) }",
        7,
    );
}

#[test]
fn a_partial_move_drops_the_rest() {
    returns(
        "type R { b: box P, c: box P, n: u64 }\n\
         fn main() -> u64 { let q = R(b: box P(a: 1, b: 2), c: box P(a: 3, b: 4), n: 7); let R(b: b, n: n, ..) = take q; n }",
        7,
    );
}

/// A loop-carried holder replaced every iteration: its version joins at the
/// loop head and the displaced value is released each time.
#[test]
fn a_loop_phi_replaced_each_iteration() {
    returns(
        "fn main() -> u64 { let mut t: box P = box P(a: 0, b: 0); let mut i = 0u64; while i < 3 { t = box P(a: i, b: i); i = i + 1 } i }",
        3,
    );
}

/// Moved on one branch only: the cleanup at scope end is conditional.
#[test]
fn a_conditional_cleanup() {
    returns(
        "fn one() -> u64 { 1 }\n\
         fn main() -> u64 { let t: box P = box P(a: 1, b: 2); if one() == 1 { let u = take t; } 3 }",
        3,
    );
}

/// A taken parameter released inside the callee at its last use.
#[test]
fn a_taken_parameter_released_at_last_use() {
    returns(
        "fn eat(take t: box P, n: u64) -> u64 { let u = copy t; n + 1 }\n\
         fn main() -> u64 { let t: box P = box P(a: 1, b: 2); eat(take t, 4) }",
        5,
    );
}

fn count<S>(units: &[LirUnit<S>], f: impl Fn(&hirsel_lir::ir::Inst) -> bool) -> usize {
    units
        .iter()
        .map(|u| u.instructions.iter().filter(|i| f(i)).count())
        .sum()
}

/// A copy that is its holder's last use cancels against the release after
/// it: the optimized program moves instead, and both run the same.
#[test]
fn elision_turns_a_final_copy_into_a_move() {
    let source = format!(
        "{P}fn main() -> u64 {{ let t: box P = box P(a: 1, b: 2); let u = copy t; let v = copy u; 3 }}"
    );
    let copies = |optimize| {
        let built = build(
            &source,
            Mode {
                regions: false,
                optimize,
            },
        );
        let run = execute(&built);
        assert!(run.result.is_ok(), "{:?}", run.result);
        (
            count(&built.rc, |i| {
                matches!(i, hirsel_lir::ir::Inst::Copy { .. })
            }),
            run.counters.dups,
        )
    };
    let (plain, plain_dups) = copies(false);
    let (optimized, optimized_dups) = copies(true);
    assert!(
        optimized < plain,
        "{optimized} copies, {plain} before elision"
    );
    assert!(optimized_dups < plain_dups);
}

/// A flat record built from a literal and an integer, copied explicitly, its
/// original's fixed field rewritten: the copy keeps its bytes (MEM-12), and
/// every mode runs clean on the ledger (LIR schema revision 4).
#[test]
fn a_flat_record_from_ordinary_values_copies_its_bytes() {
    let (o, _) = run_all(
        "flat type Item { text: String, n: u64 }\nfn main() -> u64 {\n  let mut a = Item(text: \"flat\", n: 7)\n  let b = copy a\n  a.n = 2\n  b.n * 10 + a.n\n}",
    );
    assert_eq!(returned_u64(&o), 72);
}

/// v1 source cannot write through a block the heap checker executes (box
/// payloads are opaque, sequences are not executable), so this fixture
/// edits E's unit for `let t = box P(..); let u = copy t; ..` to write
/// `t.a = 3` (a replacement) after the copy and return `u.a`. Copy on write must detach `t`
/// before the write: the run returns `u`'s untouched 1.
fn shared_write() -> (
    Program,
    hirsel_lir::layout_types::LayoutTable,
    Vec<LirUnit<hirsel_lir::stage::Value>>,
) {
    use hirsel_lir::ir::{
        AddressBase, AddressInfo, BlockKind, Holding, Inst, PoolRange, ProjectionStep, ReadMode,
        SlotKind, Terminator, ValueRepr,
    };
    let source =
        format!("{P}fn main() -> u64 {{ let t: box P = box P(a: 1, b: 2); let u = copy t; 3 }}");
    let (program, lowered) = lower_valid(&source);
    let table = lowered.table.clone();
    let mut units = lowered.units;
    let m = units
        .iter()
        .position(|u| u.header.definition.0.name == "main")
        .unwrap();
    let mut ed = hirsel_rc::insert::Editor::new(units[m].clone());
    let block_slots: Vec<_> = (0..ed.unit.slots.len())
        .filter(|&s| {
            ed.unit.slots[s].kind == SlotKind::Local
                && ed.unit.types[ed.unit.slots[s].ty.index()].repr == ValueRepr::Block
        })
        .map(|s| hirsel_lir::ir::SlotId(s as u32))
        .collect();
    let [t, u] = block_slots[..] else {
        panic!("two boxes: {block_slots:?}")
    };
    let whole = |unit: &LirUnit<_>, s| {
        (0..unit.addresses.len())
            .find(|&a| {
                let info: &AddressInfo = &unit.addresses[a];
                info.base == AddressBase::Slot(s) && info.projection.len == 0
            })
            .map(|a| hirsel_lir::ir::AddressId(a as u32))
            .unwrap()
    };
    let (wt, wu) = (whole(&ed.unit, t), whole(&ed.unit, u));
    let find = |ed: &hirsel_rc::insert::Editor<_>, f: &dyn Fn(&Inst) -> bool| {
        ed.code
            .iter()
            .enumerate()
            .find_map(|(b, c)| c.iter().position(|i| f(i)).map(|i| (b, i)))
            .unwrap()
    };
    let (b, store_u) = find(
        &ed,
        &|i| matches!(*i, Inst::Store { address, .. } if address == wu),
    );
    let Inst::Store {
        version: u_version, ..
    } = ed.code[b][store_u]
    else {
        unreachable!()
    };
    let (_, store_t) = find(
        &ed,
        &|i| matches!(*i, Inst::Store { address, .. } if address == wt),
    );
    let Inst::Store {
        version: t_version, ..
    } = ed.code[b][store_t]
    else {
        unreachable!()
    };
    let (cb, three) = find(&ed, &|i| {
        matches!(i, Inst::Const { .. }) && false
            || matches!(*i, Inst::Const { constant, .. } if ed.unit.tables.constants[constant.index()].value == hirsel_lir::ir::ConstantValue::Integer(3))
    });
    let Inst::Const {
        result: three_value,
        constant,
    } = ed.code[cb][three]
    else {
        unreachable!()
    };
    let anchor_of = |ed: &hirsel_rc::insert::Editor<_>, a| {
        ed.code
            .iter()
            .flatten()
            .find_map(|i| match *i {
                Inst::Drop {
                    address, anchor, ..
                } if address == a => Some(anchor),
                _ => None,
            })
            .unwrap()
    };
    let (anchor_t, anchor_u) = (anchor_of(&ed, wt), anchor_of(&ed, wu));
    let at = ed.unit.blocks[b].anchor;
    // A failure cleanup with both boxes initialized: drop u, drop t, propagate.
    let chain = |ed: &mut hirsel_rc::insert::Editor<_>| {
        let end = ed.block(BlockKind::FailureCleanup, &[], at);
        ed.unit.terminators[end.index()] = Terminator::Propagate;
        let mut next = end;
        for (address, anchor) in [(wt, anchor_t), (wu, anchor_u)] {
            let link = ed.block(BlockKind::FailureCleanup, &[], at);
            let fe = ed.edge(next, &[]);
            let failure = ed.failure(fe, at);
            ed.code[link.index()].push(Inst::Drop {
                address,
                if_live: false,
                anchor,
                failure,
            });
            let je = ed.edge(next, &[]);
            ed.unit.terminators[link.index()] = Terminator::Jump(je);
            next = link;
        }
        let head = ed.edge(next, &[]);
        ed.failure(head, at)
    };
    let field = |ed: &mut hirsel_rc::insert::Editor<_>, s| {
        let id = hirsel_lir::ir::AddressId(ed.unit.addresses.len() as u32);
        let projection =
            PoolRange::push(&mut ed.unit.pools.projections, [ProjectionStep::Field(0)]);
        let ty = ed.unit.values[three_value.index()].ty;
        ed.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(s),
            projection,
            ty,
        });
        id
    };
    let (ft, fu) = (field(&mut ed, t), field(&mut ed, u));
    let (f1, f2, f3) = (chain(&mut ed), chain(&mut ed), chain(&mut ed));
    let displaced = hirsel_lir::ir::SlotId(ed.unit.slots.len() as u32);
    let scope = ed.unit.slots[t.index()].scope;
    ed.unit.slots.push(hirsel_lir::ir::SlotInfo {
        ty: ed.unit.values[three_value.index()].ty,
        kind: SlotKind::Displaced,
        scope,
    });
    let value_info = ed.unit.values[t_version.index()].clone();
    let t_after = ed.value(value_info.ty, value_info.holding);
    let displaced_version = ed.value(ed.unit.values[three_value.index()].ty, Holding::Plain);
    let written = ed.value(ed.unit.values[three_value.index()].ty, Holding::Plain);
    // The answer reads u.a instead of the constant.
    ed.code[cb][three] = Inst::Read {
        result: three_value,
        address: fu,
        reaching: u_version,
        mode: ReadMode::Bits,
    };
    ed.code[cb].insert(
        three,
        Inst::Address {
            address: fu,
            failure: f2,
        },
    );
    ed.code[b].splice(
        store_u + 1..store_u + 1,
        [
            Inst::Const {
                result: written,
                constant,
            },
            Inst::Address {
                address: ft,
                failure: f1,
            },
            Inst::StorageLive { slot: displaced },
            Inst::Replace {
                address: ft,
                value: written,
                displaced,
                version: t_after,
                displaced_version,
                if_live: false,
                failure: f3,
            },
            Inst::StorageDead {
                slot: displaced,
                if_live: false,
            },
        ],
    );
    units[m] = ed.finish();
    (program, table, units)
}

#[test]
fn copy_on_write_detaches_a_shared_block_before_a_write() {
    let (program, table, units) = shared_write();
    let manifest = manifest(&program);
    let target = target();
    let inputs = Inputs {
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    for u in &units {
        let v = hirsel_lir::validate::check(u, &inputs);
        assert!(v.is_empty(), "{}\n{v:?}", dump(u));
    }
    let placed: Vec<_> = units
        .into_iter()
        .map(|u| hirsel_rc::placement::place(u, &table, &Options::default()).unwrap())
        .collect();
    let run = |rc: Vec<LirUnit<Rc>>| {
        for u in &rc {
            let v = hirsel_lir::validate::check(u, &inputs);
            assert!(v.is_empty(), "{}\n{v:?}", dump(u));
        }
        execute(&Built {
            rc,
            table: table.clone(),
        })
    };
    let detached = run(placed
        .iter()
        .map(|u| hirsel_rc::insert::insert(u.clone(), &table, &Proofs::default()).unwrap())
        .collect());
    let audit = detached.result.expect("a clean run");
    assert_eq!(returned_u64(&audit.observation), 1);
    // The control: without copy on write the write reaches shared storage.
    let shared = run(placed
        .iter()
        .map(|u| hirsel_rc::insert::insert_without_copy_on_write(u.clone(), &table).unwrap())
        .collect());
    assert_eq!(
        shared.violation(),
        Some(hirsel_heapcheck::ViolationKind::WriteShared)
    );
}

fn uniqueness_tests(source: &str, unit: &str) -> usize {
    let mut seen = None;
    for mode in MODES {
        let built = build(source, mode);
        let n = built
            .rc
            .iter()
            .filter(|u| u.header.definition.0.name == unit)
            .map(|u| {
                u.instructions
                    .iter()
                    .filter(|i| matches!(i, hirsel_lir::ir::Inst::IsUnique { .. }))
                    .count()
            })
            .sum();
        assert_eq!(*seen.get_or_insert(n), n, "{mode:?}");
    }
    seen.unwrap()
}

/// A write through a list another holder shares detaches it first. The heap
/// checker does not execute sequences, so C checks these; the box fixture
/// above executes the same transformation.
#[test]
fn a_list_written_after_a_copy_is_detached_first() {
    let n = uniqueness_tests(
        "fn main() -> u64 { let mut xs: List[u64] = [1, 2]; let ys = copy xs; xs[0] = 5; if ys[0] == 1 { 7 } else { 9 } }",
        "main",
    );
    assert_eq!(n, 1);
}

/// COW through shared ancestors: the outer list is made unique, then the
/// inner one it now holds.
#[test]
fn a_write_two_lists_deep_detaches_each_ancestor() {
    let n = uniqueness_tests(
        "fn main() -> u64 { let mut xs: List[List[u64]] = [[1, 2]]; let ys = copy xs; xs[0][1] = 5; if ys[0][1] == 2 { 7 } else { 9 } }",
        "main",
    );
    assert_eq!(n, 2);
}

/// A `mut` argument into a list element: E reserves the place with its
/// mutable loan before the later argument and prepares the argument's own
/// address at the call, after the last argument (#66). Each is detached
/// where it is prepared, so a later argument that shares the list again is
/// still detached before the callee writes.
#[test]
fn a_mut_element_argument_is_detached_after_the_last_argument() {
    let source = "fn add(mut n: u64, extra: u64) -> () { n = n + extra }\n\
                  fn main() -> u64 { let mut xs: List[u64] = [1, 2]; let ys = copy xs; add(mut xs[0], { let zs = copy xs; xs[1] }); if ys[0] == 1 { 7 } else { 9 } }";
    assert_eq!(uniqueness_tests(source, "main"), 2);
}

/// Writes rooted in a `unique` parameter, and every write of an accepted
/// `noalloc` body (MEM-5), need no detach.
#[test]
fn proven_unique_storage_is_written_in_place() {
    assert_eq!(
        uniqueness_tests(
            "fn edit(mut xs: unique List[u64]) -> () { xs[0] = 2 }\n\
             fn main() -> u64 { 3 }",
            "edit",
        ),
        0
    );
    assert_eq!(
        uniqueness_tests(
            "@noalloc\nfn edit(mut xs: unique Array[u8]) -> () { xs[0] = 2 }\n\
             fn main() -> u64 { 3 }",
            "edit",
        ),
        0
    );
    assert_eq!(
        uniqueness_tests(
            "fn edit(mut xs: List[u64]) -> () { xs[0] = 2 }\n\
             fn main() -> u64 { 3 }",
            "edit",
        ),
        1
    );
}

/// A closure capturing a holder by borrow (fix2's environment shape).
#[test]
fn a_borrowed_capture_leaves_its_holder_to_the_frame() {
    returns(
        "fn main() -> u64 { let t: box P = box P(a: 1, b: 2); let c = fn() -> u64 { let u = copy t; 4 }; c() }",
        4,
    );
}

/// Copy on write detaches only the written block (OWN-4: "First mutation of
/// shared storage detaches it"): a shallow copy whose children are shared
/// with the original (one more holder each), never a deep copy of the graph,
/// which is `unique x`. Writing one element of a list of three lists another
/// holder shares allocates the outer list's copy and the new element; the
/// two untouched inner lists stay one allocation each, held by both lists.
#[test]
fn a_detach_copies_the_written_block_and_shares_its_children() {
    let source = "fn main() -> u64 { let mut xs: List[List[u64]] = [[1], [2], [3]]; let ys = copy xs; xs[0] = [9]; xs[0][0] + xs[1][0] + ys[0][0] + ys[1][0] }";
    let built = build(
        source,
        Mode {
            regions: false,
            optimize: true,
        },
    );
    let run = execute(&built);
    let audit = run.result.clone().expect("a clean run");
    assert_eq!(returned_u64(&audit.observation), 9 + 2 + 1 + 2);
    // Four lists built, the detached outer list and the new element: six
    // (a deep copy allocated the three inner lists again: nine). The audit
    // recounts every holder after each step, so the three inner lists are
    // held by both outer lists: one dup for `copy xs` and one per child the
    // detach shares.
    assert_eq!(run.counters.allocations, 6, "{:?}", run.counters);
    assert_eq!(run.counters.dups, 1 + 3, "{:?}", run.counters);
}

#[test]
fn a_part_taken_out_of_a_temporary_s_block_is_copied_and_the_block_stays_shared() {
    let source = "fn main() -> u64 { let xs: List[List[u64]] = [[1], [2, 3]]; list.len({ copy xs }[1]) + list.len(xs[0]) }";
    for optimize in [false, true] {
        let built = build(
            source,
            Mode {
                regions: false,
                optimize,
            },
        );
        let run = execute(&built);
        let audit = run.result.clone().expect("a clean run");
        assert_eq!(returned_u64(&audit.observation), 2 + 1);
        // The three lists alone: taking element 1 out of the shared copy
        // would detach it first (a fourth block) and leave a hole in it.
        assert_eq!(run.counters.allocations, 3, "{:?}", run.counters);
    }
}

#[test]
fn a_view_of_a_shared_block_a_later_argument_writes_is_detached_before_the_view() {
    let source = "type R {\n  name: String,\n  n: u64,\n}\nfn len(s: String, k: u64) -> u64 { string.byte_len(s) + k }\nfn main() -> u64 { let mut xs: List[R] = [R(name: \"ab\", n: 1)]\n let i = 0u64\n let ys = copy xs\n len(xs[i].name, { xs[i].n = 5\n 1 }) + xs[0].n * 10 + ys[0].n * 100 }";
    for optimize in [false, true] {
        let built = build(
            source,
            Mode {
                regions: false,
                optimize,
            },
        );
        let run = execute(&built);
        let audit = run.result.clone().expect("a clean run");
        assert_eq!(returned_u64(&audit.observation), 153);
    }
}

#[test]
fn lists_are_equal_by_their_elements_and_their_lengths() {
    let source = "fn bit(b: bool, k: u64) -> u64 { if b { k } else { 0 } }\nfn main() -> u64 { let a: List[String] = [\"a\", \"b\"]\n let b: List[String] = [\"a\", \"b\"]\n let c: List[String] = [\"a\"]\n let d: List[String] = [\"a\", \"c\"]\n let e: List[List[u64]] = [[1], [2, 3]]\n let f: List[List[u64]] = [[1], [2, 4]]\n let g: List[List[u64]] = [[1], [2, 3]]\n bit(a == b, 1) + bit(a == c, 2) + bit(c == a, 4) + bit(a == d, 8) + bit(e == f, 16) + bit(e == g, 32) + bit(a != d, 64) }";
    for optimize in [false, true] {
        let built = build(
            source,
            Mode {
                regions: false,
                optimize,
            },
        );
        let run = execute(&built);
        let audit = run.result.clone().expect("a clean run");
        assert_eq!(returned_u64(&audit.observation), 1 + 32 + 64);
    }
}

#[test]
fn a_packed_local_holds_its_largest_variant_after_a_whole_replacement() {
    let source = "type Choice = Small(value: u64) | Large(value: u64, extra: u64)\nfn main() -> u64 { let mut x: Packed[Choice] = Small(value: 1)\n x.Small.value = 2\n x = Large(value: 3, extra: 4)\n x.Large.extra = 5\n x.Large.value * 10 + x.Large.extra }";
    for optimize in [false, true] {
        let built = build(
            source,
            Mode {
                regions: false,
                optimize,
            },
        );
        let run = execute(&built);
        let audit = run.result.clone().expect("a clean run");
        assert_eq!(returned_u64(&audit.observation), 35);
    }
}

#[test]
fn a_unique_copy_of_an_inline_value_copies_each_shared_block_once() {
    let source = "type P {\n  a: List[u64],\n  b: List[u64],\n}\nfn main() -> u64 { let xs: List[u64] = [1]; let pair = P(a: copy xs, b: copy xs); let mut u = unique pair; u.a[0] = 5; u.a[0] + u.b[0] + pair.a[0] }";
    let built = build(
        source,
        Mode {
            regions: false,
            optimize: true,
        },
    );
    let run = execute(&built);
    let audit = run.result.clone().expect("a clean run");
    // Both fields of the copy name one copy of `xs`, so writing `u.a`
    // detaches it and `u.b` still reads the copy's 1.
    assert_eq!(returned_u64(&audit.observation), 5 + 1 + 1);
    // `xs`, its one deep copy and the detach: three.
    assert_eq!(run.counters.allocations, 3, "{:?}", run.counters);
}

#[test]
fn cow_loop_resource_replacement_keeps_reaching_versions() {
    let source =
        include_str!("../../../../conformance/OWN/OWN-7-cow-loop-resource-replace-run.hir");
    for mode in MODES {
        build(source, mode);
    }
}
