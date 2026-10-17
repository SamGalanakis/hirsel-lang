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
use hirsel_rc::instance::InstanceId;
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
    let (program, lowered) = lower_valid(source);
    let table = lowered.table.clone();
    let manifest = manifest(&program);
    let mut units = Vec::new();
    for (i, u) in lowered.units.into_iter().enumerate() {
        let id = InstanceId(i as u32);
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
    Built { units, table }
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
    let mut out: Vec<(Mode, Outcome)> = Vec::new();
    for mode in MODES {
        let built = build(source, mode);
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
    // With regions on, these blocks are region-placed and never donate.
    let outcomes = run_all(&source);
    {
        let regions = false;
        let (on, off) = (on(&outcomes, regions, true), on(&outcomes, regions, false));
        assert_eq!(returned_u64(&on.observation), 3);
        assert_eq!(on.counters.reuses, 1, "regions {regions}");
        assert_eq!(on.counters.allocations + 1, off.counters.allocations);
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
