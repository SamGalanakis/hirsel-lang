//! Pinned Go x/net lexical vectors and explicit bounded-input controls.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn html_tokenizer_15_vectors_on_h_heap_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/html-tokenizer-15/vectors.hir");
        let mut loaded = load_program(&path).unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(100_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            reference.outcome
        );
        eprintln!("html-tokenizer-15: H completed 369 lexical cases and cap controls");
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configs = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let ready = native::build(&validated, &configs[0].options).unwrap();
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(100_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        eprintln!(
            "html-tokenizer-15: heap completed 369 lexical cases and cap controls; no live heap bytes"
        );
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("html-tokenizer-15");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
        eprintln!("html-tokenizer-15: H, heap, LLVM and Cranelift executed the fixed vectors");
    });
}
