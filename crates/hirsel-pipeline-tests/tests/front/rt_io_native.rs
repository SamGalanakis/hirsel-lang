//! Every owned Io primitive through compiled calls, C/H validation and exact replay.
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Configuration, Observation, OptLevel, Options, Outcome, Partition, Sites, Tape, Toolchain,
    build, differential, full_matrix, launch_with, link, observe, smoke_matrix,
};
use hirsel_engine::{
    QueryEngine, RunConfiguration, SimulationConfig, check_progress, default_context,
    default_environment, entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_rt::{io::Adapter, io_sim::SimIo};
use std::path::Path;

fn campaign(include: fn(&str, bool) -> bool) {
    let matrix = full_matrix();
    assert_eq!(matrix.len(), 50);
    campaign_in(include, matrix, "io-native");
}

fn campaign_in(include: fn(&str, bool) -> bool, matrix: Vec<Configuration>, scratch_name: &str) {
    let scratch_name = scratch_name.to_owned();
    with_compiler_stack(move || {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(scratch_name);
        std::fs::create_dir_all(&scratch).unwrap();
        let mut targets = vec![];
        for configuration in &matrix {
            if !targets.contains(&configuration.options.target) {
                targets.push(configuration.options.target.clone());
            }
        }
        let tools = Toolchain::for_matrix(&scratch.join("runtime"), &matrix).unwrap();
        let mut cases = vec![];
        for name in [
            "io-dir",
            "dir-narrow",
            "path-of",
            "path-order",
            "arg-path",
            "io-clock",
            "io-random",
            "file-open",
            "file-narrow",
            "file-size",
            "file-read-bytes",
            "file-write-bytes",
            "file-remove",
            "file-create-atomic",
            "file-write-all",
            "file-commit",
            "file-walk",
            "file-atomic-fail",
        ] {
            cases.push((
                name,
                root.join("native-tests/library")
                    .join(format!("{name}.hir")),
                false,
            ));
        }
        for name in [
            "LANG-9-native-test-check-run",
            "LANG-9-native-test-check-fail",
            "LANG-9-native-test-dir-run",
            "LANG-9-native-property-run",
            "LANG-9-native-property-fail",
        ] {
            cases.push((
                name,
                root.join("conformance/LANG").join(format!("{name}.hir")),
                true,
            ));
        }
        cases.retain(|(name, _, test)| include(name, *test));
        assert!(!cases.is_empty());
        let mut failures = vec![];
        'cases: for (name, path, test) in cases {
            eprintln!("Io native matrix: {name}");
            let source = std::fs::read_to_string(&path).unwrap();
            let mut engine = if path.with_extension("hir.json").exists() {
                source_std::engine()
            } else {
                QueryEngine::new(default_environment(), compiler_adapters())
            };
            let context = default_context();
            let file = engine
                .sources
                .add(format!("{name}.hir"), source.clone())
                .unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            if !checked.diagnostics.is_empty() {
                failures.push(format!("{name}: {:?}", checked.diagnostics));
                continue;
            }
            let mut entry = entry_path(&context);
            if test {
                entry.0.name = "trial".into();
            }
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
            assert_eq!(
                reference.failures.is_empty(),
                !name.ends_with("fail"),
                "{name}: {reference:?}"
            );
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            let work = scratch.join(name);
            std::fs::create_dir_all(&work).unwrap();
            for (configuration, outcome) in
                differential(&input, &entry, &reference, &matrix, &tools, &work)
            {
                if outcome != Outcome::Agree {
                    failures.push(format!("{name}: {configuration}: {outcome:?}"));
                    continue 'cases;
                }
            }
            for target in &targets {
                let work = work.join(&target.triple);
                std::fs::create_dir_all(&work).unwrap();
                let program = build(&input, &Options::new(target.clone())).unwrap();
                let executable = link(
                    &program,
                    &entry,
                    OptLevel::Speed,
                    Partition::Whole,
                    &tools,
                    &work,
                )
                .unwrap();
                let tape = work.join("tape");
                let sites = Sites::new(input.snapshot());
                let recorded = launch_with(
                    &executable,
                    target,
                    &tools,
                    &work,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                )
                .unwrap();
                assert_eq!(
                    observe(&recorded, &program, &sites).unwrap(),
                    reference,
                    "{name} record"
                );
                let recording = hirsel_rt::launch::parse_recording(
                    &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
                )
                .unwrap();
                assert_eq!(recording.test_seed, 0);
                if name == "LANG-9-native-property-run" {
                    // 100 passing cases read the seed once and report nothing.
                    let requests: Vec<_> = recording.events.iter().map(|e| &e.request).collect();
                    assert!(
                        matches!(requests[..], [hirsel_rt::io::Request::TestSeed { .. }]),
                        "{requests:?}"
                    );
                }
                let mut sim = SimIo::new(recording.tree.clone(), recording.events.clone());
                if test {
                    sim.test_context(recording.test_seed);
                }
                for event in &recording.events {
                    assert_eq!(
                        sim.call(event.request.clone()).unwrap(),
                        crate::rt_support::library::delivered(&event.completion)
                    );
                }
                sim.finish().unwrap();
                if !test {
                    crate::rt_replay::replay_interpreter(
                        &recording.events,
                        &recording.tree,
                        sim.tree(),
                        &sim.handles().observed(),
                        sim.stdout(),
                    );
                }
                let replayed = launch_with(
                    &executable,
                    target,
                    &tools,
                    &work,
                    true,
                    Tape::Replay(&tape),
                )
                .unwrap();
                assert_eq!(
                    observe(&replayed, &program, &sites).unwrap(),
                    reference,
                    "{name} replay"
                );
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_io_leaves_agree_and_replay_on_both_targets() {
    campaign(|_, test| !test);
}
#[test]
#[ignore = "full gate"]
fn full_gate_test_launches_and_callbacks_agree_and_replay_on_both_targets() {
    campaign(|_, test| test);
}

#[test]
fn test_launches_and_callback_failure_agree_and_replay_in_smoke() {
    let matrix = smoke_matrix();
    assert_eq!(matrix.len(), 3);
    // The failing property shrinks and reports once. The 100-case success and
    // its complete injected-failure campaign remain in the full test above.
    campaign_in(
        |name, _| {
            matches!(
                name,
                "LANG-9-native-test-check-run"
                    | "LANG-9-native-test-check-fail"
                    | "LANG-9-native-property-fail"
            )
        },
        matrix,
        "test-native-smoke",
    );
}

#[test]
#[ignore = "full gate"]
fn full_gate_partial_write_and_canceled_commit_replay_the_reference_tape_on_both_targets() {
    partial_write_replay(
        vec![
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ],
        &[OptLevel::None, OptLevel::Speed],
        "io-fault-native",
    );
}

#[test]
fn partial_write_and_canceled_commit_replay_the_reference_tape_in_smoke() {
    partial_write_replay(
        vec![TargetSpec::x86_64_unknown_linux_gnu()],
        &[OptLevel::Speed],
        "io-fault-native-smoke",
    );
}

fn partial_write_replay(
    targets: Vec<TargetSpec>,
    opts: &'static [OptLevel],
    scratch_name: &'static str,
) {
    use hirsel_interp::library::simulation::Fault as ReferenceFault;
    use hirsel_rt::io::{Completion as C, Event, Fault, IoError, Request as R, Tree};
    use hirsel_rt::launch::{Recording, render_recording};
    use std::collections::BTreeMap;
    with_compiler_stack(move || {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(scratch_name);
        std::fs::create_dir_all(&scratch).unwrap();
        let tools = Toolchain::in_tree_diagnostic(
            &scratch.join("runtime"),
            &targets.iter().collect::<Vec<_>>(),
        )
        .unwrap();
        let directory = Event {
            request: R::Directory {
                io: 1,
                path: ".".into(),
            },
            completion: C::Handle(Ok(2)),
            fault: None,
            resources: None,
        };
        let cases = [
            (
                "file-write-partial",
                "file.write_bytes",
                ReferenceFault::Fault {
                    code: 71,
                    partial: 2,
                },
                vec![
                    directory.clone(),
                    Event {
                        request: R::Write {
                            directory: 2,
                            path: "sample".into(),
                            bytes: b"abcdef".to_vec(),
                        },
                        completion: C::Unit(Err(IoError::Fault(71))),
                        fault: Some(Fault {
                            error: IoError::Fault(71),
                            partial: 2,
                        }),
                        resources: None,
                    },
                    Event {
                        request: R::Read {
                            directory: 2,
                            path: "sample".into(),
                        },
                        completion: C::Bytes(Ok(b"ab".to_vec())),
                        fault: None,
                        resources: None,
                    },
                ],
            ),
            (
                "file-commit-canceled",
                "file.commit",
                ReferenceFault::Canceled(2),
                vec![
                    directory,
                    Event {
                        request: R::CreateAtomic {
                            directory: 2,
                            path: "sample".into(),
                        },
                        completion: C::Handle(Ok(3)),
                        fault: None,
                        resources: None,
                    },
                    Event {
                        request: R::WriteAll {
                            directory: 2,
                            file: 3,
                            bytes: b"data".to_vec(),
                        },
                        completion: C::Unit(Ok(())),
                        fault: None,
                        resources: None,
                    },
                    Event {
                        request: R::Commit {
                            directory: 2,
                            file: 3,
                        },
                        completion: C::Unit(Err(IoError::Canceled(2))),
                        fault: Some(Fault {
                            error: IoError::Canceled(2),
                            partial: 99,
                        }),
                        resources: None,
                    },
                    Event {
                        request: R::Open {
                            directory: 2,
                            path: "sample".into(),
                        },
                        completion: C::Handle(Err(IoError::Fault(2))),
                        fault: Some(Fault {
                            error: IoError::Fault(2),
                            partial: 0,
                        }),
                        resources: None,
                    },
                ],
            ),
        ];
        for (name, operation, fault, events) in cases {
            let path = root
                .join("native-tests/library")
                .join(format!("{name}.hir"));
            let source = std::fs::read_to_string(&path).unwrap();
            let mut engine = if path.with_extension("hir.json").exists() {
                source_std::engine()
            } else {
                QueryEngine::new(default_environment(), compiler_adapters())
            };
            let context = default_context();
            let file = engine.sources.add(format!("{name}.hir"), source).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let mut simulation = SimulationConfig::default();
            simulation.faults.insert(operation.into(), [fault].into());
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                simulation,
            )
            .unwrap();
            let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0));
            assert!(reference.failures.is_empty());
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            let sites = Sites::new(input.snapshot());
            let tree = Tree::new(BTreeMap::new()).unwrap();
            let mut sim = SimIo::new(tree.clone(), events.clone());
            for event in &events {
                assert_eq!(
                    sim.call(event.request.clone()).unwrap(),
                    crate::rt_support::library::delivered(&event.completion)
                );
            }
            sim.finish().unwrap();
            crate::rt_replay::replay_interpreter(
                &events,
                &tree,
                sim.tree(),
                &sim.handles().observed(),
                sim.stdout(),
            );
            if name == "file-write-partial" {
                assert_eq!(sim.tree().files["sample"], b"ab");
            } else {
                assert!(sim.tree().files.is_empty());
                assert!(!sim.handles().observed().contains_key(&3));
            }
            for target in &targets {
                let program = build(&input, &Options::new(target.clone())).unwrap();
                for &opt in opts {
                    let work = scratch.join(format!("{name}-{}-{opt:?}", target.triple));
                    std::fs::create_dir_all(&work).unwrap();
                    let executable =
                        link(&program, &entry, opt, Partition::Whole, &tools, &work).unwrap();
                    let run = hirsel_run_config::RunConfig::read_path(
                        &std::fs::canonicalize(work.join("run.json")).unwrap(),
                    )
                    .unwrap();
                    let config = run.config();
                    let tape = work.join("tape");
                    std::fs::write(
                        &tape,
                        render_recording(&Recording {
                            arguments: vec![],
                            run_identity: config.run_identity(),
                            hash_seed: config.hash_seed(),
                            random_seed: config.random_seed(),
                            test_seed: config.test_seed(),
                            binding: None,
                            tree: tree.clone(),
                            events: events.clone(),
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    let replayed = launch_with(
                        &executable,
                        target,
                        &tools,
                        &work,
                        true,
                        Tape::Replay(&tape),
                    )
                    .unwrap();
                    assert_eq!(
                        observe(&replayed, &program, &sites).unwrap(),
                        reference,
                        "{name}: {} {opt:?}",
                        target.triple
                    );
                }
            }
        }
    });
}
