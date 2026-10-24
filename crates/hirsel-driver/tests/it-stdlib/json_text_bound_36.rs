//! Strict quoted transitions/vectors: serde_json 8c535a2 (MIT OR Apache-2.0).
use hirsel_driver::{native::*, *};
use hirsel_engine::{LoadedProgram, RunConfiguration, ToolError, with_compiler_stack};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn check(path: &Path, source: Option<&str>) -> LoadedProgram {
    let mut loaded = load_program_source(path, source).unwrap();
    loaded.check_for_execution().unwrap_or_else(|error| {
        if let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = &error
        {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic.render_human(sources));
            }
        }
        panic!("{error}")
    });
    loaded
}
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/SEM/SEM-3-json-text-bound-36-run.hir")
}
fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
// Qualification recovery only: default runs every cell. Accepted unchanged
// observations can be reused explicitly without relaunching them after a
// harness-only repair. Receipts remain in the caller's retained action logs.
fn reused(id: &str, engine: &str) -> bool {
    let cell = format!("{id}/{engine}");
    let reused = std::env::var("JSON_TEXT_BOUND_36_REUSE")
        .is_ok_and(|cells| cells.split(',').any(|accepted| accepted == cell));
    if reused {
        eprintln!("BOUND_REUSED {cell}");
    }
    reused
}
// One compile per program, named inner cells, exact stdout/status/stderr and
// cursor assertions inside the program. Native tapes stay separate per recipe.
fn matrix(path: &Path, loaded: &mut LoadedProgram, rows: Vec<(&str, Fixture, Vec<u8>, i32)>) {
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
        .map(|backend| {
            (
                backend,
                emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap(),
            )
        })
        .collect();
    for (id, fixture, output, exit_code) in rows {
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let expected = Observation {
            exit_code: Some(exit_code),
            blocked: false,
            budget: None,
            signaled: None,
            output: output.clone(),
            failures: vec![],
            roots: vec![],
        };
        for checked in [false, true] {
            if reused(id, if checked { "checked" } else { "reference" }) {
                continue;
            }
            eprintln!(
                "BOUND_STARTED {id}/{}",
                if checked { "checked" } else { "reference" }
            );
            let result = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, (&fixture).into())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, (&fixture).into())
            }
            .unwrap();
            let observed = result
                .observation
                .unwrap_or_else(|| panic!("INCOMPLETE {id}: {:?}", result.outcome));
            assert_eq!(observed.exit_code, Some(exit_code), "{id}: {observed:?}");
            assert_eq!(observed.output, output, "{id}");
            assert!(
                observed.failures.is_empty() && observed.blocked.is_none(),
                "{observed:?}"
            );
            assert!(observed.final_state.as_ref().unwrap().stderr.is_empty());
            let observation = Observation::of_reference(&observed);
            assert_eq!(observation, expected);
            eprintln!(
                "BOUND_PASS {id}/{}",
                if checked { "checked" } else { "reference" }
            );
        }
        let mut runnables = vec![];
        for (backend, objects) in &objects {
            let dir = scratch.path().join(format!("{id}-{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                    .unwrap();
            runnables.push((backend, runnable, dir));
        }
        for (backend, runnable, dir) in &runnables {
            if reused(id, &format!("{backend:?}")) {
                continue;
            }
            eprintln!("BOUND_STARTED {id}/{backend:?}");
            let tape = dir.join("recording");
            let launched = launch_fixture(
                runnable,
                &ready.target,
                &tools,
                dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                &fixture,
            )
            .unwrap();
            assert_eq!(
                observe(&launched, &ready, &sites).unwrap(),
                expected,
                "{id}: {launched:?}"
            );
            assert!(launched.stderr.is_empty());
            eprintln!(
                "BOUND_TAPE {id}/{backend:?} {}",
                hex(&std::fs::read(&tape).unwrap())
            );
            eprintln!("BOUND_PASS {id}/{backend:?}");
            // Replay one representative success, Limit and malformed field per
            // app. Test entries have no bound run record for offline replay;
            // Main+Args fixtures below exercise replay and changed bindings.
            if !["success", "name129", "source129", "malformed"].contains(&id) {
                continue;
            }
            for (label, replay) in [
                (
                    "reference",
                    replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                ),
                ("checked", replay_observations_checked),
            ] {
                let observed = replay(path, &tape, Some(1_000_000)).unwrap();
                assert_eq!(Observation::of_reference(&observed), expected);
                eprintln!("BOUND_PASS {id}/{backend:?}/replay/{label}");
            }
            for (replayer, executable, replay_dir) in &runnables {
                let replayed = launch_fixture(
                    executable,
                    &ready.target,
                    &tools,
                    replay_dir,
                    true,
                    Tape::Replay(&tape),
                    &fixture,
                )
                .unwrap();
                assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                assert!(replayed.stderr.is_empty());
                eprintln!("BOUND_PASS {id}/{backend:?}/replay/{replayer:?}");
            }
            // A changed Args header must refuse against its bound run record.
            let mut record =
                hirsel_rt::launch::parse_recording_bytes(&std::fs::read(&tape).unwrap()).unwrap();
            record.arguments.push("changed".into());
            let altered = dir.join("changed-binding");
            std::fs::write(
                &altered,
                hirsel_rt::launch::render_recording(&record).unwrap(),
            )
            .unwrap();
            for replay in [replay_observations, replay_observations_checked] {
                let refusal = replay(path, &altered, Some(1_000_000))
                    .unwrap_err()
                    .to_string();
                assert!(
                    refusal.contains("the header disagrees with its run record"),
                    "{refusal}"
                );
            }
            for (_, executable, replay_dir) in &runnables {
                let refusal = match launch_fixture(
                    executable,
                    &ready.target,
                    &tools,
                    replay_dir,
                    true,
                    Tape::Replay(&altered),
                    &fixture,
                ) {
                    Err(error) => error.to_string(),
                    Ok(launched) => format!("{launched:?}"),
                };
                assert!(
                    refusal.contains("the header disagrees with its run record"),
                    "{refusal}"
                );
            }
            eprintln!("BOUND_PASS {id}/{backend:?}/changed-binding/four-engines");
        }
    }
}

#[test]
fn four_engine_decoded_bytes_syntax_cursor_and_lifetimes() {
    with_compiler_stack(|| {
        let path = corpus();
        let mut loaded = check(&path, None);
        matrix(
            &path,
            &mut loaded,
            vec![("controls", Fixture::default(), vec![], 0)],
        );
    });
}

#[test]
fn production_append_and_capacity_requests_are_bounded() {
    with_compiler_stack(|| {
        let production = include_str!("../../../../stdlib/std@1/json.hir");
        let start = production.find("pub fn read_owned_text_bounded(").unwrap();
        let end = production[start..].find("\n}\n").unwrap() + start + 3;
        let function = &production[start..end];
        // These are all growing operations in the decoder. A new growth path
        // must update this instrumentation; no RSS sample stands in for it.
        assert_eq!(function.matches("core@1/bytes.append").count(), 1);
        assert_eq!(function.matches("char.format_into").count(), 1);
        assert!(!function.contains("reserve") && !function.contains("list.push"));
        let traced = function
            .replace("pub fn read_owned_text_bounded(mut cursor: Reader, max_decoded_bytes: u64)", "fn traced(mut cursor: json.Reader, max_decoded_bytes: u64, mut requested: u64)")
            .replace("ReadTextBoundedError", "json.ReadTextBoundedError")
            .replace("peek(mut cursor)", "json.peek(mut cursor)")
            .replace("valid_utf8(span)", "json.valid_utf8(span)")
            .replace("json_escape(cursor.data, mut cursor.pos)", "escape(cursor.data, mut cursor.pos)")
            .replace("core@1/bytes.append(mut output, span)", "failure.check(!limited && bytes.len(span) <= max_decoded_bytes - requested, Arithmetic())\nrequested += bytes.len(span)\ncore@1/bytes.append(mut output, span)")
            .replace("char.format_into(scalar, mut output)", "failure.check(!limited && width <= max_decoded_bytes - requested, Arithmetic())\nrequested += width\nchar.format_into(scalar, mut output)");
        // 214eae8f's bulk scan uses the production stop set as well as its helpers.
        let mut helpers = production
            .lines()
            .find(|line| line.starts_with("const JSON_STRING_STOP:"))
            .unwrap()
            .to_owned();
        helpers.push('\n');
        for (name, replacement) in [("json_hex4", "hex4"), ("json_escape", "escape")] {
            let start = production.find(&format!("fn {name}(")).unwrap();
            let end = production[start..].find("\n}\n").unwrap() + start + 3;
            helpers.push_str(
                &production[start..end]
                    .replace("JsonError", "json.JsonError")
                    .replace(
                        "json_peek(data, pos)",
                        "if pos < bytes.len(data) { data[pos] } else { 0 }",
                    )
                    .replace("json_hex4", "hex4")
                    .replace(name, replacement),
            );
        }
        let source = include_str!("../../../../conformance/SEM/SEM-3-json-text-bound-36-run.hir")
            .replace("json.read_owned_text_bounded(mut cursor, limit)", "let mut requested = 0u64\n  let value = traced(mut cursor, limit, mut requested)\n  failure.check(requested <= limit, Arithmetic())\n  take value");
        let mut loaded = check(&corpus(), Some(&format!("{source}\n{helpers}\n{traced}")));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for checked in [false, true] {
            let result = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, Default::default())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, Default::default())
            }
            .unwrap();
            let observation = result
                .observation
                .unwrap_or_else(|| panic!("INCOMPLETE: {:?}", result.outcome));
            assert_eq!(observation.exit_code, Some(0), "{observation:?}");
            assert!(observation.failures.is_empty(), "{observation:?}");
            eprintln!("BOUND_PASS append-capacity-request-instrumentation checked={checked}");
        }
    });
}

fn application(app: &str, root: &Path) -> PathBuf {
    let (main, model, manifest) = match app {
        "zonetable" => (
            include_str!("../../../../examples/zonetable/main.hir"),
            include_str!("../../../../examples/zonetable/model.hir"),
            include_str!("../../../../examples/zonetable/hirsel.json"),
        ),
        "versionaudit" => (
            include_str!("../../../../examples/versionaudit/main.hir"),
            include_str!("../../../../examples/versionaudit/model.hir"),
            include_str!("../../../../examples/versionaudit/hirsel.json"),
        ),
        _ => unreachable!(),
    };
    std::fs::write(root.join("main.hir"), main).unwrap();
    std::fs::write(root.join("model.hir"), model).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let path = root.join("hirsel.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn fixture(data: Vec<u8>) -> Fixture {
    Fixture {
        args: vec!["input.json".into()],
        files: BTreeMap::from([("input/input.json".into(), data)]),
        mounts: BTreeMap::from([("input".into(), hirsel_run_schema::Access::Read)]),
        ..Default::default()
    }
}
fn error(kind: &str, offset: usize) -> Vec<u8> {
    format!("{{\"class\":\"invalid_input\",\"error\":\"{kind}\",\"row\":{},\"detail\":\"{}\",\"offset\":{offset}}}\n", if kind == "field_bytes" { 1 } else { 0 }, if kind == "json" { "Syntax" } else { "" }).into_bytes()
}
#[test]
fn versionaudit_full_main_boundaries_and_replay() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let path = application("versionaudit", scratch.path());
        let mut loaded = check(&path, None);
        let success =
            b"{\"mismatches\":[],\"summary\":{\"class\":\"ok\",\"checked\":1,\"mismatched\":0}}\n"
                .to_vec();
        let mut rows = vec![];
        for (id, key, value, over) in [
            ("success", "name", "a".into(), false),
            ("name128", "name", "é".repeat(64), false),
            ("name129", "name", format!("{}a", "é".repeat(64)), true),
            (
                "requirement256",
                "requirement",
                format!("*{}", " ".repeat(255)),
                false,
            ),
            (
                "requirement257",
                "requirement",
                format!("*{}", " ".repeat(256)),
                true,
            ),
            (
                "version256",
                "version",
                format!("1.0.0+{}", "a".repeat(250)),
                false,
            ),
            (
                "version257",
                "version",
                format!("1.0.0+{}", "a".repeat(251)),
                true,
            ),
        ] {
            let mut row = serde_json::json!({"name":"a", "requirement":"*", "version":"1.0.0"});
            row[key] = value.into();
            rows.push((
                id,
                fixture(serde_json::to_vec(&vec![row]).unwrap()),
                if over {
                    error("field_bytes", 0)
                } else {
                    success.clone()
                },
                if over { 2 } else { 0 },
            ));
        }
        let mut malformed = format!("[{{\"name\":\"{}", "a".repeat(129)).into_bytes();
        let offset = malformed.len() + 1;
        malformed.extend_from_slice(b"\\q\"}]");
        rows.push(("malformed", fixture(malformed), error("json", offset), 2));
        matrix(&path, &mut loaded, rows);
    });
}
#[test]
fn zonetable_full_main_boundaries_and_replay() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let path = application("zonetable", scratch.path());
        let mut loaded = check(&path, None);
        let long = "a".repeat(128);
        let mut rows = vec![];
        for (id, source, over) in [
            ("success", "utc.tzif".to_string(), false),
            ("source128", long.clone(), false),
            ("source129", format!("{long}a"), true),
        ] {
            let row = serde_json::json!({"local":"2024-01-01T00:00:00", "source":source, "destination":"utc.tzif"});
            let mut recipe = fixture(serde_json::to_vec(&vec![row]).unwrap());
            for name in ["utc.tzif", &long] {
                recipe.files.insert(
                    format!("input/{name}"),
                    include_bytes!("../../../../examples/zonetable/fixtures/utc.tzif").to_vec(),
                );
            }
            let output = if over {
                error("field_bytes", 0)
            } else {
                format!("[{{\"local\":\"2024-01-01T00:00:00\",\"source\":\"{source}\",\"destination\":\"utc.tzif\",\"policy\":\"Reject\",\"resolution\":\"single\",\"instant\":\"2024-01-01T00:00:00Z\",\"source_time\":\"2024-01-01T00:00:00\",\"destination_time\":\"2024-01-01T00:00:00.0+0000\",\"offset_seconds\":0}}]\n").into_bytes()
            };
            rows.push((id, recipe, output, if over { 2 } else { 0 }));
        }
        let mut malformed = format!("[{{\"local\":\"{}", "a".repeat(129)).into_bytes();
        let offset = malformed.len() + 1;
        malformed.extend_from_slice(b"\\q\"}]");
        rows.push(("malformed", fixture(malformed), error("json", offset), 2));
        matrix(&path, &mut loaded, rows);
    });
}
