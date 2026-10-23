//! A generic deep copy retains source bounds; replace checks receiving storage.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("replace-unique-copy-31.hir", source.into())
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
            hash_seed: 31,
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
        .prefix("replace-unique-copy-31-29-")
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

const WITNESS: &str = include_str!("../fixtures/replace-unique-copy-31/witness.hir");

fn source_check(source: &str, policy: LoweringPolicy) -> CheckOutput {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.hir");
    std::fs::write(&path, source).unwrap();
    std::fs::write(
        directory.path().join("main.hir.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "package": {"name": "witness", "version": "1"},
            "sources": [{"path": "main.hir", "module": []}], "dependencies": [],
            "entry": {"module": [], "name": "main"}
        }))
        .unwrap(),
    )
    .unwrap();
    load_program(&path).unwrap().check(policy).unwrap()
}

#[test]
fn replace_unique_copy_31_witness_names_copy_replace_and_removal() {
    with_compiler_stack(|| {
        for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
            let output = source_check(WITNESS, policy);
            let [diagnostic] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(diagnostic.rule.as_str(), "MODE-1");
            let replacement = "replace(mut state.current, take candidate)";
            assert_eq!(output.sources.slice(diagnostic.primary), Some(replacement));
            let at = output.sources.resolve(diagnostic.primary);
            assert_eq!(at.start as usize, WITNESS.find(replacement).unwrap());
            assert_eq!(
                at.end as usize,
                WITNESS.find(replacement).unwrap() + replacement.len()
            );
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(|label| output.sources.slice(label.span)
                        == Some("unique state.frontier[0u64]")
                        && label.message.contains("borrowed captures")),
                "{diagnostic:?}"
            );
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(|label| label.message.contains("caller")),
                "{diagnostic:?}"
            );
            assert!(
                diagnostic.notes.iter().any(|note| note
                    .contains("list.remove(mut state.frontier, 0u64)")
                    && note.contains("take candidate")),
                "{diagnostic:?}"
            );
            assert!(diagnostic.fix.is_none());
        }
    });
}

#[test]
fn replace_unique_copy_31_direct_field_also_keeps_generic_bounds() {
    with_compiler_stack(|| {
        let source = WITNESS.replace("unique state.frontier[0u64]", "unique state.current");
        for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
            let output = source_check(&source, policy);
            let [diagnostic] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(diagnostic.rule.as_str(), "MODE-1");
            assert_eq!(
                output.sources.slice(diagnostic.primary),
                Some("replace(mut state.current, take candidate)")
            );
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(
                        |label| output.sources.slice(label.span) == Some("unique state.current")
                            && label.message.contains("borrowed captures")
                    ),
                "{diagnostic:?}"
            );
        }
    });
}

#[test]
fn replace_unique_copy_31_remove_repair_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/MODE/MODE-1-replace-unique-copy-31-remove-run.hir"
        ))
    });
}

#[test]
fn replace_unique_copy_31_nongeneric_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/MODE/MODE-1-replace-unique-copy-31-nongeneric-run.hir"
        ))
    });
}
