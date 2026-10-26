//! LANG-10 constructor-tail recursion: qualifying sites run in one logical
//! activation and constant native stack on the reference interpreter, checked
//! H, LLVM and Cranelift; every other site stays ordinary recursion.
//! Vectors follow OCaml 4.14's testsuite/tests/tmc (stack_space.ml, semantic.ml)
//! and Koka's test/cgen/ctail*.kk, restricted to Hirsel's last-field subset.
use hirsel_driver::{load_program_source, native};
use hirsel_engine::{DefinitionQueries, RunConfiguration, SimulationConfig, with_compiler_stack};
use hirsel_tir::TailKind;

/// How a vector runs: its explicit reference step grant (None: the default
/// budget), from which H's instruction limit rises by the same factor, and
/// whether H also runs its campaign, one audited run per failure-capable site
/// the clean run executed, so only short vectors run it.
#[derive(Clone, Copy)]
struct Grant {
    steps: Option<u64>,
    campaign: bool,
}

const SHORT: Grant = Grant {
    steps: None,
    campaign: true,
};
const DEEP: Grant = Grant {
    steps: None,
    campaign: false,
};

/// Check `source`, return the tail kinds of `function`, and run it on all four
/// engines, which must agree; the reference observation and its consumed steps
/// are returned.
fn agree(source: &str, function: &str, grant: Grant) -> (Vec<TailKind>, native::Observation, u64) {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
    let mut program = load_program_source(&fixture, Some(source)).unwrap();
    program
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{source}\n{}", concise(&e)));
    let mut path = program.entry.clone();
    path.0.name = function.into();
    let tails = program.engine.lower_to_tir(&path).unwrap().value.bodies[0]
        .tail_sites
        .iter()
        .map(|t| t.kind.clone())
        .collect();
    let run = RunConfiguration {
        entry: program.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let reference = hirsel_engine::run_observed_snapshot_bounded(
        &mut program.engine,
        &run,
        SimulationConfig::default(),
        grant.steps.unwrap_or(hirsel_interp::DEFAULT_STEP_LIMIT),
    )
    .unwrap();
    let consumed = reference.step_usage.as_ref().unwrap().consumed;
    let observation = native::Observation::of_reference(
        reference
            .observation
            .as_ref()
            .unwrap_or_else(|| panic!("{}", concise(&reference.outcome))),
    );
    let mut matrix = native::trace_roots_matrix();
    matrix.push(native::Configuration {
        options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        engine: native::Engine::Heap {
            campaign: grant.campaign,
        },
    });
    let scratch = tempfile::tempdir().unwrap();
    let tools = native::Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let input = program.engine.export_native_input().unwrap();
    let validated = program.engine.import_native_input(input).unwrap();
    let outcomes = native::differential_in(
        &validated,
        &run.entry,
        &observation,
        &matrix,
        &tools,
        scratch.path(),
        &native::Fixture {
            steps: grant.steps,
            ..Default::default()
        },
    );
    assert_eq!(outcomes.len(), 3);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, native::Outcome::Agree),
            "{source}\n{configuration}: {outcome:?}\nreference: {observation:?}"
        );
    }
    (tails, observation, consumed)
}

/// An error without the source files it carries.
fn concise(error: &impl std::fmt::Debug) -> String {
    let text = format!("{error:?}");
    let text = text.split(", sources: SourceMap").next().unwrap_or(&text);
    text.chars().take(2_000).collect()
}

fn printed(observation: &native::Observation, expected: &str) {
    assert_eq!(observation.exit_code, Some(0), "{observation:?}");
    assert!(observation.failures.is_empty(), "{observation:?}");
    assert_eq!(String::from_utf8_lossy(&observation.output), expected);
}

fn failure(observation: &native::Observation, rule: &str) {
    assert_eq!(observation.exit_code, None, "{observation:?}");
    assert_eq!(observation.failures.len(), 1, "{observation:?}");
    assert_eq!(observation.failures[0].rule, rule, "{observation:?}");
}

/// The chain and its iterative consumer: a `box Chain` cursor walked by a
/// self tail call, so only the builder is under test.
const CHAIN: &str = r#"
type Chain = End | Link(u64, box Chain)
fn sum(take c: box Chain) -> u64 {
  let mut cur = take c
  let mut total = 0u64
  let mut going = true
  while going {
    match take cur {
      End => {
        going = false
        cur = box End
      }
      Link(n, next) => {
        total = total + n
        cur = take next
      }
    }
  }
  total
}
"#;

fn chain_program(builder: &str, n: u64) -> String {
    format!(
        "{CHAIN}{builder}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let c = box chain({n}u64)\n  let _ = out.print(io.stdout(rt)?, u64.to_string(sum(take c)))\n  Ok(())\n}}\n"
    )
}

const LAST_FIELD: &str =
    "fn chain(n: u64) -> Chain {\n  if n == 0u64 { End } else { Link(n, box chain(n - 1u64)) }\n}";

/// Reference steps per level of `chain_program(LAST_FIELD, _)`, builder and
/// consumer together, measured on 5,000 levels (`ctor_tail_chain_5_000_runs_on_four_engines`)
/// with headroom: the 100,000-level grant is sized from it.
const CHAIN_STEPS_PER_LEVEL: u64 = 160;

#[test]
fn ctor_tail_chain_5_000_runs_on_four_engines() {
    with_compiler_stack(|| {
        let (tails, observation, consumed) =
            agree(&chain_program(LAST_FIELD, 5_000), "chain", DEEP);
        assert_eq!(tails, vec![TailKind::ConstructorHole(vec![1, 0])]);
        printed(&observation, "12502500");
        eprintln!("chain 5000: {consumed} reference steps");
        assert!(consumed < 5_000 * CHAIN_STEPS_PER_LEVEL, "{consumed}");
    });
}

/// The acceptance vector: 100,000 levels, a hundred times the activation
/// limit, on four engines under an explicit grant sized from the work.
#[test]
fn ctor_tail_chain_100_000_runs_in_one_activation() {
    with_compiler_stack(|| {
        let grant = Grant {
            steps: Some(100_000 * CHAIN_STEPS_PER_LEVEL),
            campaign: false,
        };
        let (tails, observation, _) = agree(&chain_program(LAST_FIELD, 100_000), "chain", grant);
        assert_eq!(tails, vec![TailKind::ConstructorHole(vec![1, 0])]);
        printed(&observation, "5000050000");
    });
}

/// Every failure-capable site of a short chain fails in H's campaign: each
/// partial result is released once, with no open block read.
#[test]
fn ctor_tail_chain_campaign_releases_each_partial_result_once() {
    with_compiler_stack(|| {
        let (tails, observation, _) = agree(&chain_program(LAST_FIELD, 12), "chain", SHORT);
        assert_eq!(tails, vec![TailKind::ConstructorHole(vec![1, 0])]);
        printed(&observation, "78");
    });
}

/// A binary tree whose right child is the constructor tail and whose left
/// child is an ordinary call, after OCaml 4.14's right-leaning `TreeMap`
/// (testsuite/tests/tmc/stack_space.ml) and Koka's ctail-tree.kk: 3,000 nodes,
/// a right spine past the activation limit, a left depth of a few levels.
const TREE: &str = r#"
type Tree = Leaf | Node(box Tree, u64, box Tree)
fn total(take t: box Tree) -> u64 {
  let mut cur = take t
  let mut acc = 0u64
  let mut going = true
  while going {
    match take cur {
      Leaf => {
        going = false
        cur = box Leaf
      }
      Node(l, v, r) => {
        acc = acc + v + total(take l)
        cur = take r
      }
    }
  }
  acc
}
fn build(lo: u64, hi: u64) -> Tree {
  if lo >= hi { Leaf } else { Node(box build(lo, lo + (hi - lo) / 512u64), lo + (hi - lo) / 512u64, box build(lo + (hi - lo) / 512u64 + 1u64, hi)) }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let t = box build(0u64, 3000u64)
  let _ = out.print(io.stdout(rt)?, u64.to_string(total(take t)))
  Ok(())
}
"#;

/// Reference steps per node of `TREE`, builder and consumer together: each
/// node evaluates three fields, an ordinary left call and a recursive
/// consumer call, several times a chain level's 124 (measured above).
const TREE_STEPS_PER_NODE: u64 = 600;

#[test]
fn ctor_tail_tree_builds_its_right_spine_in_one_activation() {
    with_compiler_stack(|| {
        let grant = Grant {
            steps: Some(3_000 * TREE_STEPS_PER_NODE),
            campaign: false,
        };
        let (tails, observation, consumed) = agree(TREE, "build", grant);
        eprintln!("tree 3000: {consumed} reference steps");
        assert_eq!(tails, vec![TailKind::ConstructorHole(vec![2, 0])]);
        printed(&observation, "4498500");
    });
}

/// The recursive operand before a later field is no constructor tail: the
/// 1,025th activation fails LANG-10 Depth on every engine.
#[test]
fn ctor_tail_later_field_counterexample_stops_at_the_limit() {
    with_compiler_stack(|| {
        let source = r#"
type Rev = Stop | Step(box Rev, u64)
fn rev(n: u64) -> Rev {
  if n == 0u64 { Stop } else { Step(box rev(n - 1u64), n) }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let r = box rev(2000u64)
  let _ = out.print(io.stdout(rt)?, "built")
  Ok(())
}
"#;
        let (tails, observation, _) = agree(source, "rev", DEEP);
        assert_eq!(tails, vec![]);
        failure(&observation, "LANG-10");
    });
}

/// A `let`-bound result and an `ensures` keep the call ordinary.
#[test]
fn ctor_tail_let_bound_and_ensures_variants_stay_ordinary() {
    with_compiler_stack(|| {
        for builder in [
            "fn chain(n: u64) -> Chain {\n  if n == 0u64 { End } else {\n    let rest = chain(n - 1u64)\n    Link(n, box rest)\n  }\n}",
            "fn chain(n: u64) -> Chain\n  ensures true {\n  if n == 0u64 { End } else { Link(n, box chain(n - 1u64)) }\n}",
        ] {
            let (tails, observation, _) = agree(&chain_program(builder, 2_000), "chain", DEEP);
            assert_eq!(tails, vec![], "{builder}");
            failure(&observation, "LANG-10");
        }
    });
}

/// An earlier field fails 1,500 levels deep, past the activation limit: the
/// partial result, its strings included, is released once, and every engine
/// reports the same failure site.
#[test]
fn ctor_tail_failure_mid_construction_releases_the_partial_result_once() {
    with_compiler_stack(|| {
        let source = r#"
type Names = Nil | Name(String, box Names)
fn names(n: u64) -> Names {
  if n == 0u64 { Nil } else { Name(u64.to_string(10u64 / (n - 2500u64)), box names(n - 1u64)) }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let ns = box names(4000u64)
  let _ = out.print(io.stdout(rt)?, "built")
  Ok(())
}
"#;
        let (tails, observation, _) = agree(source, "names", DEEP);
        assert_eq!(tails, vec![TailKind::ConstructorHole(vec![1, 0])]);
        assert_eq!(observation.exit_code, None, "{observation:?}");
        assert_eq!(observation.failures.len(), 1, "{observation:?}");
        assert_ne!(observation.failures[0].rule, "LANG-10", "{observation:?}");
    });
}
