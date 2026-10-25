//! CT-4 source budgets: each evaluation charges one unit per source call and entered
//! loop iteration, exactly B units succeed, and the next event is denied before it
//! executes. Vectors port rustc's stable-metric UI tests
//! (tests/ui/consts/const-eval/stable-metric/ctfe-{fn-call,recursion,simple-loop}.rs,
//! counted by compiler/rustc_mir_transform/src/ctfe_limit.rs:22-38) and Clang's
//! test/SemaCXX/constexpr-steps.cpp boundary (check before decrement,
//! lib/AST/ExprConstant.cpp:969-980), restated in Hirsel's charge units.
use hirsel_driver::*;
use hirsel_engine::{store::MemStore, *};
use std::sync::Arc;

/// A diagnostic as lines: rule, primary line, secondary lines and notes.
type Summary = (String, u32, Vec<u32>, Vec<String>);

struct Run {
    budget: u64,
    jobs: usize,
    store: Option<Arc<MemStore>>,
    verify: Verify,
    lowering: LoweringPolicy,
}
impl Run {
    fn new(budget: u64) -> Self {
        Self {
            budget,
            jobs: 1,
            store: None,
            verify: Verify::On,
            lowering: LoweringPolicy::Comptime,
        }
    }
    fn check(&self, source: &str) -> Vec<Summary> {
        with_compiler_stack(|| {
            let directory = tempfile::tempdir().unwrap();
            // Without std, whose own constants would spend these small budgets.
            std::fs::write(directory.path().join("main.hir"), source).unwrap();
            let path = directory.path().join("hirsel.json");
            std::fs::write(&path, r#"{"schema":1,"package":{"name":"budget","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let mut config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(self.jobs),
                verify: self.verify,
                ..CompileConfig::for_memory_cap(None)
            };
            if let Some(store) = &self.store {
                config.store = store.clone();
            }
            let mut program = COMPILER
                .with_config(config)
                .with_comptime_limits(ComptimeLimits {
                    budget: self.budget,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let output = program.check(self.lowering).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(program.engine.last_comptime_limit.is_none());
            let line = |span| output.sources.location(span).unwrap().line;
            output
                .diagnostics
                .iter()
                .map(|d| {
                    (
                        d.rule.as_str().to_string(),
                        line(d.primary),
                        d.secondary.iter().map(|l| line(l.span)).collect(),
                        d.notes.clone(),
                    )
                })
                .collect()
        })
    }
}

/// CT-1 at the comptime expression's line, denying the event on `denied` under `budget`.
fn exhausted(at: u32, denied: u32, budget: u64) -> Vec<Summary> {
    vec![(
        "CT-1".into(),
        at,
        vec![denied],
        vec![format!(
            "compile-time budget: {budget} units, one per source call and loop iteration"
        )],
    )]
}

const COUNT: &str = "fn count(n: u64) -> u64 {
  let mut i: u64 = 0
  while i < n {
    i = i + 1
  }
  i
}
";
fn count(n: u64) -> String {
    format!("{COUNT}const C: u64 = comptime count({n})\npub fn main() -> u64 {{ C }}\n")
}

// constexpr-steps.cpp: steps(MAX - 4) passes and steps(MAX - 3) fails at MAX = 10 and
// 1234. Here count(n) charges its call and n iterations, so n = B - 1 spends exactly B.
#[test]
fn the_exact_budget_succeeds_and_the_next_iteration_is_denied() {
    for budget in [10, 1234] {
        assert_eq!(Run::new(budget).check(&count(budget - 1)), vec![]);
        assert_eq!(
            Run::new(budget).check(&count(budget)),
            exhausted(8, 3, budget)
        );
    }
}

// One program needing exactly K = 6 units under B = K - 1, K and K + 1.
#[test]
fn budgets_below_at_and_above_a_programs_charges() {
    let source = count(5);
    assert_eq!(Run::new(5).check(&source), exhausted(8, 3, 5));
    assert_eq!(Run::new(6).check(&source), vec![]);
    assert_eq!(Run::new(7).check(&source), vec![]);
}

// ctfe-fn-call.rs: the call past the budget is the one reported.
#[test]
fn sequential_calls_charge_one_each_and_the_denied_call_is_the_site() {
    let calls = (0..20).map(|_| "  let _ = foo()\n").collect::<String>();
    let source = format!(
        "fn foo() -> u64 {{ 0 }}\nfn call_foo() -> u64 {{\n{calls}  0\n}}\nconst X: u64 = comptime call_foo()\npub fn main() -> u64 {{ X }}\n"
    );
    // call_foo and its twenty calls: 21 units.
    assert_eq!(Run::new(21).check(&source), vec![]);
    // Under 20, call_foo and 19 calls run; the twentieth (line 22) is denied.
    assert_eq!(Run::new(20).check(&source), exhausted(25, 22, 20));
}

// ctfe-recursion.rs: every recursive call, including a self-tail restart, charges.
#[test]
fn recursive_and_tail_calls_charge_each_restart() {
    let source = "fn recurse(n: u64) -> u64 {
  if n == 0 {
    n
  } else {
    recurse(n - 1)
  }
}
const X: u64 = comptime recurse(19)
pub fn main() -> u64 { X }
";
    assert_eq!(Run::new(20).check(source), vec![]);
    assert_eq!(Run::new(19).check(source), exhausted(8, 5, 19));
}

#[test]
fn an_empty_infinite_loop_stops_at_its_budget() {
    let source = "fn spin() -> u64 {
  while true {
  }
  0
}
const X: u64 = comptime spin()
pub fn main() -> u64 { X }
";
    assert_eq!(Run::new(1000).check(source), exhausted(6, 2, 1000));
}

// A call written in a contract charges like any source call; the clause itself is
// compiler-run and adds nothing.
#[test]
fn a_contract_call_charges_its_unit() {
    let source = "fn positive(n: u64) -> bool { n > 0 }
fn checked(n: u64) -> u64
  requires positive(n) {
  n
}
const X: u64 = comptime checked(3)
pub fn main() -> u64 { X }
";
    assert_eq!(Run::new(2).check(source), vec![]);
    assert_eq!(Run::new(1).check(source), exhausted(6, 3, 1));
}

// A size-dependent comptime expression is its own staged evaluation with its own
// budget: count(2) spends 3 units there, while the constant's evaluation spends 2
// (outer and scaled). Under B = 3 both pass although together they spend 5.
#[test]
fn a_nested_staged_evaluation_has_its_own_budget() {
    let source = format!(
        "{COUNT}fn scaled[n: size](xs: Inline[u8, n]) -> u64 {{
  let _ = xs
  comptime count(n)
}}
fn outer() -> u64 {{
  let xs: Inline[u8, 2] = [1, 2]
  scaled(xs)
}}
const X: u64 = comptime outer()
pub fn main() -> u64 {{ X }}
"
    );
    assert_eq!(Run::new(3).check(&source), vec![]);
    let denied = Run::new(2).check(&source);
    assert_eq!(denied.len(), 1, "{denied:?}");
    assert_eq!(denied[0].0, "CT-1");
    assert!(denied[0].3.iter().any(|n| n.contains("budget: 2 units")));
}

// Acceptance and its diagnostics are the same cold and warm, at one and four workers,
// with and without V, and whether the compile lowers only evaluated bodies or every
// reachable one.
#[test]
fn acceptance_is_identical_across_caches_workers_and_lowering() {
    for (n, budget) in [(5, 6), (6, 6)] {
        let source = count(n);
        let expected = Run::new(budget).check(&source);
        assert_eq!(expected.is_empty(), n < budget);
        for jobs in [1, 4] {
            let store = Arc::new(MemStore::default());
            for _warm in [false, true] {
                let run = Run {
                    jobs,
                    store: Some(store.clone()),
                    ..Run::new(budget)
                };
                assert_eq!(run.check(&source), expected, "jobs {jobs}");
            }
            for verify in [Verify::On, Verify::Off] {
                for lowering in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
                    let run = Run {
                        jobs,
                        verify,
                        lowering,
                        ..Run::new(budget)
                    };
                    assert_eq!(run.check(&source), expected, "jobs {jobs} {lowering:?}");
                }
            }
        }
    }
}

// A cached result never answers for another budget: the store is keyed by it.
#[test]
fn a_warm_store_re_evaluates_under_a_changed_budget() {
    let source = count(5);
    let store = Arc::new(MemStore::default());
    let at = |budget| {
        Run {
            store: Some(store.clone()),
            ..Run::new(budget)
        }
        .check(&source)
    };
    assert_eq!(at(6), vec![]);
    assert_eq!(at(5), exhausted(8, 3, 5));
    assert_eq!(at(6), vec![]);
}
