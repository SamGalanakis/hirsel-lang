//! Counted items moved from unique consuming cursors remain unique.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("unique-empty-builder.hir", source.into())
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
            hash_seed: 29,
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
        .prefix("unique-empty-builder-29-")
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
fn unique_empty_builder_29_minimal_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-minimal-run.hir"
        ))
    });
}

#[test]
fn unique_empty_builder_29_witness_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-witness-run.hir"
        ))
    });
}

#[test]
fn unique_empty_builder_29_matrix_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-matrix-run.hir"
        ))
    });
}

#[test]
fn unique_empty_builder_29_nearest_twins_still_reject() {
    with_compiler_stack(|| {
        for (source, expected_rules, primary) in [
            (
                include_str!(
                    "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-aliased-source-bad.hir"
                ),
                &["MODE-2"][..],
                "unique []",
            ),
            (
                include_str!(
                    "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-shared-source-bad.hir"
                ),
                &["MODE-2"][..],
                "unique []",
            ),
            (
                include_str!(
                    "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-copied-item-bad.hir"
                ),
                &["MODE-2"][..],
                "unique []",
            ),
            (
                include_str!(
                    "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-borrowed-source-bad.hir"
                ),
                &["MODE-2", "MODE-1"][..],
                "unique []",
            ),
            (
                include_str!(
                    "../../../../conformance/MODE/MODE-2-unique-empty-builder-29-nonfresh-initializer-bad.hir"
                ),
                &["MODE-2"][..],
                "copy source",
            ),
        ] {
            // Rejected programs must agree at source checking and the reachable
            // lowering used by execution/build. No engine may receive their TIR.
            for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("main.hir");
                std::fs::write(&path, source).unwrap();
                let manifest = directory.path().join("hirsel.json");
                crate::image_fixture::core_manifest(&path, &manifest);
                let mut program = load_program(&manifest).unwrap();
                let output = program.check(policy).unwrap();
                let errors: Vec<_> = output
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == hirsel_diag::Severity::Error)
                    .collect();
                assert_eq!(
                    errors.iter().map(|d| d.rule.as_str()).collect::<Vec<_>>(),
                    expected_rules,
                    "{source}\n{errors:?}"
                );
                assert_eq!(output.sources.slice(errors[0].primary), Some(primary));
                let resolved = output.sources.resolve(errors[0].primary);
                let start = source.find(primary).unwrap() as u32;
                assert_eq!(resolved.start, start);
                assert_eq!(resolved.end, start + primary.len() as u32);
            }
        }
    });
}

#[test]
fn unique_empty_builder_29_v_rejects_forged_unique_cursor_results() {
    with_compiler_stack(|| {
        for source in [
            // Taking an aliased collection cannot make its items unique.
            "fn build(take source: List[String]) -> List[String] { let mut values: List[String] = []; for item in take source { list.push(mut values, take item) }; take values }",
            // A share made before consuming defeats the owner's original proof.
            "fn build(take source: unique List[String]) -> List[String] { let alias = copy source; let mut values: List[String] = []; for item in take source { list.push(mut values, take item) }; take values }",
            // Copying a yielded item also loses its current uniqueness.
            "fn build(take source: unique List[String]) -> List[String] { let mut values: List[String] = []; for item in take source { list.push(mut values, copy item) }; take values }",
        ] {
            let mut engine = checked(source);
            let input = engine.export_native_input().unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "build".into();
            let mut forged = (*input.definitions[&path]).clone();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let signature = signatures.get_mut(&path).unwrap();
            let hirsel_types::SignatureKind::Function(function) = &mut signature.kind else {
                panic!()
            };
            let mut row = engine.types().row(function.result).clone();
            row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
            function.result = engine.types().intern(row);
            let mut row = engine.types().row(forged.bodies[0].result).clone();
            row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
            forged.bodies[0].result = engine.types().intern(row);
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let violations =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![])
                    .violations;
            assert!(
                violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "value modes"
                    }
                )),
                "{source}\n{violations:?}"
            );
        }
    });
}

#[test]
fn unique_empty_builder_29_original_unchanged_four_engines() {
    with_compiler_stack(|| {
        runtime(include_str!(
            "../fixtures/unique-empty-builder-29/original.hir"
        ))
    });
}
