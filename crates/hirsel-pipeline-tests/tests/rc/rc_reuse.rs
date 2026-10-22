//! Reuse tokens over real driver-derived programs and hand-built Rc units:
//! every Reuse unit passes its validator, the heap executor runs it with an
//! empty terminal ledger on the normal path and on every injected failure,
//! and reuse-on and reuse-off runs observe the same thing. Counters and
//! dumps witness actual reuse, so output alone never passes a test.
use crate::rc_support as support;

use hirsel_heapcheck::{Config, NoOperations, Observation};
use hirsel_lir::ir::{Inst, LirUnit};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::{Reuse, Stage};
use hirsel_lir::validate::Inputs;
use hirsel_rc::insert::Proofs;
use hirsel_rc::placement::Options as PlaceOptions;
use hirsel_rc::reuse::Options;
use support::*;

#[derive(Clone, Copy, Debug)]
struct Mode {
    regions: bool,
    reuse: bool,
}

const MODES: [Mode; 4] = [
    Mode {
        regions: true,
        reuse: true,
    },
    Mode {
        regions: true,
        reuse: false,
    },
    Mode {
        regions: false,
        reuse: true,
    },
    Mode {
        regions: false,
        reuse: false,
    },
];

struct Built {
    units: Vec<LirUnit<Reuse>>,
    table: LayoutTable,
    specializations: usize,
}

#[test]
fn decimal_money_keeps_exhaustive_switch_defaults_direct() {
    let source = include_str!("../../../../conformance/SEM/SEM-1-decimal-money-run.hir")
        .replace("decimal.", "std@1/decimal.")
        .replace("bigint.", "std@1/bigint.")
        .replace("json.", "std@1/json.");
    build(&source, MODES[0]);
}

fn check<S: Stage>(
    program: &hirsel_rc::instance::Program,
    manifest: &hirsel_core_contract::Manifest,
    table: &LayoutTable,
    units: &[LirUnit<S>],
    what: &str,
) {
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: table,
        manifest,
    };
    for u in units {
        let v = hirsel_lir::validate::check(u, &inputs);
        assert!(
            v.is_empty(),
            "{:?} unit {}: {v:?}\n{what}\n{}",
            S::KIND,
            u.header.definition.0.name,
            dump(u)
        );
    }
}

/// Lower, place, insert, elide and reuse, validating every stage.
fn build(source: &str, mode: Mode) -> Built {
    build_with_scoped(source, mode, true)
}

fn build_with_scoped(source: &str, mode: Mode, scoped: bool) -> Built {
    let (program, lowered) = if scoped {
        lower_valid(source)
    } else {
        crate::scoped_consumer_9::lower_disabled(source)
    };
    let specializations = lowered.units.len() - lowered.instances.len();
    let table = lowered.table.clone();
    let manifest = manifest(&program);
    let mut units = Vec::new();
    assert_eq!(lowered.units.len(), lowered.unit_origins.len());
    for (u, id) in lowered.units.into_iter().zip(lowered.unit_origins) {
        let options = PlaceOptions::for_instance(mode.regions, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u, &table, &options).expect("placement");
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let rc = hirsel_rc::insert::insert(placed, &table, &proofs).expect("insertion");
        let rc = hirsel_rc::elide::elide(rc, &table).expect("elision");
        check(
            &program,
            &manifest,
            &table,
            std::slice::from_ref(&rc),
            source,
        );
        let reused =
            hirsel_rc::reuse::reuse(rc, &table, &Options { reuse: mode.reuse }).expect("reuse");
        units.push(reused);
    }
    check(&program, &manifest, &table, &units, source);
    Built {
        units,
        table,
        specializations,
    }
}

fn heap_program(built: &Built) -> hirsel_heapcheck::Program<Reuse> {
    hirsel_heapcheck::Program {
        target: target(),
        layouts: built.table.clone(),
        units: built.units.clone(),
        statics: Vec::new(),
    }
}

fn count(units: &[LirUnit<Reuse>], f: impl Fn(&Inst) -> bool) -> usize {
    units
        .iter()
        .map(|u| u.instructions.iter().filter(|i| f(i)).count())
        .sum()
}

/// What one mode did: its observation, its counters and its built units.
struct Outcome {
    observation: Observation,
    exemptions: Vec<hirsel_heapcheck::Exemption>,
    counters: hirsel_heapcheck::Counters,
    built: Built,
}

/// Run `source` in every mode, each failure-capable site of each mode
/// injected in turn as well; every audit is clean and reuse-on and
/// reuse-off observe the same thing with the same terminal exemptions.
fn run_all(source: &str) -> Vec<(Mode, Outcome)> {
    run_all_with_scoped(source, true)
}

fn run_all_with_scoped(source: &str, scoped: bool) -> Vec<(Mode, Outcome)> {
    let mut out: Vec<(Mode, Outcome)> = Vec::new();
    for mode in MODES {
        let built = build_with_scoped(source, mode, scoped);
        let campaign = hirsel_heapcheck::fault_campaign(
            &heap_program(&built),
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        let dumps = || built.units.iter().map(dump).collect::<Vec<_>>().join("\n");
        let audit = campaign
            .clean
            .result
            .clone()
            .unwrap_or_else(|v| panic!("{mode:?}: {v}\n{source}\n{}", dumps()));
        for (site, run) in &campaign.injected {
            if let Err(v) = &run.result {
                panic!("{mode:?} failing site {site}: {v}\n{source}\n{}", dumps());
            }
        }
        if let Some((_, first)) = out.iter().find(|(m, _)| m.regions == mode.regions) {
            assert!(
                first.observation.equivalent(&audit.observation),
                "{source}\n{mode:?}: {:?} != {:?}",
                audit.observation,
                first.observation
            );
            assert_eq!(first.exemptions.len(), audit.exemptions.len(), "{mode:?}");
        }
        out.push((
            mode,
            Outcome {
                observation: audit.observation,
                exemptions: audit.exemptions.iter().cloned().collect(),
                counters: campaign.clean.counters,
                built,
            },
        ));
    }
    out
}

fn on(outcomes: &[(Mode, Outcome)], regions: bool, reuse: bool) -> &Outcome {
    &outcomes
        .iter()
        .find(|(m, _)| m.regions == regions && m.reuse == reuse)
        .unwrap()
        .1
}

fn returned_u64(o: &Observation) -> u128 {
    match o.returned {
        Some(hirsel_heapcheck::Observed::Integer { bits, .. }) => bits,
        ref other => panic!("not an integer: {other:?}"),
    }
}

const P: &str = "type P { a: u64, b: u64 }\ntype Q { b: box P, n: u64 }\n";

/// A box released at its last use donates its storage to the next box of
/// the same bucket: one reuse instead of a free and an allocation.
#[test]
fn a_released_box_donates_to_the_next_construction() {
    let source = format!(
        "{P}fn mk(n: u64) -> box P {{ box P(a: n, b: n) }}\n\
         fn swap(take t: box P, n: u64) -> box P {{ let u = t; box P(a: n, b: 2) }}\n\
         fn main() -> u64 {{ let t: box P = mk(1); let u = swap(take t, 4); 3 }}"
    );
    let outcomes = run_all(&source);
    for regions in [true, false] {
        let on = on(&outcomes, regions, true);
        let off = super_off(&outcomes, regions);
        assert_eq!(returned_u64(&on.observation), 3);
        assert_eq!(on.counters.reuses, 1, "regions {regions}");
        assert_eq!(off.counters.reuses, 0);
        assert_eq!(on.counters.allocations + 1, off.counters.allocations);
        assert_eq!(
            count(&on.built.units, |i| matches!(i, Inst::Reuse { .. })),
            1
        );
    }
}

fn super_off(outcomes: &[(Mode, Outcome)], regions: bool) -> &Outcome {
    on(outcomes, regions, false)
}

/// A closure's environment block donates to a box of its bucket when the
/// closure's slot is dropped: the drop becomes a take and a reset keyed by
/// the environment layout the closure was built with.
#[test]
fn a_dropped_closure_donates_its_environment() {
    let source = format!(
        "{P}fn main() -> u64 {{\n  let k = 5u64\n  \
         let r = {{ let c = fn() -> u64 {{ k + 1 }}; c() }}\n  \
         let v: box P = box P(a: r, b: 2)\n  3\n}}"
    );
    // Keep the heap-lowering control: it has an environment to donate.
    // The optimized twin adds generated units and eliminates that environment.
    let outcomes = run_all_with_scoped(&source, false);
    let optimized = run_all(&source);
    for (mode, outcome) in &optimized {
        assert!(outcome.built.specializations > 0);
        assert_eq!(outcome.counters.reuses, 0);
        assert_eq!(returned_u64(&outcome.observation), 3);
        assert!(
            outcome
                .observation
                .equivalent(&on(&outcomes, mode.regions, mode.reuse).observation)
        );
        assert_eq!(
            count(&outcome.built.units, |i| matches!(i, Inst::Reset { .. })),
            0
        );
    }
    {
        let regions = false;
        let (on, off) = (on(&outcomes, regions, true), on(&outcomes, regions, false));
        assert_eq!(returned_u64(&on.observation), 3);
        assert_eq!(on.counters.reuses, 1, "regions {regions}");
        assert_eq!(off.counters.reuses, 0, "reuse-disabled negative control");
        assert_eq!(on.counters.allocations + 1, off.counters.allocations);
        assert_eq!(
            super_off(&optimized, regions).counters.allocations + 1,
            off.counters.allocations,
            "eliminating the environment removes one allocation without reuse"
        );
        let callable_resets = on
            .built
            .units
            .iter()
            .flat_map(|u| {
                u.instructions.iter().filter(move |i| {
                    matches!(i, Inst::Reset { value, .. }
                        if u.types[u.values[value.index()].ty.index()].repr
                            == hirsel_lir::ir::ValueRepr::Callable)
                })
            })
            .count();
        assert_eq!(callable_resets, 1);
    }
}

/// A finalizer fails while a reset token is live: the drop's failure edge
/// disposes the token, and the construction that would have used it never
/// runs.
#[test]
fn a_failing_finalizer_disposes_the_live_token() {
    let source = format!(
        "{P}resource type R {{ n: u64 }}\ndrop(x) {{\n  let _ = 1u8 / 0u8\n}}\n\
         fn hold(take t: box P) -> u64 {{\n  let k = {{\n    let r = R(n: 1)\n    \
         let u = t\n    1u64\n  }}\n  let v: box P = box P(a: 3, b: k)\n  3\n}}\n\
         fn main() -> u64 {{\n  let t: box P = box P(a: 1, b: 2)\n  hold(take t)\n}}"
    );
    // With regions on, these blocks are region-placed and never donate.
    let outcomes = run_all(&source);
    {
        let regions = false;
        let (on, off) = (on(&outcomes, regions, true), on(&outcomes, regions, false));
        assert_eq!(on.observation.failures.len(), 1);
        assert_eq!(on.counters.finalizers, off.counters.finalizers);
        assert_eq!(on.counters.resets, 1, "regions {regions}");
        assert_eq!(on.counters.disposes, 1);
        assert_eq!(on.counters.reuses, 0);
    }
}

const LIST: &str = "type Node { value: u64, next: Option[box Node] }\n\
fn mapped(take root: Option[box Node]) -> Option[box Node] {\n\
  let mut current = take root\n\
  let mut reversed: Option[box Node] = None()\n\
  while true {\n\
    match take current {\n\
      None() => break,\n\
      Some(node) => {\n\
        let Node(value: value, next: next) = take node\n\
        current = take next\n\
        reversed = Some(box Node(value: value + 1u64, next: take reversed))\n\
      },\n\
    }\n\
  }\n\
  reversed\n\
}\n\
fn sum(xs: Option[box Node]) -> u64 {\n\
  match xs { None() => 0u64, Some(n) => n.value + sum(n.next) }\n\
}\n\
fn build() -> Option[box Node] {\n\
  let mut root: Option[box Node] = None()\n\
  let mut i = 3u64\n\
  while i > 0u64 { i -= 1u64; root = Some(box Node(value: i, next: take root)) }\n\
  root\n\
}\n";

/// `let Node(value, next) = take node` over a temporary's box copies `next`
/// and drops the box whole. Specialized on the box's count, a unique box
/// gives `next` up without a Dup and is reused for the new node with no
/// release of `next`, so the per-node Dup and child release cancel away.
#[test]
fn a_destructured_unique_box_moves_its_children() {
    let source = format!("{LIST}fn main() -> u64 {{ let m = mapped(build()); sum(m) }}");
    let outcomes = run_all(&source);
    for regions in [true, false] {
        let (on, off) = (on(&outcomes, regions, true), super_off(&outcomes, regions));
        assert_eq!(returned_u64(&on.observation), 6);
        // Two of the three nodes have a successor to copy out.
        assert_eq!(on.counters.dups + 2, off.counters.dups, "regions {regions}");
        assert_eq!(on.counters.reuses, 3, "regions {regions}");
        let mapped: Vec<_> = on
            .built
            .units
            .iter()
            .filter(|u| u.header.definition.0.name == "mapped")
            .cloned()
            .collect();
        assert_eq!(count(&mapped, |i| matches!(i, Inst::IsUnique { .. })), 1);
    }
}

/// The shared twin: the list is still held, so each box's count is two and
/// the specialized shared path copies `next` and decrements the box, leaving
/// the original list intact.
#[test]
fn a_destructured_shared_box_copies_its_children() {
    let source = format!(
        "{LIST}fn main() -> u64 {{ let xs = build(); let m = mapped(copy xs); sum(m) * 10u64 + sum(xs) }}"
    );
    let outcomes = run_all(&source);
    for regions in [true, false] {
        let (on, off) = (on(&outcomes, regions, true), super_off(&outcomes, regions));
        assert_eq!(returned_u64(&on.observation), 63);
        assert_eq!(on.counters.dups, off.counters.dups, "regions {regions}");
        assert_eq!(on.counters.reuses, 0, "regions {regions}");
    }
}
