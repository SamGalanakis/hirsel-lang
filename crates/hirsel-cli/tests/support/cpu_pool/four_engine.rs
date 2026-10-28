fn cpu_fixture_four_engine_agreement(width: u32) {
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("{}u64", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", &format!("[{jobs}]"))
        .replace("EXPECTED", &format!("[{expected}]"));
    cpu_fixture_four_engine_source(width, source, b"cpu-scalar-ok\n");
}

fn cpu_fixture_canceled_admission(width: u32) {
    let source = include_str!("cancel_admission.hir").replace("WIDTH", &format!("{width}u32"));
    cpu_fixture_four_engine_source(width, source, b"cpu-canceled-ok\n");
}

fn cpu_fixture_four_engine_source(width: u32, source: String, expected_output: &[u8]) {
    use hirsel_driver::{compiler_adapters, native};
    use hirsel_engine::{
        QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
        entry_path, run_observed_checked, run_observed_snapshot, with_compiler_stack,
    };

    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cpu.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let config = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let matrix = [
            native::Engine::Heap { campaign: false },
            native::Engine::Native {
                opt: native::OptLevel::Default,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .map(|engine| native::Configuration {
            options: native::Options::new(target.clone()),
            engine,
        });
        let root = tempfile::tempdir().unwrap();
        let mut tools =
            native::Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target])
                .unwrap();
        tools.execution = native::Execution::Executable;
        for free in [true, false] {
            let fixture = native::Fixture {
                cpu: Some(hirsel_run_schema::CpuGrant {
                    provider: "cpu.scoped@1".into(),
                    max_workers: width,
                    max_domains: 1,
                    max_physical_bytes: 1 << 30,
                    max_batch_bytes: 8 << 20,
                }),
                tasks_free: free,
                ..Default::default()
            };
            let run = run_observed_snapshot(&mut engine, &config, (&fixture).into()).unwrap();
            let reference = native::Observation::of_reference(run.observation.as_ref().unwrap());
            assert_eq!(reference.output, expected_output);
            assert!(reference.failures.is_empty(), "{reference:?}");
            let checked = run_observed_checked(&mut engine, &config, (&fixture).into()).unwrap();
            assert_eq!(
                native::Observation::of_reference(checked.observation.as_ref().unwrap()),
                reference,
            );
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let dir = root.path().join(if free { "free" } else { "ordered" });
            std::fs::create_dir_all(&dir).unwrap();
            let outcomes = native::differential_in(
                &validated, &entry, &reference, &matrix, &tools, &dir, &fixture,
            );
            assert_eq!(outcomes.len(), matrix.len());
            for (name, outcome) in outcomes {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "W{width} Free={free} {name}: {outcome:?}",
                );
            }
        }
    });
}

fn cpu_fixture_allocating_four_engine_agreement(width: u32) {
    cpu_fixture_allocating_four_engine_mapper(
        width,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut out = [job]; list.reserve(mut out, 128u64); list.push(mut out, job + 7u64); take out }",
    );
}

fn cpu_fixture_empty_list_four_engine_agreement(width: u32) {
    cpu_fixture_allocating_four_engine_mapper(
        width,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut out: List[u64] = []; list.reserve(mut out, 128u64); list.push(mut out, job); list.push(mut out, job + 7u64); take out }",
    );
}

fn cpu_fixture_copied_list_four_engine_agreement(width: u32) {
    cpu_fixture_allocating_four_engine_mapper(
        width,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut out: List[u64] = []; list.reserve(mut out, 128u64); list.push(mut out, job); list.push(mut out, job + 7u64); let mut copied = copy out; copied[1] = job + 7u64; [copied[0], copied[1]] }",
    );
}

fn cpu_fixture_allocating_four_engine_mapper(width: u32, mapper: &str) {
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("[{n}u64, {}u64]", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", &format!("[{jobs}]"))
        .replace("EXPECTED", &format!("[{expected}]"))
        .replace("fn(take job: u64) -> u64 uses {} { job + 7u64 }", mapper);
    cpu_fixture_four_engine_source(width, source.clone(), b"cpu-scalar-ok\n");
    cpu_interpreter_matrix(width as usize, false, &source, false);
}
