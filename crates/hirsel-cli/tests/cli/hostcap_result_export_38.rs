use super::*;

const PROGRAM: &str = r#"
resource type Marker { out: Stdout, text: String }
drop(m) { let _ = out.print(m.out, m.text) }
pub fn main(rt: Io) -> Result[(), GrantError] {
    let tasks = io.tasks(rt)?
    let output = io.stdout(rt)?
    task.scope(tasks, fn(s: Scope) {
        let a = task.async(s, fn() -> Marker { Marker(out: output, text: "answer\n") })
        let m = task.await(s, take a)
        let _ = out.print(output, "held\n")
    })
    let _ = out.print(output, "done\n")
    Ok(())
}
"#;

#[test]
fn hostcap_result_export_38_resource_result_and_replay() {
    resource_result_and_replay(PROGRAM);
}

#[test]
fn hostcap_result_export_38_nested_resource_result_and_replay() {
    let nested = PROGRAM.replace(
        "fn() -> Marker { Marker(out: output, text: \"answer\\n\") }",
        "fn() -> Option[(Marker, Stdout)] { Some((Marker(out: output, text: \"answer\\n\"), output)) }",
    );
    assert_ne!(nested, PROGRAM);
    resource_result_and_replay(&nested);
}

#[test]
fn hostcap_result_export_38_variant_resource_result_and_replay() {
    let variant = PROGRAM
        .replace(
            "pub fn main",
            "resource type Wrapped = Held(marker: Marker) | Empty\ndrop(w) { () }\npub fn main",
        )
        .replace(
            "fn() -> Marker { Marker(out: output, text: \"answer\\n\") }",
            "fn() -> Wrapped { Held(marker: Marker(out: output, text: \"answer\\n\")) }",
        );
    resource_result_and_replay(&variant);
}

fn resource_result_and_replay(program: &str) {
    let source = file(program);
    let dir = tempfile::tempdir().unwrap();
    let mut canonical = None;
    for backend in ["llvm", "cranelift"] {
        let executable = build_executable(&source, &["--backend", backend]);
        for width in [1, 4] {
            let tape = dir.path().join(format!("{backend}-{width}.tape"));
            let trace = dir.path().join(format!("{backend}.jsonl"));
            let recorded = Command::new(&executable)
            .arg("--hirsel-grants").arg(serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"placement":{"cores":width}}).to_string())
            .arg("--hirsel-record").arg(&tape)
            .arg("--hirsel-trace-events").arg(&trace)
            .output().unwrap();
            assert_eq!(recorded.status.code(), Some(0), "{backend}: {recorded:?}");
            assert_eq!(recorded.stdout, b"held\nanswer\ndone\n");
            let evidence = std::fs::read(&trace).unwrap();
            let rows: Vec<serde_json::Value> = std::str::from_utf8(&evidence)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                rows.iter()
                    .all(|row| row["schema"] == "hirsel.ordered.events@2")
            );
            assert!(
                rows.iter().any(|row| row["outcome"]["kind"] == "task_end"
                    && row["outcome"]["result"]
                        .as_str()
                        .is_some_and(|value| value.contains("06000000000000005374646f7574"))),
                "{rows:?}"
            );
            if let Some(canonical) = &canonical {
                assert_eq!(&evidence, canonical, "cross-backend canonical identity");
            } else {
                canonical = Some(evidence.clone());
            }
            hirsel_engine::with_compiler_stack(|| {
                let reference = hirsel_driver::replay_observations(&source, &tape, None).unwrap();
                let checked =
                    hirsel_driver::replay_observations_checked(&source, &tape, None).unwrap();
                assert_eq!(reference.exit_code, Some(0));
                assert_eq!(reference.output, recorded.stdout);
                assert_eq!(reference, checked, "reference/checked H exact replay");
            });
            let replayed = Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&trace)
                .output()
                .unwrap();
            assert_eq!(replayed.status.code(), Some(0), "{backend}: {replayed:?}");
            assert_eq!(replayed.stdout, recorded.stdout);
            assert_eq!(std::fs::read(&trace).unwrap(), evidence);
        }
        std::fs::remove_file(executable).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}

/// Task results are outputs, never tape inputs. Even a checksum-valid frame
/// claiming one cannot become a capability acquisition during replay.
#[test]
fn hostcap_result_export_38_forged_task_result_is_not_replay_authority() {
    let source = file(PROGRAM);
    let dir = tempfile::tempdir().unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = build_executable(&source, &["--backend", backend]);
        let tape = dir.path().join("input.tape");
        let recorded = Command::new(&executable)
            .args([
                "--hirsel-grants",
                r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
            ])
            .arg("--hirsel-record")
            .arg(&tape)
            .output()
            .unwrap();
        assert_ne!(recorded.status.code(), Some(0));
        assert!(recorded.stdout.is_empty());
        let original = std::fs::read(&tape).unwrap();
        // Keep the real bound header. Replace everything after it with a
        // well-framed fabricated task result, without a stdout launch grant.
        let header =
            hirsel_run_schema::tape::read(&original, hirsel_run_schema::tape::MAGIC.len(), 0)
                .unwrap();
        for (kind, creator, ordinal, field) in [
            ("Stdout", vec![], 1, 0),
            ("File", vec![], 1, 0),
            ("Stdout", vec![9, 9], u64::MAX, 0),
            ("Stdout", vec![], 1, u32::MAX),
        ] {
            let claimed = serde_json::json!({"kind":"task_result", "value":{"kind":kind,
                "identity":{"creator":creator,"ordinal":ordinal,"field":field}}});
            let mut bytes = original[..header.range.end].to_vec();
            hirsel_run_schema::tape::frame(&mut bytes, 1, 0x38, claimed.to_string().as_bytes())
                .unwrap();
            std::fs::write(&tape, &bytes).unwrap();
            let output = Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(3), "{backend}: {output:?}");
            assert!(output.stdout.is_empty());
            hirsel_engine::with_compiler_stack(|| {
                assert!(hirsel_driver::replay_observations(&source, &tape, None).is_err());
                assert!(hirsel_driver::replay_observations_checked(&source, &tape, None).is_err());
            });
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
        }
        // A forged acquisition in a real Event is also refused: the adapter
        // rechecks the bound grant, rather than trusting a Handle completion.
        let mut recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let event = recording
            .events
            .iter_mut()
            .find(|event| matches!(event.request, hirsel_rt::io::Request::Stdout { .. }))
            .unwrap();
        event.completion = hirsel_rt::io::Completion::Handle(Ok(0xdead_beef));
        let events = std::mem::take(&mut recording.events);
        let mut bytes = Vec::new();
        {
            let mut writer = hirsel_rt::tape_binary::Writer::new(&mut bytes, &recording).unwrap();
            for event in &events {
                writer.append(event).unwrap();
            }
            writer.finish().unwrap();
        }
        std::fs::write(&tape, &bytes).unwrap();
        let output = Command::new(&executable)
            .arg("--hirsel-replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(3),
            "forged acquisition {backend}: {output:?}"
        );
        assert!(output.stdout.is_empty());
        hirsel_engine::with_compiler_stack(|| {
            assert!(hirsel_driver::replay_observations(&source, &tape, None).is_err());
            assert!(hirsel_driver::replay_observations_checked(&source, &tape, None).is_err());
        });
        std::fs::remove_file(executable).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}
