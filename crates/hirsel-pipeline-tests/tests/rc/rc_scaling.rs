//! Compile time of the native middle end against the size of one function.
//!
//! `pipeline_time_is_near_linear_in_function_size` is the gate: every stage
//! from import to Ready, and C after each, on a function of two shapes and
//! on one eight times its size (20 and 160 straight-line statements).
//! `failure_cleanups_grow_linearly_with_live_counted_values` gates the size
//! of shared failure cleanups and the time they cost.
use crate::rc_support as support;

use hirsel_lir::ir::LirUnit;
use hirsel_lir::stage::Stage;
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::InstanceId;
use hirsel_rc::placement::Options;
use std::time::{Duration, Instant};
use support::*;

/// `n` statements, each a call, a comparison and a checked addition.
fn straight(n: usize) -> String {
    let mut s = String::from(
        "fn wa(a: i32, b: i32) -> i32 {
  int.wrapping_add(a, b)
}
fn check(b: bool) -> u64 {
  if b {
    0
  } else {
    1
  }
}
fn value() -> u64 {
  let mut bad: u64 = 0
",
    );
    for i in 0..n {
        s += &format!("  bad = bad + check(wa({i}i32, 1i32) == {}i32)\n", i + 1);
    }
    s + "  bad\n}\npub fn main() -> Result[(), u64] {\n  Error(value())\n}\n"
}

/// `n` counted lists, all live to the end, read in a loop whose body nests
/// `n` conditionals: `n` counted values live across about `n` failure sites,
/// whose cleanup chains share their tails.
fn deep(n: usize) -> String {
    let mut s = String::from("fn value(k: u64) -> u64 {\n");
    for i in 0..n {
        s += &format!("  let x{i}: List[u64] = [{i}, k]\n");
    }
    s += "  let mut total: u64 = 0\n  let mut i: u64 = 0\n  while i < k {\n";
    for i in 0..n {
        s += &format!("    if x{i}[1] > i {{\n      total = total + x{i}[0]\n");
    }
    for _ in 0..n {
        s += "    }\n";
    }
    s += "    i = i + 1\n  }\n";
    for i in 0..n {
        s += &format!("  total = total + x{i}[0]\n");
    }
    s + "  total\n}\npub fn main() -> Result[(), u64] {\n  Error(value(3))\n}\n"
}

/// A clock that a loaded host disturbs little: the time this thread has
/// spent running (Linux), or the wall clock where that is not reported.
#[derive(Clone, Copy)]
enum Clock {
    Running(Duration),
    Wall(Instant),
}

impl Clock {
    fn now() -> Self {
        std::fs::read_to_string("/proc/thread-self/schedstat")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse().ok())
            .map(|ns| Clock::Running(Duration::from_nanos(ns)))
            .unwrap_or_else(|| Clock::Wall(Instant::now()))
    }

    fn elapsed(self) -> Duration {
        match (self, Clock::now()) {
            (Clock::Running(then), Clock::Running(now)) => now.saturating_sub(then),
            (Clock::Wall(then), _) => then.elapsed(),
            (Clock::Running(_), Clock::Wall(_)) => Duration::ZERO,
        }
    }
}

/// Control flow over a unit that stays linear: a loop whose body is `n / 4`
/// nests of four conditionals, each reading two counted lists and adding to
/// one total. (Import joins every written slot still live at a merge, the
/// temporaries of enclosing statements included, so one nest of `n` has
/// about `n * n` values: `locals`.)
fn nested(n: usize) -> String {
    let mut s = String::from(
        "fn value(k: u64) -> u64 {
  let xs: List[u64] = [1, k]
  let ys: List[u64] = [2, k]
  let mut total: u64 = 0
  let mut i: u64 = 0
  while i < k {
",
    );
    for nest in 0..n / 4 {
        for i in 0..4 {
            let c = nest * 4 + i;
            s += &format!("    if xs[1] + {c} > i {{\n      total = total + ys[0] + {c}\n");
        }
        s += "    }\n    }\n    }\n    }\n";
    }
    s + "    i = i + 1\n  }\n  total + ys[1]\n}\npub fn main() -> Result[(), u64] {\n  Error(value(3))\n}\n"
}

const STAGES: [&str; 14] = [
    "front end",
    "import",
    "C value",
    "placement",
    "C placed",
    "insert",
    "elide",
    "C rc",
    "reuse",
    "C reuse",
    "cleanup",
    "legalize",
    "C ready",
    "total",
];

fn checked<S: Stage>(units: &[LirUnit<S>], inputs: &Inputs<'_>) -> Duration {
    let t = Clock::now();
    for u in units {
        let v = hirsel_lir::validate::check(u, inputs);
        assert!(v.is_empty(), "{:?}\n{}\n{v:#?}", S::KIND, dump(u));
    }
    t.elapsed()
}

/// The time of each stage of `STAGES` over every unit of a checked program, the
/// last being everything after the front end; the front end's entry stays
/// zero. Also the number of blocks of the Ready units.
fn middle_end(program: &hirsel_rc::instance::Program) -> ([Duration; 14], usize) {
    let mut out = [Duration::ZERO; 14];
    let t = Clock::now();
    let lowered = hirsel_rc::import::lower(program, &target()).unwrap();
    out[1] = t.elapsed();
    let table = &lowered.table;
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        operations: &program.operation_registry(),
        target: &target,
        layouts: table,
        manifest: &manifest,
    };
    out[2] = checked(&lowered.units, &inputs);
    let ids = || (0..lowered.units.len()).map(|i| InstanceId(i as u32));
    let t = Clock::now();
    let placed: Vec<_> = ids()
        .map(|id| {
            let options = Options::for_instance(true, &lowered.instances, id);
            hirsel_rc::placement::place(lowered.units[id.index()].clone(), table, &options).unwrap()
        })
        .collect();
    out[3] = t.elapsed();
    out[4] = checked(&placed, &inputs);
    let t = Clock::now();
    let inserted: Vec<_> = ids()
        .zip(placed)
        .map(|(id, u)| {
            let proofs = Proofs::for_instance(&lowered.instances, id);
            hirsel_rc::insert::insert(u, table, &proofs).unwrap()
        })
        .collect();
    out[5] = t.elapsed();
    let t = Clock::now();
    let rc: Vec<_> = inserted
        .into_iter()
        .map(|u| hirsel_rc::elide::elide(u, table).unwrap())
        .collect();
    out[6] = t.elapsed();
    out[7] = checked(&rc, &inputs);
    let t = Clock::now();
    let reuse: Vec<_> = rc
        .into_iter()
        .map(|u| hirsel_rc::reuse::reuse(u, table, &Default::default()).unwrap())
        .collect();
    out[8] = t.elapsed();
    out[9] = checked(&reuse, &inputs);
    let finalizers = Finalizers::new(&lowered.instances, &reuse);
    let t = Clock::now();
    let lowered_cleanup: Vec<_> = reuse
        .into_iter()
        .map(|u| hirsel_rc::cleanup::lower(u, table, &finalizers).unwrap())
        .collect();
    out[10] = t.elapsed();
    let t = Clock::now();
    let ready: Vec<_> = lowered_cleanup
        .into_iter()
        .map(|u| hirsel_rc::legalize::legalize(u, table).unwrap())
        .collect();
    out[11] = t.elapsed();
    out[12] = checked(&ready, &inputs);
    out[13] = out[1..13].iter().sum();
    (out, ready.iter().map(|u| u.blocks.len()).sum())
}

/// Eight times the statements may take at most 32 times as long. Linear is 8
/// and `n log n` about 14; quadratic is 64, and before the states shared
/// structure the straight-line shape measured about 400. The bound is loose
/// and a failing measurement is taken again, twice, so a loaded host does not
/// fail it.
#[test]
fn pipeline_time_is_near_linear_in_function_size() {
    // The nested shape at half the size: the front end takes over a minute
    // on 160 nested conditionals unoptimized.
    let shapes: [(&str, fn(usize) -> String, usize); 2] =
        [("straight", straight, 20), ("nested", nested, 10)];
    for (name, shape, n) in shapes {
        let (small, large) = (compile(&shape(n)), compile(&shape(8 * n)));
        let ratio = || {
            let total = |p| middle_end(p).0[STAGES.len() - 1].as_secs_f64();
            total(&large) / total(&small)
        };
        let mut least = ratio();
        for _ in 0..2 {
            if least >= 32.0 {
                least = least.min(ratio());
            }
        }
        assert!(
            least < 32.0,
            "{name}: {} statements took {least:.1} times as long as {n}",
            8 * n
        );
    }
}

/// Failure cleanups share their tails. The deep shape keeps `n` counted
/// lists live across about `n` failure sites; when every site had its own
/// chain of `n` drops the unit had about `13 * n * n` blocks (5207 at 20)
/// and every later stage followed. Doubling `n` from 20 to 40 adds about
/// twice the blocks that doubling it from 10 to 20 did (four times when
/// quadratic), and four times `n` takes at most 10 times as long (linear is
/// 4, quadratic 16; with a chain per site it measured 15).
#[test]
fn failure_cleanups_grow_linearly_with_live_counted_values() {
    let [small, middle, large] = [10, 20, 40].map(|n| compile(&deep(n)));
    let blocks = [&small, &middle, &large].map(|p| middle_end(p).1);
    let (first, second) = (blocks[1] - blocks[0], blocks[2] - blocks[1]);
    assert!(
        2 * second <= 5 * first,
        "Ready blocks at 10, 20 and 40 live lists: {blocks:?}"
    );
    let ratio = || {
        let total = |p| middle_end(p).0[STAGES.len() - 1].as_secs_f64();
        total(&large) / total(&small)
    };
    let mut least = ratio();
    for _ in 0..2 {
        if least >= 10.0 {
            least = least.min(ratio());
        }
    }
    assert!(
        least < 10.0,
        "40 live lists took {least:.1} times as long as 10"
    );
}
