//! Remaining-count vectors adapted from Rust Take (MIT), f6c1107f8be13e48465e12c0fd26e6a39fe18e4f:
//! library/coretests/tests/iter/adapters/take.rs:103-128. Overflow/error/drop
//! traces are Hirsel-specific additions; the oracle never calls an adapter.
use hirsel_driver::{native::*, *};
use hirsel_engine::{RunConfiguration, Verify, with_compiler_stack};
use std::{fmt::Write, path::PathBuf};

fn report<T>(error: hirsel_engine::ToolError) -> T {
    if let hirsel_engine::ToolError::Query {
        error: hirsel_types::QueryError::Diagnostics(ds),
        sources,
        ..
    } = &error
    {
        for diagnostic in ds {
            eprintln!("{}", diagnostic.render_human(sources));
        }
    }
    panic!("{error}")
}

const SOURCE: &str = include_str!("../fixtures/stream-item-bound-38/trace.hir");
fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream-item-bound-38/trace.hir")
}
// (id, raw length, error position, premature EOF position, budget, caller pulls, truncating).
const CASES: &[(&str, u64, u64, u64, u64, u64, bool)] = &[
    ("zero-empty", 0, 99, 99, 0, 3, false),
    ("zero-item", 1, 99, 99, 0, 3, false),
    ("zero-error", 1, 0, 99, 0, 3, false),
    ("one-exact", 1, 99, 99, 1, 4, false),
    ("one-over", 2, 99, 99, 1, 4, false),
    ("exact", 4, 99, 99, 4, 7, false),
    ("short", 4, 99, 99, 5, 7, false),
    ("max", 2, 99, 99, u64::MAX, 5, false),
    ("error-before", 4, 0, 99, 2, 4, false),
    ("error-at", 4, 1, 99, 2, 5, false),
    ("error-lookahead", 4, 2, 99, 2, 5, false),
    ("error-after", 4, 3, 99, 2, 5, false),
    ("nonfused", 4, 99, 1, 3, 5, false),
    ("nonfused-error", 4, 1, 99, 3, 5, false),
    ("drop-before-pull", 4, 99, 99, 0, 0, false),
    ("drop-at-bound", 4, 99, 99, 2, 2, false),
    ("drop-prefix", 4, 99, 99, 3, 1, false),
    ("take-zero", 4, 0, 99, 0, 3, true),
    ("take-prefix", 4, 2, 99, 2, 5, true),
];
fn oracle() -> Vec<u8> {
    let mut trace = String::new();
    for &(id, length, error, eof, limit, stop, truncate) in CASES {
        writeln!(trace, "{id}\nconstructed").unwrap();
        let mut pulls = 0;
        let mut terminal = false;
        for demand in 0..stop {
            if terminal || (truncate && demand >= limit) {
                terminal = true;
                writeln!(trace, "none").unwrap();
                continue;
            }
            let index = pulls;
            pulls += 1;
            writeln!(trace, "pull:{index}").unwrap();
            if index >= length || index == eof {
                terminal = true;
                writeln!(trace, "none").unwrap();
            } else if !truncate && demand == limit {
                terminal = true;
                writeln!(trace, "drop:{index}\nlimit").unwrap();
            } else {
                let tag = if index == error {
                    terminal = true;
                    "source-error"
                } else {
                    "yield"
                };
                writeln!(trace, "{tag}:{index}\ndrop:{index}").unwrap();
            }
        }
        writeln!(trace, "caller-stop:{stop}\nsource-drop:{pulls}\nreleased").unwrap();
    }
    // Independent effect catalogues: in the first, the extra item must never
    // reach the predicate. In the second, rejection occurs before the probe.
    trace
        .push_str("bound-before-filter\npull:0\nfilter:0\ndrop:0\npull:1\ndrop:1\nsource-drop:2\n");
    trace.push_str("filter-before-bound\npull:0\nfilter:0\ndrop:0\npull:1\nfilter:1\ndrop:1\npull:2\nfilter:2\ndrop:2\nsource-drop:3\n");
    trace.push_str("nested\npull:0\ndrop:0\npull:1\ndrop:1\nsource-drop:2\n");
    trace.into_bytes()
}

fn controls(engine: &str) {
    with_compiler_stack(|| {
        let mut loaded = load_program_source(&path(), Some(SOURCE)).unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(|error| {
            if let hirsel_engine::ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(ds),
                sources,
                ..
            } = &error
            {
                for d in ds {
                    eprintln!("{}", d.render_human(sources));
                }
            }
            panic!("{error}")
        });
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: oracle(),
            failures: vec![],
            roots: vec![],
        };
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for checked in [false, true] {
            if engine != if checked { "checked" } else { "reference" } {
                continue;
            }
            eprintln!(
                "ITEM_BOUND_STARTED checked={checked} rows={}",
                CASES.len() + 3
            );
            let result = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, Default::default())
            }
            .unwrap();
            let observed = result
                .observation
                .unwrap_or_else(|| panic!("INCOMPLETE {:?}", result.outcome));
            assert_eq!(
                String::from_utf8_lossy(&observed.output),
                String::from_utf8_lossy(&expected.output)
            );
            assert_eq!(Observation::of_reference(&observed), expected);
            assert!(observed.final_state.unwrap().stderr.is_empty());
            eprintln!("ITEM_BOUND_PASS checked={checked} rows={}", CASES.len() + 3);
        }
        if engine == "reference" || engine == "checked" {
            return;
        }
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input.clone()).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        assert!(ready.units.iter().any(|unit| {
            let h = &unit.unit().header;
            h.definition.0.modules == ["stream"]
                && h.definition.0.name == "bound"
                && h.origin == hirsel_lir::ir::UnitOrigin::Source
        }));
        let scratch = tempfile::tempdir().unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &scratch.path().join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        let sites = Sites::new(&input);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            if engine
                != if backend == Backend::Llvm {
                    "llvm"
                } else {
                    "cranelift"
                }
            {
                continue;
            }
            eprintln!("ITEM_BOUND_STARTED {backend:?} rows={}", CASES.len() + 3);
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let dir = scratch.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let fixture = Fixture::default();
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                    .unwrap();
            let tape = dir.join("recording");
            let launched = launch_fixture(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                &fixture,
            )
            .unwrap();
            assert_eq!(observe(&launched, &ready, &sites).unwrap(), expected);
            assert!(launched.stderr.is_empty());
            eprintln!(
                "ITEM_BOUND_TAPE {backend:?} {}",
                std::fs::read(tape)
                    .unwrap()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            eprintln!("ITEM_BOUND_PASS {backend:?} rows={}", CASES.len() + 3);
        }
    });
}

// V must independently refuse a declaration promising an escaping/global
// result when its body stores the incoming cursor. No source negative is used
// as a substitute for this forged-TIR check.
#[test]
fn v_refuses_erased_capture_relation() {
    with_compiler_stack(|| {
        let mut loaded = load_program_source(&path(), Some(SOURCE)).unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        let (path, original) = input
            .definitions
            .iter()
            .find(|(path, _)| path.0.name == "bounded")
            .unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        assert!(
            hirsel_validate::validate(original, &input.types, &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut signature = input.signatures[path].as_ref().clone();
        let hirsel_types::SignatureKind::Function(f) = &mut signature.kind else {
            unreachable!()
        };
        let old = f.result;
        let mut row = input.types.get(old).unwrap().clone();
        assert!(matches!(&row.erased_captures,
            Some(hirsel_types::ErasedCaptures::FromArguments(parameters)) if !parameters.is_empty()));
        row.erased_captures = Some(hirsel_types::ErasedCaptures::Known(
            hirsel_types::AuthoritySummary::inert(),
        ));
        let new = input.types.intern(row);
        f.result = new;
        signatures.insert(signature.definition, signature);
        let mut forged = original.as_ref().clone();
        for body in &mut forged.bodies {
            // Body parameter references are bound to its own scope, so its
            // result row need not be the signature's declaration-scope row.
            let body_old = body.result;
            let mut body_row = input.types.get(body_old).unwrap().clone();
            body_row.erased_captures = Some(hirsel_types::ErasedCaptures::Known(
                hirsel_types::AuthoritySummary::inert(),
            ));
            let body_new = input.types.intern(body_row);
            body.result = body_new;
            for local in &mut body.locals {
                if local.ty == body_old {
                    local.ty = body_new;
                }
            }
            for place in &mut body.places {
                if place.ty == body_old {
                    place.ty = body_new;
                }
            }
        }
        forged.types = hirsel_tir::definition_types(&input.types, &forged);
        let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
        assert!(
            !report.violations.is_empty(),
            "missing V refusal: {report:?}"
        );
        assert!(
            !report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "body result"
                }
            )),
            "a mismatched header is not the capture adversary: {report:?}"
        );
        eprintln!("ITEM_BOUND_PASS V-negative {:?}", report.violations);
    });
}

fn faults(engine: &str) {
    with_compiler_stack(|| {
        let path = path().with_file_name("fault.hir");
        let mut loaded = load_program_source(
            &path,
            Some(include_str!("../fixtures/stream-item-bound-38/fault.hir")),
        )
        .unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input.clone()).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(&input);
        let scratch = tempfile::tempdir().unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &scratch.path().join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        let objects: Vec<_> = [Backend::Llvm, Backend::Cranelift]
            .into_iter()
            .map(|b| {
                (
                    b,
                    emit_backend_objects(&ready, b, OptLevel::None, Partition::Whole, false, None)
                        .unwrap(),
                )
            })
            .collect();
        let drop_definition = input
            .signatures
            .values()
            .find_map(|signature| {
                if signature.definition.path().0.name != "Item" {
                    return None;
                }
                let hirsel_types::SignatureKind::Type(t) = &signature.kind else {
                    return None;
                };
                t.drop.clone()
            })
            .unwrap();
        for mode in ["pull", "drop-item", "drop-error"] {
            let fixture = Fixture {
                args: vec![mode.into()],
                ..Default::default()
            };
            let expected_output = if mode == "pull" {
                b"pull\n".as_slice()
            } else {
                b"pull\ndiscard\n".as_slice()
            };
            // The independent catalogue fixes both the failure site and all
            // visible output; catching the library Limit must never run.
            // ebc3e4a3's fixture declares read at 17; its actual failure.check is 19.
            let expected_line = if mode == "pull" { 19 } else { 7 };
            let expected_name = if mode == "pull" {
                "read"
            } else {
                drop_definition.0.name.as_str()
            };
            let check = |observed: &Observation| {
                assert_eq!(observed.output, expected_output, "{mode}: {observed:?}");
                assert_eq!(observed.exit_code, None, "{mode}: {observed:?}");
                assert!(
                    !observed.blocked && observed.budget.is_none() && observed.signaled.is_none()
                );
                assert_eq!(observed.failures.len(), 1, "{mode}: {observed:?}");
                let failure = &observed.failures[0];
                assert_eq!(failure.rule, "MEM-14");
                assert_eq!(failure.line, expected_line);
                assert_eq!(failure.definition.0.name, expected_name);
            };
            let mut previous = None;
            for checked in [false, true] {
                if engine != if checked { "checked" } else { "reference" } {
                    continue;
                }
                eprintln!("ITEM_BOUND_STARTED fault/{mode}/checked={checked}");
                let run = RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &run, (&fixture).into())
                } else {
                    hirsel_engine::run_observed_snapshot(
                        &mut loaded.engine,
                        &run,
                        (&fixture).into(),
                    )
                }
                .unwrap();
                let observed = Observation::of_reference(&execution.observation.unwrap());
                check(&observed);
                if let Some(old) = &previous {
                    assert_eq!(&observed, old);
                }
                previous = Some(observed);
                eprintln!("ITEM_BOUND_PASS fault/{mode}/checked={checked}");
            }
            for (backend, objects) in &objects {
                if engine
                    != if *backend == Backend::Llvm {
                        "llvm"
                    } else {
                        "cranelift"
                    }
                {
                    continue;
                }
                eprintln!("ITEM_BOUND_STARTED fault/{mode}/{backend:?}");
                let dir = scratch.path().join(format!("{mode}-{backend:?}"));
                std::fs::create_dir_all(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                let tape = dir.join("recording");
                let launched = launch_fixture(
                    &runnable,
                    &ready.target,
                    &tools,
                    &dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &fixture,
                )
                .unwrap();
                let observed = observe(&launched, &ready, &sites).unwrap();
                check(&observed);
                if let Some(previous) = &previous {
                    assert_eq!(&observed, previous);
                }
                eprintln!(
                    "ITEM_BOUND_TAPE fault/{mode}/{backend:?} {}",
                    std::fs::read(tape)
                        .unwrap()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                eprintln!("ITEM_BOUND_PASS fault/{mode}/{backend:?}");
            }
        }
    });
}

#[test]
fn reference_controls() {
    controls("reference");
}
#[test]
fn checked_controls() {
    controls("checked");
}
#[test]
fn llvm_controls() {
    controls("llvm");
}
#[test]
fn cranelift_controls() {
    controls("cranelift");
}
#[test]
fn reference_failures() {
    faults("reference");
}
#[test]
fn checked_failures() {
    faults("checked");
}
#[test]
fn llvm_failures() {
    faults("llvm");
}
#[test]
fn cranelift_failures() {
    faults("cranelift");
}

// The original source-checker prerequisite, independent of the adapter.
#[test]
fn unique_payload_source_prerequisite() {
    with_compiler_stack(|| {
        let path = path().with_file_name("unique-original.hir");
        let mut loaded = load_program_source(
            &path,
            Some(include_str!(
                "../fixtures/stream-item-bound-38/unique-original.hir"
            )),
        )
        .unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        loaded.engine.import_native_input(input).unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
                .unwrap();
        let observed = execution.observation.unwrap();
        assert_eq!(observed.exit_code, Some(0));
        assert!(observed.output.is_empty() && observed.failures.is_empty());
        eprintln!("ITEM_BOUND_PASS unique-direct/V/checked-H");
    });
}

#[test]
fn unique_payload_aliased_source_refused() {
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/stream-item-bound-38/unique-original.hir")
            .replace(
                "  match take received {",
                "  let snapshot = copy received\n  match take received {",
            )
            .replace("  Ok(())", "  let _ = take snapshot\n  Ok(())");
        let mut loaded = load_program_source(&path(), Some(&source)).unwrap_or_else(report);
        let error = loaded
            .check_for_execution()
            .expect_err("a shared receipt cannot promise a unique payload");
        let hirsel_engine::ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(ds),
            ..
        } = error
        else {
            panic!("expected MODE-2 diagnostic");
        };
        assert!(ds.iter().any(|d| d.rule.as_str() == "MODE-2"));
        eprintln!("ITEM_BOUND_PASS unique-aliased/source-refusal");
    });
}

#[test]
fn unique_payload_aliased_v_refused() {
    with_compiler_stack(|| {
        let path = path().with_file_name("unique-original.hir");
        let mut loaded = load_program(&path).unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        let (_, original) = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "main" && p.0.package.name_is("bound_tests"))
            .unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        assert!(
            hirsel_validate::validate(original, &input.types, &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let (body_index, value_index) = original.bodies.iter().enumerate().find_map(|(b, body)| {
            body.rvalues.iter().enumerate().find_map(|(v, value)| {
                let hirsel_tir::Rvalue::Use(operand) = value else { return None; };
                let place = &body.places[operand.place.0 as usize];
                (operand.usage == hirsel_types::UseKind::Take && place.projection.is_empty()
                    && matches!(&input.types.get(place.ty).unwrap().kind,
                        hirsel_types::TypeKind::Named { definition, .. } if definition.path().0.name == "Option"))
                    .then_some((b, v))
            })
        }).expect("owned Option receipt transfer");
        let mut forged = original.as_ref().clone();
        let hirsel_tir::Rvalue::Use(operand) = &mut forged.bodies[body_index].rvalues[value_index]
        else {
            unreachable!()
        };
        operand.usage = hirsel_types::UseKind::Copy;
        let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "copied receipt must not validate as unique: {report:?}"
        );
        eprintln!("ITEM_BOUND_PASS unique-aliased/V {:?}", report.violations);
    });
}

#[test]
fn unique_stream_payloads_four_engines() {
    with_compiler_stack(|| {
        let path = path().with_file_name("unique-flow.hir");
        let mut loaded = load_program_source(
            &path,
            Some(include_str!(
                "../fixtures/stream-item-bound-38/unique-flow.hir"
            )),
        )
        .unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        loaded.engine.import_native_input(input).unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for checked in [false, true] {
            eprintln!("ITEM_BOUND_STARTED unique-stream/checked={checked}");
            let execution = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, Default::default())
            }
            .unwrap();
            let observed = execution.observation.unwrap();
            assert_eq!(observed.exit_code, Some(0));
            assert!(observed.output.is_empty() && observed.failures.is_empty());
            eprintln!("ITEM_BOUND_PASS unique-stream/checked={checked}");
        }
        let scratch = tempfile::tempdir().unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            eprintln!("ITEM_BOUND_STARTED unique-stream/{backend:?}");
            let tape = scratch.path().join(format!("{backend:?}.tape"));
            let mut stdout = vec![];
            let mut stderr = vec![];
            let outcome = run_path_native_backend_with_validation(
                &path,
                HostLaunch::Live {
                    config: LaunchConfig::Grants {
                        input: std::ffi::OsStr::new("{}"),
                        arguments: None,
                    },
                    record: Some(&tape),
                    revocations: None,
                },
                None,
                backend,
                Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    hirsel_engine::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{outcome:?}"
            );
            assert!(stdout.is_empty() && stderr.is_empty());
            eprintln!(
                "ITEM_BOUND_TAPE unique-stream/{backend:?} {}",
                std::fs::read(tape)
                    .unwrap()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            eprintln!("ITEM_BOUND_PASS unique-stream/{backend:?}");
        }
    });
}

#[test]
fn unique_receipt_shared_intermediate_and_unqualified_payload_refused() {
    with_compiler_stack(|| {
        let original = include_str!("../fixtures/stream-item-bound-38/unique-original.hir");
        let intermediate = original
            .replace(
                "Some(value) => match take value {",
                "Some(value) => { let snapshot = copy value; match take value {",
            )
            .replace(
                "    },\n    None",
                "    }; let _ = take snapshot },\n    None",
            );
        let inferred = original.replace("unique Result[unique Bytes, ()]", "Result[Bytes, ()]");
        let mut loaded = load_program_source(&path(), Some(&inferred)).unwrap_or_else(report);
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
                .unwrap_or_else(report);
        assert!(matches!(
            execution.outcome,
            Ok(hirsel_engine::RunOutcome::Returned(
                hirsel_run_schema::Returned::Ok
            ))
        ));
        eprintln!("ITEM_BOUND_PASS private-producer/checked-H");
        // An exported producer supplies only its declared contract. The private
        // control above additionally proves deep uniqueness from its body.
        let original = original.replace("fn fresh(", "pub fn fresh(");
        let intermediate = intermediate.replace("fn fresh(", "pub fn fresh(");
        let unqualified = original.replace("unique Result[unique Bytes, ()]", "Result[Bytes, ()]");
        // An unqualified wrapper may share its declared unique payload with a
        // counted copy made before the receipt, so the receipt's root evidence
        // stops there: through an Option, and through a stream item.
        let wrapper = original.replace(
            "unique Result[unique Bytes, ()]",
            "Result[unique Bytes, ()]",
        );
        let stream = include_str!("../fixtures/stream-item-bound-38/unique-flow.hir").replace(
            "require_unique(unique payload",
            "require_unique(take payload",
        );
        for (id, source) in [
            ("shared-intermediate", intermediate),
            ("unqualified-payload", unqualified),
            ("unqualified-intermediate", wrapper),
            ("unqualified-stream-item", stream),
        ] {
            let mut loaded = load_program_source(&path(), Some(&source)).unwrap_or_else(report);
            let run = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let error =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
                    .expect_err("checked H must refuse an unproved unique transfer");
            let hirsel_engine::ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(ds),
                ..
            } = error
            else {
                panic!("expected MODE-2: {id}");
            };
            assert!(
                ds.iter().any(|d| d.rule.as_str() == "MODE-2"),
                "{id}: {ds:?}"
            );
            eprintln!("ITEM_BOUND_PASS {id}/checked-H-refusal");
        }
    });
}

#[test]
fn native_importer_with_symbolic_capture_dependency() {
    with_compiler_stack(|| {
        let path = path().with_file_name("unique-flow.hir");
        let mut loaded = load_program(&path).unwrap_or_else(report);
        loaded.check_for_execution().unwrap_or_else(report);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let ready = build(&validated, &options).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &scratch.path().join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            assert!(!objects.is_empty());
            let dir = scratch.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            link_fixture_runnable(
                &ready,
                &loaded.entry,
                &objects,
                &tools,
                &dir,
                &Fixture::default(),
            )
            .unwrap();
            eprintln!("ITEM_BOUND_PASS post-capture-dependency/V/native-build/{backend:?}");
        }
    });
}
