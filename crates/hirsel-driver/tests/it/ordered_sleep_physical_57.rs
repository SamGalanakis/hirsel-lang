//! Like the compact custody witness in runtime50_resume, with an entered host
//! marker: a pre-operation resume point alone is not an in-operation leave.
use hirsel_driver::native::{self, Backend, Fixture, OptLevel, Partition, Tape, Toolchain};
use hirsel_engine::*;

#[test]
fn entered_sleep_and_join_release_stacks_at_widths_one_and_four_and_replay() {
    with_compiler_stack(|| {
        let source = super::ordered_sleep_resume_57::CERTIFIED_SOURCE;
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let fixture = Fixture {
            monotonic: Some(0),
            ..Default::default()
        };
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 50,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let root = tempfile::Builder::new()
            .prefix("ordered-sleep57-")
            .tempdir()
            .unwrap();
        let mut tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
        tools.execution = native::Execution::Executable;
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            for width in [1, 4] {
                let dir = root.path().join(format!("{backend:?}-{width}"));
                std::fs::create_dir_all(&dir).unwrap();
                let runnable = native::link_fixture_runnable(
                    &program, &entry, &objects, &tools, &dir, &fixture,
                )
                .unwrap();
                // Placement is excluded from the semantic binding. Harness
                // admission uses it to exercise the actual physical dispatcher.
                let mut run = native::fixture_run_record(&program, &entry, &fixture)
                    .unwrap()
                    .record()
                    .clone();
                run.placement =
                    Some(serde_json::from_value(serde_json::json!({"cores":width})).unwrap());
                std::fs::write(
                    dir.join("run.json"),
                    run.validated().unwrap().launch_bytes(),
                )
                .unwrap();
                let tape = dir.join("wait.tape");
                for replay in [false, true] {
                    let trace = dir.join(format!("compact-{replay}"));
                    tools.wrapper = vec![
                        "/usr/bin/env".into(),
                        format!("HIRSEL_RT_COMPACT_TRACE={}", trace.display()),
                    ];
                    let cell = format!("{backend:?} width={width} replay={replay}");
                    let launched = native::launch_fixture(
                        &runnable,
                        &target,
                        &tools,
                        &dir,
                        true,
                        if replay {
                            Tape::Replay(&tape)
                        } else {
                            Tape::Live {
                                record: Some(&tape),
                                revocations: None,
                            }
                        },
                        &fixture,
                    )
                    .unwrap_or_else(|error| panic!("{cell}: {error}"));
                    assert_eq!(
                        native::observe(&launched, &program, &sites).unwrap(),
                        expected,
                        "{cell}"
                    );
                    let trace = std::fs::read_to_string(trace).unwrap();
                    check_pending_leaves(&trace, &cell);
                }
            }
        }
    });
}

fn check_pending_leaves(trace: &str, cell: &str) {
    let rows: Vec<Vec<&str>> = trace
        .lines()
        .map(|s| s.split_whitespace().collect())
        .collect();
    for kind in ["32", "33"] {
        let waits: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.first() == Some(&"runtime") && w.get(4) == Some(&kind))
            .collect();
        assert!(
            !waits.is_empty(),
            "{cell}: no entered wait kind {kind}\n{trace}"
        );
        for (index, wait) in waits {
            assert!(
                rows[index + 1..].iter().any(|w| w.first() == Some(&"leave")
                    && w.get(1) == wait.get(1)
                    && w.get(4) == Some(&"1")),
                "{cell}: Pending never released its stack\n{trace}"
            );
        }
    }
}
