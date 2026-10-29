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
    cpu_fixture_four_engine_source_grant(
        width,
        source,
        expected_output,
        8 << 20,
        &[true, false],
        &[hirsel_driver::native::Runtime::Diagnostic],
    );
}

fn cpu_fixture_four_engine_source_grant(
    width: u32,
    source: String,
    expected_output: &[u8],
    batch_bytes: u64,
    policies: &[bool],
    runtimes: &[hirsel_driver::native::Runtime],
) {
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
        let engines = std::iter::once(native::Engine::Heap { campaign: false }).chain(
            runtimes.iter().flat_map(|&runtime| {
                [
                    native::Engine::Native {
                        opt: native::OptLevel::Default,
                        partition: native::Partition::Whole,
                        runtime,
                    },
                    native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime,
                    },
                ]
            }),
        );
        let matrix: Vec<_> = engines
            .map(|engine| native::Configuration {
                options: native::Options::new(target.clone()),
                engine,
            })
            .collect();
        let root = tempfile::tempdir().unwrap();
        let mut tools =
            native::Toolchain::for_matrix(&root.path().join("runtime"), &matrix).unwrap();
        tools.execution = native::Execution::Executable;
        for &free in policies {
            let fixture = native::Fixture {
                cpu: Some(hirsel_run_schema::CpuGrant {
                    provider: "cpu.scoped@1".into(),
                    max_workers: width,
                    max_domains: 1,
                    max_physical_bytes: 1 << 30,
                    max_batch_bytes: batch_bytes,
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
        "fn(take job: u64) -> unique List[u64] uses {} { let mut checksum = 0u64; for i in 0u64..96u64 { let mut chunk = [job]; list.reserve(mut chunk, 128u64); list.push(mut chunk, i); checksum = int.wrapping_add(checksum, chunk[1]) }; let mut out = [job]; list.reserve(mut out, 128u64); list.push(mut out, job + checksum - 4553u64); take out }",
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

fn cpu_fixture_allocating_pressure_four_engine_agreement(width: u32) {
    // Two 2 MiB temporaries exceed the 3 MiB grant together. Each job frees
    // its large buffer before publishing a small result, so serial succeeds.
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", "[1u64, 2u64]")
        .replace("EXPECTED", "[[1u64, 8u64], [2u64, 9u64]]")
        .replace(
            "fn(take job: u64) -> u64 uses {} { job + 7u64 }",
            "fn(take job: u64) -> unique List[u64] uses {} { let mut buffer = [job]; list.reserve(mut buffer, 262144u64); [buffer[0], buffer[0] + 7u64] }",
        );
    cpu_fixture_four_engine_source_grant(
        width,
        source.clone(),
        b"cpu-scalar-ok\n",
        3 << 20,
        &[false],
        &[
            hirsel_driver::native::Runtime::Diagnostic,
            hirsel_driver::native::Runtime::Default,
            hirsel_driver::native::Runtime::System,
        ],
    );
    cpu_interpreter_matrix_grant(width as usize, false, &source, false, 3 << 20);
}
