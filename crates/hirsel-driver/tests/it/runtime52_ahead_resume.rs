//! A host-return ahead child whose body plans Resumable enters its resumable
//! ahead code (spawn-ahead v4, resume-coverage-52): a save at a logical
//! boundary hands its frame, at the saved state, to the body's ordinary
//! dispatcher, which leaves the stack and resumes the frame there without
//! restarting the body.
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::continuation::Representation;

const SOURCE: &str = include_str!("../fixtures/e1-completed-read-35/main.hir");

fn net() -> serde_json::Value {
    serde_json::json!({
        "provider": "net.script@2",
        "rules": [{"address": "/fixture/gate.sock", "service": "0", "operation": "unix_connect"}],
        "peers": [{"address": "/fixture/gate.sock", "port": 0, "resolves_to": [], "tls": "plain",
            "connections": [
                {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                {"open": [{"expect": [71]}, {"send": [71]}, "close"]}
            ]}]
    })
}

fn checked() -> (QueryEngine, ReadyProgram) {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("main.hir", SOURCE.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let ready = build(
        &validated,
        &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap();
    (engine, ready)
}

#[test]
fn runtime52_ahead_resume_certificate() {
    with_compiler_stack(|| {
        let (_, program) = checked();
        let plan = ahead_plan(&program);
        let plans = native::continuation::plans(&program);
        let roots: Vec<_> = plan
            .roots
            .iter()
            .filter(|(_, r)| r.host.is_some())
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(roots.len(), 1, "{:?}", plan.refusals);
        let root = roots[0];
        assert_eq!(
            plans.get(root).map(|p| &p.representation),
            Some(&Representation::Resumable)
        );
        let definition = &plan.symbols[&root].definition;
        let entry = hirsel_lir::ahead::alternate_resume_name(definition, root);
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
            check_ahead_objects(&program, &objects).unwrap();
            assert_eq!(
                objects
                    .iter()
                    .flat_map(|o| &o.exports)
                    .filter(|s| s.name == entry)
                    .count(),
                1,
                "{backend:?}: {entry}"
            );
            let evidence = objects.iter().find_map(|o| o.ahead.clone()).unwrap();
            let certificates = native::continuation::ahead_certificates(&program, &objects);
            let certificate = certificates.iter().find(|c| c.root == root).unwrap();
            certificate.check(&plan, &plans, &evidence).unwrap();
            let resume = certificate.resume.as_ref().unwrap();
            assert_eq!(resume.symbol, entry);
            assert_eq!(
                resume.dispatcher,
                hirsel_lir::continuation::resume_name(definition, root, None)
            );
            assert_eq!(resume.plan, plans.get(root).unwrap().certificate.identity);
            for mutation in 0..8 {
                let mut bad = certificate.clone();
                let r = bad.resume.as_mut().unwrap();
                match mutation {
                    0 => r.symbol = bad.ahead_symbol.clone(),
                    1 => r.symbol.push('x'),
                    2 => r.dispatcher = r.symbol.clone(),
                    3 => r.dispatcher.push('x'),
                    4 => r.frame_bytes += 8,
                    5 => r.frame_bytes = 64 * 1024 + 8,
                    6 => r.plan = hirsel_syntax::ContentHash::of_bytes(b"plan"),
                    _ => bad.resume = None,
                }
                assert!(
                    bad.check(&plan, &plans, &evidence).is_err(),
                    "{backend:?}: resume mutation {mutation} accepted"
                );
            }
        }
    });
}

type Line = (String, u32, u32, String);

#[test]
fn runtime52_ahead_child_suspends_compactly_and_replays() {
    with_compiler_stack(|| {
        let (mut engine, ready) = checked();
        let fixture = Fixture {
            args: vec!["1".into(), "/fixture/gate.sock".into()],
            net: Some(serde_json::from_value(net()).unwrap()),
            ..Default::default()
        };
        let entry = entry_path(&default_context());
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 52,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let root = tempfile::Builder::new()
            .prefix("runtime52-ahead-")
            .tempdir()
            .unwrap();
        let tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(dir.join("root")).unwrap();
            let objects = native::emit_backend_objects(
                &ready,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let exe =
                native::link_standalone_objects(&ready, &entry, &objects, &tools, &dir).unwrap();
            let grants = dir.join("grants.json");
            std::fs::write(
                &grants,
                serde_json::json!({
                    "io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1",
                        "dirs": [], "net": net()},
                    "placement": {"cores": 4, "commit": "shared", "reorder": "auto", "run_ahead": 4}
                })
                .to_string(),
            )
            .unwrap();
            let tape = dir.join("run.tape");
            for replay in [false, true] {
                let cell = format!("{backend:?} replay={replay}");
                let record = dir.join("observation");
                let trace = dir.join(format!("compact-{replay}"));
                let mut command = std::process::Command::new(&exe);
                command
                    .env_clear()
                    .env("HIRSEL_RT_ROOT", dir.join("root"))
                    .env("HIRSEL_RT_DIAGNOSTIC", "1")
                    .env("HIRSEL_RT_OBSERVATION", &record)
                    .env("HIRSEL_RT_MIGRATE", "1")
                    .env("HIRSEL_RT_COMPACT_TRACE", &trace)
                    .stdin(std::process::Stdio::null());
                if replay {
                    command.arg("--hirsel-replay").arg(&tape);
                } else {
                    command
                        .arg("--hirsel-record")
                        .arg(&tape)
                        .arg("--hirsel-grants")
                        .arg(&grants)
                        .args(["1", "/fixture/gate.sock"]);
                }
                let output = command.output().unwrap();
                let launched = native::Launched {
                    audit_required: true,
                    exit: output.status.code(),
                    signal: None,
                    stdout: output.stdout,
                    stderr: output.stderr,
                    record: std::fs::read_to_string(&record).unwrap(),
                };
                assert_eq!(
                    native::observe(&launched, &ready, &sites).unwrap(),
                    expected,
                    "{cell}: {}",
                    String::from_utf8_lossy(&launched.stderr)
                );
                let lines: Vec<Line> = std::fs::read_to_string(&trace)
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| {
                        ["ahead ", "leave ", "activate "]
                            .iter()
                            .any(|kind| line.starts_with(kind))
                    })
                    .map(|line| {
                        let words: Vec<&str> = line.split(' ').collect();
                        (
                            words[0].to_owned(),
                            words[1].parse().unwrap(),
                            words[2].parse().unwrap(),
                            words[3].to_owned(),
                        )
                    })
                    .collect();
                assert!(ahead_custody(&lines, &cell) > 0, "{cell}: {lines:?}");
            }
        }
    });
}

/// Each ahead save hands over a frame past its entry; the task's next leave is
/// at exactly that state, and its activation resumes there: never the entry,
/// never twice. Returns the ahead children that left their stacks.
fn ahead_custody(trace: &[Line], label: &str) -> usize {
    let mut handed: std::collections::BTreeMap<u32, u32> = Default::default();
    let mut saved: std::collections::BTreeMap<u32, u32> = Default::default();
    let mut left = 0;
    for (kind, task, state, _) in trace {
        match kind.as_str() {
            "ahead" => {
                assert!(*state > 0, "{label}: ahead task {task} saved at its entry");
                assert!(
                    handed.insert(*task, *state).is_none() && !saved.contains_key(task),
                    "{label}: ahead task {task} handed over twice"
                );
            }
            "leave" => {
                assert!(*state > 0, "{label}: task {task} saved at its entry");
                if let Some(at) = handed.remove(task) {
                    assert_eq!(*state, at, "{label}: ahead task {task} left another state");
                    left += 1;
                }
                assert!(
                    saved.insert(*task, *state).is_none(),
                    "{label}: task {task} saved twice without an activation"
                );
            }
            "activate" => {
                let at = saved
                    .remove(task)
                    .unwrap_or_else(|| panic!("{label}: task {task} activated unsaved"));
                assert_eq!(*state, at, "{label}: task {task} resumed another state");
            }
            other => panic!("{label}: {other}"),
        }
    }
    assert!(
        saved.is_empty(),
        "{label}: frames never activated: {saved:?}"
    );
    left
}
