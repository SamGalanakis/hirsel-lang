//! Peek loans survive their last use, then permit another mutable pull.
//!
//! Reuses std's existing Peekable port and its OWN-3 twins. Repeated peek/next
//! follows Rust coretests/tests/iter/adapters/peekable.rs:5-34 at f6c1107f
//! (MIT OR Apache-2.0), adapted to Hirsel's explicit fill and loan semantics.
use crate::rc_support::target;
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, run_observed_snapshot, with_compiler_stack};
use std::path::PathBuf;

const ORIGINAL: &str =
    include_str!("../../../../conformance/OWN/OWN-3-iter-peekable-7-scoped-run.hir");
const STEP: &str = r#"
fn step(mut n: u64) -> Option[List[u64]] {
  if n == 3 { return None() }
  n += 1
  Some([n])
}
"#;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/OWN/OWN-3-iter-peekable-7-scoped-run.hir")
}

fn matrix() -> Vec<Configuration> {
    let options = Options::new(target());
    vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: true },
        },
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ]
}

fn agree(name: &str, source: &str, failure: Option<&str>) {
    with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program_source(&fixture(), Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let entry = loaded.entry.clone();
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(
            run.observation
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: {run:?}")),
        );
        if let Some(rule) = failure {
            assert_eq!(reference.failures.len(), 1, "{name}: {reference:?}");
            assert_eq!(reference.failures[0].rule, rule, "{name}");
        } else {
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
        }
        eprintln!("{name}: H: {reference:?}");
        let raw = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(raw).unwrap();
        let configurations = matrix();
        let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("native-scoped-view-18")
            .join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let tools = Toolchain::for_matrix(&directory.join("runtime"), &configurations).unwrap();
        let outcomes = differential(
            &input,
            &entry,
            &reference,
            &configurations,
            &tools,
            &directory,
        );
        assert_eq!(outcomes.len(), 3, "{name}");
        for (configuration, outcome) in &outcomes {
            eprintln!("{name}: {configuration}: {outcome:?}");
        }
        for (configuration, outcome) in outcomes {
            assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
        }
    });
}

#[test]
fn original_scoped_view_on_recorded_native_configurations() {
    agree("original", ORIGINAL, None);
}

#[test]
fn repeated_peeks_end_before_consuming_and_refilling() {
    let source = format!(
        r#"{STEP}
pub fn main(t: Test) -> Result[(), ()] {{
  let mut p = std@1/peekable.new(iter.from(0u64, step))
  for n in 1u64..4u64 {{
    failure.check(std@1/peekable.fill(mut p), Bounds())
    let first = std@1/peekable.peek(p)
    let second = std@1/peekable.peek(p)
    failure.check(first == Some([n]) && second == first, Bounds())
    failure.check(std@1/peekable.next(mut p) == Some([n]), Bounds())
  }}
  failure.check(!std@1/peekable.fill(mut p), Bounds())
  failure.check(std@1/peekable.peek(p) == None(), Bounds())
  failure.check(std@1/peekable.peek(p) == None(), Bounds())
  failure.check(std@1/peekable.next(mut p) == None(), Bounds())
  Ok(())
}}
"#
    );
    agree("repeated", &source, None);
}

#[test]
fn nested_callback_keeps_view_until_its_last_use() {
    let source = format!(
        r#"{STEP}
fn invoke(f: fn() -> bool uses {{}}) -> bool uses {{f}} {{ f() }}
fn inspect(view: Option[List[u64]]) -> bool {{
  invoke(fn() -> bool uses {{}} {{
    invoke(fn() -> bool uses {{}} {{ view == Some([1u64]) }})
  }})
}}
pub fn main(t: Test) -> Result[(), ()] {{
  let mut p = std@1/peekable.new(iter.from(0u64, step))
  let _ = std@1/peekable.fill(mut p)
  let view = std@1/peekable.peek(p)
  failure.check(inspect(view), Bounds())
  failure.check(std@1/peekable.next(mut p) == Some([1u64]), Bounds())
  Ok(())
}}
"#
    );
    agree("nested", &source, None);
}

#[test]
fn early_return_cleans_view_and_other_arm_can_consume_after_last_use() {
    let source = format!(
        r#"{STEP}
fn early(stop: bool) -> bool {{
  let mut p = std@1/peekable.new(iter.from(0u64, step))
  let _ = std@1/peekable.fill(mut p)
  let view = std@1/peekable.peek(p)
  if stop {{ return view == Some([1u64]) }}
  let present = view == Some([1u64])
  present && std@1/peekable.next(mut p) == Some([1u64])
}}
pub fn main(t: Test) -> Result[(), ()] {{
  failure.check(early(true) && early(false), Bounds())
  Ok(())
}}
"#
    );
    agree("early-return", &source, None);
}

#[test]
fn callback_failure_hands_back_mutable_state_after_prior_view_ended() {
    let source = r#"
resource type State { n: u64 }
drop(state) { failure.check(state.n == 2u64, Bounds()) }
fn step(mut state: State) -> Option[List[u64]] {
  state.n += 1
  if state.n == 2 { let zero = 0u64; let _ = 1u64 / zero }
  Some([state.n])
}
pub fn main(t: Test) -> Result[(), ()] {
  let mut p = std@1/peekable.new(iter.from(State(n: 0u64), step))
  let _ = std@1/peekable.fill(mut p)
  let view = std@1/peekable.peek(p)
  failure.check(view == Some([1u64]), Bounds())
  failure.check(std@1/peekable.next(mut p) == Some([1u64]), Bounds())
  let _ = std@1/peekable.fill(mut p)
  Ok(())
}
"#;
    agree("callback-failure", source, Some("SEM-1"));
}

#[test]
fn still_live_peek_views_reject_mutable_fill_and_consume() {
    for source in [
        include_str!("../../../../conformance/OWN/OWN-3-iter-peekable-7-fill-borrowed-bad.hir"),
        include_str!("../../../../conformance/OWN/OWN-3-iter-peekable-7-next-borrowed-bad.hir"),
    ] {
        with_compiler_stack(|| {
            let mut loaded = hirsel_driver::load_program_source(&fixture(), Some(source)).unwrap();
            let error = loaded.check_for_execution().unwrap_err();
            assert!(format!("{error:?}").contains("MODE-1"), "{error:?}");
        });
    }
}
