use hirsel_driver::*;
use hirsel_engine::*;

const MINIMAL: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(s: Scope) {
    let mut pending: TaskSet[unique Result[String, String]] of s = task.set(s)
    task.add(s, mut pending, task.async(s, fn() -> unique Result[String, String] { unique Ok("a") }))
    let joined = task.await_at(s, mut pending, 0u64)
    match take joined { Ok(row) => {}, Error(problem) => {} }
    task.cancel_all(s, take pending)
  })
  Ok(())
}
"#;

#[test]
fn taskset_await_mode_27_minimal_exports() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("taskset-await.hir", MINIMAL.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        engine.export_native_input().unwrap();
    });
}

fn probe(source: &str) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("probe.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine.export_native_input().unwrap();
}

#[test]
fn taskset_await_mode_27_shrunk() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[unique Result[String,String]] of s = task.set(s); let joined = task.await_at(s, mut pending, 0u64); match take joined { Ok(row)=>{}, Error(problem)=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_plain() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[Result[String,String]] of s = task.set(s); let joined = task.await_at(s, mut pending, 0u64); match take joined { Ok(row)=>{}, Error(problem)=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_borrowed_match() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[unique Result[String,String]] of s = task.set(s); let joined = task.await_at(s, mut pending, 0u64); match joined { Ok(row)=>{}, Error(problem)=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_await_any() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[unique Result[String,String]] of s = task.set(s); let slot = match task.await_any(s,pending) {Some(i)=>i,None()=>0u64}; let joined = task.await_at(s, mut pending, slot); match take joined { Ok(row)=>{}, Error(problem)=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_option() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[unique Option[String]] of s = task.set(s); let joined = task.await_at(s, mut pending, 0u64); match take joined { Some(row)=>{}, None()=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_direct() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s:Scope) {let child=task.async(s,fn()->unique Result[String,String]{unique Ok("a")}); let joined=task.await(s,take child); match take joined {Ok(row)=>{},Error(problem)=>{}}}"#,
        )
    });
}

#[test]
fn taskset_await_mode_27_direct_call_match() {
    with_compiler_stack(|| {
        probe(
            r#"fn probe(s: Scope) { let mut pending: TaskSet[unique Result[String,String]] of s = task.set(s); match take task.await_at(s, mut pending, 0u64) { Ok(row)=>{}, Error(problem)=>{} }; task.cancel_all(s,take pending) }"#,
        )
    });
}

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("taskset-await.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}
fn runtime(source: &str) {
    use hirsel_driver::native::*;
    let mut engine = checked(source);
    let expected = Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 27,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(run.observation.as_ref().unwrap()),
        expected,
        "interpreter"
    );
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "checked H");
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let matrix = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
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
    ];
    let scratch = tempfile::Builder::new()
        .prefix("taskset-await-mode-27-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

#[test]
fn taskset_await_mode_27_runtime_main() {
    with_compiler_stack(|| runtime(include_str!("../fixtures/taskset_await_mode_27/main.hir")));
}

#[test]
fn taskset_await_mode_27_runtime_minimal() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../fixtures/taskset_await_mode_27/minimal.hir"
        ))
    });
}

#[test]
fn taskset_await_mode_27_runtime_set_await() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../fixtures/taskset_await_mode_27/set-await.hir"
        ))
    });
}

#[test]
fn taskset_await_mode_27_runtime_minimal_await() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../fixtures/taskset_await_mode_27/minimal-await.hir"
        ))
    });
}

#[test]
fn taskset_await_mode_27_runtime_typed_payload() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../fixtures/taskset_await_mode_27/typed-payload.hir"
        ))
    });
}

#[test]
fn taskset_await_mode_27_runtime_matrix() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/CONC/CONC-2-taskset-await-mode-27-run.hir"
        ))
    });
}

#[test]
fn taskset_await_mode_27_declared_and_current_uniqueness() {
    with_compiler_stack(|| {
        // An ordinary bare-R result weakens the root. A receiving binder
        // must not recover the earlier generic actual's deep uniqueness.
        let bad = MINIMAL.replace("Ok(row) => {}", "Ok(row) => { need(take row) }");
        let source = format!("fn need(take row: unique String) {{}}\n{bad}");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("receiving.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-2"),
            "{:?}",
            checked.diagnostics
        );
        // A declared unique owner still supplies deep uniqueness, and a
        // nonunique receipt preserves an explicitly unique payload field.
        probe(
            r#"fn need(take row: unique String) {}
fn make() -> unique Result[String, String] { unique Ok("a") }
fn receive() { match take make() { Ok(row) => need(take row), Error(problem) => need(take problem) } }
fn explicit(s: Scope) {
  let mut set: TaskSet[Result[unique String, unique String]] of s = task.set(s)
  let joined = task.await_at(s, mut set, 0u64)
  match take joined { Ok(row) => need(take row), Error(problem) => need(take problem) }
  task.cancel_all(s, take set)
}"#,
        );
    });
}

#[test]
fn taskset_await_mode_27_v_rejects_forged_arm_modes() {
    use hirsel_tir::{InstructionKind, Rvalue, Storage};
    use hirsel_types::{Primitive, TypeKind, Uniqueness};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let mut engine = checked(MINIMAL);
        let input = engine.export_native_input().unwrap();
        let path = entry_path(&default_context());
        let original = &input.definitions[&path];
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        hirsel_validate::assert_valid(original, signatures.types(), &signatures);
        let body = &original.bodies[1];
        let destinations: Vec<_> = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                let Rvalue::Use(source) = &body.rvalues[value.0 as usize] else {
                    return None;
                };
                let target = &body.places[destination.0 as usize];
                (body.locals[target.root.0 as usize].storage == Storage::Local
                    && !body.places[source.place.0 as usize].projection.is_empty()
                    && matches!(
                        input.types.get(target.ty).unwrap().kind,
                        TypeKind::Primitive(Primitive::String)
                    ))
                .then_some(target.root)
            })
            .collect();
        assert_eq!(
            destinations.len(),
            2,
            "both match arms must be forged independently"
        );
        for root in destinations {
            let mut forged = (**original).clone();
            let body = &mut forged.bodies[1];
            let mut row = input
                .types
                .get(body.locals[root.0 as usize].ty)
                .unwrap()
                .clone();
            assert_eq!(row.modes.uniqueness, Uniqueness::Aliased);
            row.modes.uniqueness = Uniqueness::Unique;
            let ty = input.types.intern(row);
            body.locals[root.0 as usize].ty = ty;
            for place in &mut body.places {
                if place.root == root {
                    place.ty = ty;
                }
            }
            forged.types = hirsel_tir::definition_types(&input.types, &forged);
            let report =
                hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![]);
            assert!(!report.violations.is_empty());
            assert!(
                report.violations.iter().all(|v| v.kind
                    == ViolationKind::TypeMismatch {
                        entity: "value modes"
                    }),
                "{:?}",
                report.violations
            );
        }
    });
}
