//! LANG-13/DROP-2/FAIL-3 oracle, independent of the cleanup planner.
//! Prior art: Rust struct-field-drop-order.rs (afe686de), lines 1–69, and
//! alloctests/tests/vec.rs (f6c1107f), lines 350–376. No Rust code is copied.
//! Hirsel omissions use declaration order; ordinary fields reverse it. Hirsel
//! extends the failure chain and continues cleanup instead of double-panic abort.
use crate::rc_support::target;
use hirsel_driver::native::*;
use hirsel_engine::*;
use std::collections::BTreeMap;

const SOURCE: &str = r#"resource type First { stdout: Stdout, text: String, zero: u64, parent_zero: u64, payload: box u64 }
drop(x) { let _ = out.print(x.stdout, x.text); let _ = 1u64 / x.zero }
resource type Middle { stdout: Stdout, text: String, zero: u64, payload: box u64 }
drop(x) { let _ = out.print(x.stdout, x.text); let _ = 1u64 / x.zero }
resource type Last { stdout: Stdout, text: String, zero: u64, payload: box u64 }
drop(x) { let _ = out.print(x.stdout, x.text); let _ = 1u64 / x.zero }
resource type Parent { first: First, middle: Middle, last: Last }
drop(x) { let _ = out.print(x.first.stdout, "parent\n"); let _ = 1u64 / x.first.parent_zero }
fn primary(zero: u64) { let _ = 1u64 / zero }
fn flag(e: Env, name: String) -> bool { env.get(e, name) == Ok(Some("1")) }
fn denominator(e: Env, name: String) -> u64 { if flag(e, name) { 0 } else { 1 } }
fn bind_first(take parent: Parent, mode: u64) -> Result[(), ()] {
  let Parent(first: kept, ..) = take parent
  let _ = out.print(kept.stdout, "body\n")
  if mode == 1 { return Ok(()) }
  if mode == 2 { let r: Result[(), ()] = Error(()); r? }
  if mode == 3 { primary(0) }
  Ok(())
}
fn bind_middle(take parent: Parent, mode: u64) -> Result[(), ()] {
  let Parent(middle: kept, ..) = take parent
  let _ = out.print(kept.stdout, "body\n")
  if mode == 1 { return Ok(()) }
  if mode == 2 { let r: Result[(), ()] = Error(()); r? }
  if mode == 3 { primary(0) }
  Ok(())
}
fn bind_last(take parent: Parent, mode: u64) -> Result[(), ()] {
  let Parent(last: kept, ..) = take parent
  let _ = out.print(kept.stdout, "body\n")
  if mode == 1 { return Ok(()) }
  if mode == 2 { let r: Result[(), ()] = Error(()); r? }
  if mode == 3 { primary(0) }
  Ok(())
}
pub fn main(rt: Io, e: Env) -> Result[(), ()] {
  let stdout = match io.stdout(rt) { Ok(s) => s, Error(_) => return Error(()) }
  let parent = Parent(
    first: First(stdout: stdout, text: "first\n", zero: denominator(e, "first"), parent_zero: denominator(e, "parent"), payload: box 11u64),
    middle: Middle(stdout: stdout, text: "middle\n", zero: denominator(e, "middle"), payload: box 22u64),
    last: Last(stdout: stdout, text: "last\n", zero: denominator(e, "last"), payload: box 33u64))
  if flag(e, "uncommitted") {
    match take parent {
      Parent(first: kept, ..) if false => (),
      _ => ()
    }
    Ok(())
  } else {
    let mode: u64 = if flag(e, "return") { 1 } else { if flag(e, "question") { 2 } else { if flag(e, "failure") { 3 } else { 0 } } }
    if flag(e, "bind_first") { bind_first(take parent, mode) } else {
      if flag(e, "bind_last") { bind_last(take parent, mode) } else { bind_middle(take parent, mode) }
    }
  }
}
"#;
const NAMES: [&str; 3] = ["first", "middle", "last"];

#[derive(Clone, Copy, Debug)]
struct Case {
    bound: usize,
    failures: u8,
    mode: usize,
    committed: bool,
    parent_fails: bool,
}
impl Case {
    fn env(self) -> BTreeMap<String, Option<String>> {
        let mut env = BTreeMap::new();
        for (index, name) in NAMES.iter().enumerate() {
            env.insert(
                (*name).into(),
                Some(
                    if self.failures & (1 << index) != 0 {
                        "1"
                    } else {
                        "0"
                    }
                    .into(),
                ),
            );
        }
        for (name, yes) in [
            ("bind_first", self.bound == 0),
            ("bind_last", self.bound == 2),
            ("return", self.mode == 1),
            ("question", self.mode == 2),
            ("failure", self.mode == 3),
            ("uncommitted", !self.committed),
            ("parent", self.parent_fails),
        ] {
            env.insert(name.into(), Some(if yes { "1" } else { "0" }.into()));
        }
        env
    }
    // Written directly from the rules: omit in declaration order, then the
    // bound owner; an unselected guard leaves the intact parent and its fields.
    fn expected(self) -> (String, Vec<u32>, Option<i32>) {
        let order: Vec<usize> = if self.committed {
            (0..3)
                .filter(|&i| i != self.bound)
                .chain([self.bound])
                .collect()
        } else {
            vec![2, 1, 0]
        };
        let mut markers = String::new();
        let mut causes = vec![];
        if !self.committed {
            markers.push_str("parent\n");
            if self.parent_fails {
                causes.push(8);
            }
        }
        let omission_failed = self.committed && (self.failures & !(1 << self.bound)) != 0;
        for (position, &field) in order.iter().enumerate() {
            if self.committed && position == 2 && !omission_failed {
                markers.push_str("body\n");
                if self.mode == 3 {
                    causes.push(9);
                }
            }
            markers.push_str(NAMES[field]);
            markers.push('\n');
            if self.failures & (1 << field) != 0 {
                causes.push(2 + 2 * field as u32);
            }
        }
        let exit = if causes.is_empty() {
            Some(i32::from(self.committed && self.mode == 2))
        } else {
            None
        };
        (markers, causes, exit)
    }
}
fn cases() -> Vec<Case> {
    let mut cases = vec![];
    // Every mask includes no omission failure, first, second and both, with
    // either state of the bound finalizer: 3 × 8 × 4 = 96 committed vectors.
    for bound in 0..3 {
        for failures in 0..8 {
            for mode in 0..4 {
                cases.push(Case {
                    bound,
                    failures,
                    mode,
                    committed: true,
                    parent_fails: true,
                });
            }
        }
    }
    for failures in [0, 1, 4, 7] {
        for parent_fails in [false, true] {
            cases.push(Case {
                bound: 0,
                failures,
                mode: 0,
                committed: false,
                parent_fails,
            });
        }
    }
    cases
}
fn assert_oracle(case: Case, observation: &Observation) {
    let (markers, causes, exit) = case.expected();
    assert_eq!(
        observation.output,
        markers.as_bytes(),
        "{case:?}: {observation:?}"
    );
    assert_eq!(observation.exit_code, exit, "{case:?}");
    assert!(!observation.blocked && observation.budget.is_none() && observation.signaled.is_none());
    assert_eq!(
        observation
            .failures
            .iter()
            .map(|f| f.line)
            .collect::<Vec<_>>(),
        causes,
        "{case:?}: {observation:?}"
    );
    assert!(observation.failures.iter().all(|f| f.rule == "SEM-1"));
}

#[test]
fn bounded_order_and_failure_chain_on_h() {
    with_compiler_stack(|| {
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine
            .sources
            .add("destructure.hir", SOURCE.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let raw = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(raw).unwrap();
        let sites = Sites::new(validated.snapshot());
        let ready = build(&validated, &Options::new(target())).unwrap();
        let entry = entry_path(&default_context());
        for case in cases() {
            let run = run_heap_in(
                &ready,
                &entry,
                &sites,
                &HeapWorld {
                    env: Some(case.env()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_oracle(case, &run.observation);
            assert_eq!(run.counters.live_heap_bytes, 0, "{case:?}");
            assert_eq!(
                run.counters.finalizers,
                if case.committed { 3 } else { 4 },
                "{case:?}"
            );
        }
        eprintln!("destructure_failure_order_17: 104 H vectors, no leaks or duplicate finalizers");
    });
}

#[test]
fn bounded_order_and_failure_chain_on_both_backends() {
    use std::os::unix::process::ExitStatusExt;
    with_compiler_stack(|| {
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine
            .sources
            .add("destructure.hir", SOURCE.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let raw = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(raw).unwrap();
        let sites = Sites::new(validated.snapshot());
        let ready = build(&validated, &Options::new(target())).unwrap();
        let entry = entry_path(&default_context());
        let directory = tempfile::tempdir().unwrap();
        let toolchain =
            Toolchain::in_tree_diagnostic(&directory.path().join("runtime"), &[&target()]).unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = directory.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            std::fs::create_dir_all(dir.join("root")).unwrap();
            let executable = link_objects(&ready, &entry, &objects, &toolchain, &dir).unwrap();
            // One executable per backend, reused by every environment vector.
            for case in cases() {
                let record = dir.join("observation");
                let refusal = dir.join("refusal");
                let _ = std::fs::remove_file(&refusal);
                let _ = std::fs::remove_file(&record);
                let output = std::process::Command::new(&executable)
                    .env_clear()
                    .env("HIRSEL_RT_ENV", serde_json::to_string(&case.env()).unwrap())
                    .env("HIRSEL_RT_ROOT", dir.join("root"))
                    .env("HIRSEL_RT_REFUSAL", &refusal)
                    .env("HIRSEL_RT_OBSERVATION", &record)
                    .env("HIRSEL_RT_DIAGNOSTIC", "1")
                    .output()
                    .unwrap();
                let record = std::fs::read_to_string(&record)
                    .or_else(|_| std::fs::read_to_string(&refusal))
                    .unwrap_or_else(|error| {
                        panic!(
                            "{case:?}: missing observation {error}; exit {:?}; stderr {:?}",
                            output.status,
                            String::from_utf8_lossy(&output.stderr)
                        )
                    });
                let launched = Launched {
                    audit_required: true,
                    exit: output.status.code(),
                    signal: output.status.signal(),
                    stdout: output.stdout,
                    stderr: output.stderr,
                    record,
                };
                let observation = observe(&launched, &ready, &sites).unwrap();
                assert_oracle(case, &observation);
            }
            eprintln!(
                "destructure_failure_order_17: {backend:?}: 104 vectors, diagnostic exit audit"
            );
        }
    });
}

/// Regression: both outcomes of a taken guard must cross a join with the
/// committed shell dead, or Ready validation sees possibly moved parent parts.
#[test]
fn guarded_commit_retires_the_empty_parent_before_joining() {
    for selected in [false, true] {
        let source = format!(
            r#"resource type Parent {{ n: u64 }}
drop(x) {{ () }}
pub fn main() -> Result[(), ()] {{
  match Parent(n: 1) {{ Parent(n: _) if {selected} => (), _ => () }}
  Ok(())
}}
"#
        );
        with_compiler_stack(|| {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine.sources.add("guard.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let raw = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(raw).unwrap();
            let sites = Sites::new(validated.snapshot());
            let ready = build(&validated, &Options::new(target())).unwrap();
            let run = run_heap(&ready, &entry_path(&default_context()), &sites).unwrap();
            assert_eq!(run.observation.exit_code, Some(0));
            assert_eq!(run.counters.finalizers, u64::from(!selected));
            assert_eq!(run.counters.live_heap_bytes, 0);
        });
    }
}
