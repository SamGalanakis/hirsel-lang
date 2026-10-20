//! Iterator closures are checked through the production pipeline and failure audits.
use crate::rc_support as support;
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use support::*;

/// std@1/iter.map's source: these programs compile without std.
const MAP: &str = r#"type Mapped[T, U] { inner: Iter[T], f: fn(T) -> U }
@captures(it, f)
fn map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f} {
  iter.from(take Mapped(inner: take it, f: take f), fn(mut s: Mapped[T, U]) -> Option[U] {
    match iter.next(mut s.inner) { Some(item) => Some(s.f(item)), None => None }
  })
}
"#;

/// A program using `iter.map`, with std's map spelled in its own source.
fn with_map(source: &str) -> String {
    format!("{MAP}{}", source.replace("iter.map(", "map("))
}
fn observed_heap(source: &str) -> hirsel_driver::native::HeapRun {
    let source = with_map(source);
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let entry = entry_path(&default_context());
        let config = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let reference = hirsel_driver::native::Observation::of_reference(
            run.observation
                .as_ref()
                .unwrap_or_else(|| panic!("{source}\n{run:?}")),
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let sites = hirsel_driver::native::Sites::new(validated.snapshot());
        let ready = hirsel_driver::native::build(
            &validated,
            &hirsel_driver::native::Options::new(target()),
        )
        .unwrap_or_else(|e| panic!("{source}\n{e}"));
        let heap = hirsel_driver::native::heap_campaign(&ready, &entry, &sites)
            .unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(heap.observation, reference, "{source}");
        heap
    })
}

#[test]
fn a_mapped_iterator_calls_its_mapper_after_pulling_each_item() {
    let source = r#"fn step(mut n: u64) -> Option[u64] { if n == 3 { None } else { let x = n; n += 1; Some(x) } }
fn double(x: u64) -> u64 { x * 2 }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s: u64 = 0
  let it = iter.from(take s, step)
  let mut mapped = iter.map(take it, double)
  let mut sum: u64 = 0
  for x in take mapped { sum += x }
  let _ = out.print(io.stdout(rt)?, if sum == 6 { "ok\n" } else { "bad\n" })
  Ok(())
}
"#;
    assert_eq!(observed_heap(source).observation.output, b"ok\n");
}

fn resource_iterator(action: &str, callback_failure: bool) -> String {
    let fail = if callback_failure {
        "let z: u64 = 0; let _ = 1 / z"
    } else {
        ""
    };
    format!(
        r#"resource type Trace {{ stdout: Stdout, text: String }}
drop(x) {{ let _ = out.print(x.stdout, x.text) }}
fn step(mut state: Trace) -> Option[u64] {{
  state.text = "updated\n"
  {fail}
  Some(1)
}}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let s = io.stdout(rt)?
  let state = Trace(stdout: s, text: "initial\n")
  let mut it = iter.from(take state, step)
  {action}
  Ok(())
}}
"#
    )
}

#[test]
fn unconsumed_iterators_release_their_captured_state() {
    assert_eq!(
        observed_heap(&resource_iterator("", false))
            .observation
            .output,
        b"initial\n"
    );
}

#[test]
fn a_break_releases_the_updated_captured_state() {
    assert_eq!(
        observed_heap(&resource_iterator("for x in mut it { break }", false))
            .observation
            .output,
        b"updated\n"
    );
}

#[test]
fn a_failed_callback_writes_back_state_before_iterator_cleanup() {
    let run = observed_heap(&resource_iterator("let _ = iter.next(mut it)", true));
    assert_eq!(run.observation.output, b"updated\n");
    assert_eq!(run.observation.failures[0].rule, "SEM-1");
}

fn mapped_resource() -> &'static str {
    r#"resource type Trace { stdout: Stdout, text: String }
drop(x) { let _ = out.print(x.stdout, x.text) }
fn step(mut state: Trace) -> Option[Trace] {
  Some(Trace(stdout: state.stdout, text: "pulled\n"))
}
fn mapper(x: Trace) -> u64 { let z: u64 = 0; 1 / z }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s = io.stdout(rt)?
  let state = Trace(stdout: s, text: "remaining\n")
  let inner = iter.from(take state, step)
  let mut mapped = iter.map(take inner, mapper)
  let _ = iter.next(mut mapped)
  Ok(())
}
"#
}
#[test]
fn mapped_items_finalize_before_the_remaining_state_after_mapper_failure() {
    assert_eq!(
        observed_heap(mapped_resource()).observation.output,
        b"pulled\nremaining\n"
    );
}

#[test]
fn every_owned_corpus_iterator_runs_with_failure_audits() {
    for relative in [
        "FAIL/FAIL-3-iterator-callback-mut-state-bad.hir",
        "FAIL/FAIL-3-iterator-callback-mut-state-ok.hir",
        "FAIL/FAIL-3-mapped-iterator-callback-mut-state-bad.hir",
        "FAIL/FAIL-3-mapped-iterator-callback-mut-state-ok.hir",
        "MODE/MODE-4-let-relation-iter-advanced-run.hir",
        "MODE/MODE-4-let-relation-iter-callback-run.hir",
        "MODE/MODE-4-let-relation-iter-closure-run.hir",
        "MODE/MODE-4-let-relation-iter-map-run.hir",
        "MODE/MODE-4-let-relation-iter-mut-binding-run.hir",
        "MODE/MODE-4-let-relation-iter-state-run.hir",
        "MODE/MODE-4-let-relation-mut-callback-run.hir",
        "SEM/SEM-4-iterator-mut.hir",
        "SEM/SEM-4-iterator-resume-after-break.hir",
        "SEM/SEM-4-iterator-take.hir",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance")
            .join(relative);
        observed_heap(&std::fs::read_to_string(path).unwrap());
    }
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn iterator_resource_cleanup_agrees_on_both_targets_and_all_native_configurations() {
    use hirsel_driver::native::*;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("iter-native");
    let toolchain = Toolchain::for_matrix(&dir.join("runtime"), &full_matrix()).unwrap();
    for (name, source) in [
        ("unconsumed", resource_iterator("", false)),
        (
            "break",
            resource_iterator("for x in mut it { break }", false),
        ),
        (
            "callback-failure",
            resource_iterator("let _ = iter.next(mut it)", true),
        ),
        ("mapped-failure", mapped_resource().to_owned()),
        (
            "mapped-success",
            mapped_resource().replace("let z: u64 = 0; 1 / z", "1"),
        ),
        ("owned-result", mapped_owned_result(false)),
        ("owned-result-drop-failure", mapped_owned_result(true)),
    ] {
        let source = with_map(&source);
        let directory = dir.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let toolchain = &toolchain;
        with_compiler_stack(move || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("test.hir", source.clone()).unwrap();
            check_progress(&mut engine, file, &default_context()).unwrap();
            let entry = entry_path(&default_context());
            let config = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            for (configuration, outcome) in differential(
                &input,
                &entry,
                &reference,
                &full_matrix(),
                toolchain,
                &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}\n{source}"
                );
            }
        });
    }
}

#[test]
fn iterator_construction_and_steps_keep_their_indirect_lir_shape() {
    use hirsel_lir::ir::{ArgumentValue, Callee, CaptureUse, Inst, Terminator, UnitOrigin};
    let source = include_str!("../../../../conformance/MODE/MODE-4-let-relation-iter-map-run.hir");
    let lowered = hirsel_rc::import::lower(&compile(&with_map(source)), &target()).unwrap();
    // The constructing units: main's own `iter.from`, then one per `map` instance.
    let mut closures = Vec::new();
    for unit in &lowered.units {
        for inst in &unit.instructions {
            if let Inst::Closure {
                closure, result, ..
            } = inst
            {
                let layout = unit.types[unit.values[result.index()].ty.index()].layout;
                if matches!(
                    lowered.table.layouts[layout.0 as usize].kind,
                    hirsel_lir::layout_types::LayoutKind::Iterator { .. }
                ) {
                    let captures = unit.tables.closures[closure.index()]
                        .captures
                        .get(&unit.pools.captures)
                        .unwrap();
                    assert_eq!(captures.len(), 2);
                    assert!(captures.iter().all(|c| c.usage == CaptureUse::Take));
                    closures.push(unit.header.definition.0.name.to_string());
                }
            }
        }
        if unit.header.origin == UnitOrigin::Library {
            assert!(
                unit.tables
                    .calls
                    .iter()
                    .all(|c| matches!(c.callee, Callee::Indirect(_)))
            );
            if unit.header.definition.0.name == "iter_from_step" {
                assert_eq!(unit.tables.calls.len(), 1);
                let args = unit.tables.calls[0]
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap();
                assert!(matches!(args[0].value, ArgumentValue::Address { .. }));
            }
        }
        for terminator in &unit.terminators {
            if let Terminator::Call { call, .. } = terminator {
                assert!(
                    !matches!(unit.tables.calls[call.index()].callee, Callee::Operation(ref op) if unit.tables.operations[op.index()].definition.0.modules == ["iter"])
                );
            }
        }
    }
    closures.sort();
    assert_eq!(closures[0], "main", "{closures:?}");
    assert!(
        closures.len() >= 3 && closures[1..].iter().all(|name| name == "map"),
        "{closures:?}"
    );
}

fn mapped_owned_result(failing_drop: bool) -> String {
    let source = mapped_resource().replace(
        "fn mapper(x: Trace) -> u64 { let z: u64 = 0; 1 / z }",
        "fn mapper(x: Trace) -> Trace { Trace(stdout: x.stdout, text: \"mapped\\n\") }",
    );
    if failing_drop {
        source.replace(
            "drop(x) { let _ = out.print(x.stdout, x.text) }",
            "drop(x) { let _ = out.print(x.stdout, x.text); if x.text == \"pulled\\n\" { let z: u64 = 0; let _ = 1 / z } }",
        )
    } else {
        source
    }
}

#[test]
fn a_failed_pulled_item_drop_cleans_the_prepared_owned_mapper_result() {
    for failing in [false, true] {
        let run = observed_heap(&mapped_owned_result(failing));
        assert_eq!(run.observation.output, b"pulled\nmapped\nremaining\n");
        assert_eq!(run.observation.failures.is_empty(), !failing);
    }
}

#[test]
fn iterator_state_finalization_follows_yielded_item_finalization() {
    let source = mapped_resource()
        .replace("fn mapper(x: Trace) -> u64 { let z: u64 = 0; 1 / z }", "")
        .replace("let inner = iter.from", "let mut inner = iter.from")
        .replace("let mut mapped = iter.map(take inner, mapper)", "")
        .replace("iter.next(mut mapped)", "iter.next(mut inner)");
    assert_eq!(
        observed_heap(&source).observation.output,
        b"pulled\nremaining\n"
    );
}

#[test]
fn mapped_resource_cleanup_has_the_corpus_observation() {
    let source = include_str!(
        "../../../../conformance/DROP/DROP-2-iterator-mapped-resource-cleanup-run.hir"
    );
    assert_eq!(
        observed_heap(source).observation.output,
        b"pulled\nmapped\nremaining\n"
    );
}

#[test]
fn conditionally_created_callable_arrays_keep_ordered_cleanup() {
    let source = include_str!("../../../../conformance/LANG/LANG-11-block-captured-value-run.hir");
    assert_eq!(observed_heap(source).observation.output, b"ok\n");
}

#[test]
fn a_once_closure_can_move_an_erased_iterator_capture() {
    let source =
        include_str!("../../../../conformance/CLO/CLO-2-mut-handback-erased-state-run.hir");
    assert_eq!(observed_heap(source).observation.output, b"printed\n");
}
