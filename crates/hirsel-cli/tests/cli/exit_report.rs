use super::*;
#[test]
fn returned_errors_report_payloads_without_stderr_grants_and_replay_them() {
    for (text, expected) in [
        (
            "pub fn main() -> Result[(), String] { Error(unique \"missing data\") }",
            "error: missing data\n",
        ),
        (
            "pub type Trouble = Missing(String, u64) | Other\npub fn main() -> Result[(), Trouble] { Error(Missing(\"data\", 7u64)) }",
            "error: Missing(\"data\", 7)\n",
        ),
        (
            "pub type Trouble = Missing(path: String, code: i64, detail: box Option[List[u8]]) | Other\npub fn main() -> Result[(), Trouble] { Error(Missing(path: \"data\", code: -7i64, detail: box Some([1u8, 2u8]))) }",
            "error: Missing { path: \"data\", code: -7, detail: box Some([1, 2]) }\n",
        ),
        (
            "pub type Trouble = Missing(path: String, code: i64, detail: box Option[List[u8]]) | Other\npub fn main() -> Result[(), Trouble] { Error(Other()) }",
            "error: Other\n",
        ),
        (
            "pub fn main() -> Result[(), List[u8]] { Error([1u8, 2u8]) }",
            "error: [1, 2]\n",
        ),
        ("pub fn main() -> Result[(), String] { Ok(()) }", ""),
    ] {
        let source = file(text);
        let grants = source.with_extension("grants.json");
        std::fs::write(&grants, "{}").unwrap();
        let tape = source.with_extension("tape");
        let exe = build_executable(&source, &[]);
        let code = if expected.is_empty() { 0 } else { 1 };
        // Simulation, interpreted host launch, native source run and standalone.
        for mode in 0..4 {
            let mut command = if mode == 3 {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                if mode == 2 {
                    c.arg("--native");
                }
                c
            };
            if mode != 0 {
                command
                    .arg(if mode == 3 {
                        "--hirsel-record"
                    } else {
                        "--record"
                    })
                    .arg(&tape)
                    .arg(if mode == 3 {
                        "--hirsel-grants"
                    } else {
                        "--grants"
                    })
                    .arg(&grants);
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(code), "mode={mode}: {output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                expected,
                "mode={mode}"
            );
            if mode != 0 {
                // Replay across engines, with no grants file available. The report
                // is recreated from the return value; it is not a stderr Io event.
                std::fs::remove_file(&grants).unwrap();
                let recorded = std::fs::read_to_string(&tape).unwrap();
                assert!(!recorded.contains("stderr"), "{recorded}");
                let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .arg(&source)
                    .arg("--replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(code), "{output:?}");
                assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
                let output = Command::new(&exe)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(code), "{output:?}");
                assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
                std::fs::write(&grants, "{}").unwrap();
            }
        }
        for p in [source, grants, tape, exe] {
            std::fs::remove_file(p).unwrap();
        }
    }
}

#[test]
fn buffered_stdout_is_flushed_before_the_host_exit_report() {
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    let source = file(
        "pub fn main(rt: Io) -> Result[(), GrantError] { let output = io.stdout(rt)?; let _ = out.print(output, \"before\"); Error(NotGranted()) }",
    );
    let grants = source.with_extension("grants.json");
    std::fs::write(&grants, r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#).unwrap();
    let world = source.with_extension("world.json");
    std::fs::write(&world, "{}").unwrap();
    let exe = build_executable(&source, &[]);
    let tape = source.with_extension("tape");
    for mode in 0..6 {
        let mut command = if mode == 3 || mode == 5 {
            Command::new(&exe)
        } else {
            let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            c.arg("run").arg(&source);
            if mode == 2 {
                c.arg("--native");
            }
            c
        };
        if mode >= 4 {
            command
                .arg(if mode == 5 {
                    "--hirsel-replay"
                } else {
                    "--replay"
                })
                .arg(&tape);
        } else if mode == 0 {
            command.arg("--simulation").arg(&world);
        } else {
            if mode == 3 {
                command.arg("--hirsel-record").arg(&tape);
            }
            command
                .arg(if mode == 3 {
                    "--hirsel-grants"
                } else {
                    "--grants"
                })
                .arg(&grants);
        }
        let (mut reader, writer) = UnixStream::pair().unwrap();
        command
            .stdout(Stdio::from(OwnedFd::from(writer.try_clone().unwrap())))
            .stderr(Stdio::from(OwnedFd::from(writer)));
        let mut child = command.spawn().unwrap();
        drop(command);
        assert_eq!(child.wait().unwrap().code(), Some(1));
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        assert_eq!(output, "beforeerror: NotGranted\n", "mode={mode}");
    }
    for p in [source, grants, world, exe, tape] {
        std::fs::remove_file(p).unwrap();
    }
}

#[test]
fn returned_resources_finalize_after_the_report_in_both_engines() {
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = file(
        &std::fs::read_to_string(
            root.join("conformance/PROG/PROG-3-exit-report-resource-error-run.hir"),
        )
        .unwrap(),
    );
    let grants = source.with_extension("grants.json");
    std::fs::write(&grants, r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#).unwrap();
    let world = source.with_extension("world.json");
    std::fs::write(&world, "{}").unwrap();
    let exe = build_executable(&source, &[]);
    let tape = source.with_extension("tape");
    for mode in 0..6 {
        let mut command = if mode == 3 || mode == 5 {
            Command::new(&exe)
        } else {
            let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            c.arg("run").arg(&source);
            if mode == 2 {
                c.arg("--native");
            }
            c
        };
        if mode >= 4 {
            command
                .arg(if mode == 5 {
                    "--hirsel-replay"
                } else {
                    "--replay"
                })
                .arg(&tape);
        } else if mode == 0 {
            command.arg("--simulation").arg(&world);
        } else {
            if mode == 3 {
                command.arg("--hirsel-record").arg(&tape);
            }
            command
                .arg(if mode == 3 {
                    "--hirsel-grants"
                } else {
                    "--grants"
                })
                .arg(&grants);
        }
        let (mut reader, writer) = UnixStream::pair().unwrap();
        command
            .stdout(Stdio::from(OwnedFd::from(writer.try_clone().unwrap())))
            .stderr(Stdio::from(OwnedFd::from(writer)));
        let mut child = command.spawn().unwrap();
        drop(command);
        assert_eq!(child.wait().unwrap().code(), Some(1));
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        assert_eq!(
            output, "beforeerror: { stdout: <Stdout> }\ndrop\n",
            "mode={mode}"
        );
    }
    for p in [source, grants, world, exe, tape] {
        std::fs::remove_file(p).unwrap();
    }
}
#[test]
fn nonzero_cli_exits_report_missing_and_denied_grants_and_json_causes() {
    let source = file(
        "pub fn main(rt: Io) -> Result[(), FileError] { let _ = io.dir(rt, path.of(\"data\"))?; Ok(()) }",
    );
    let grants = source.with_extension("grants.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(grants.file_name().unwrap().to_str().unwrap()),
        "{output:?}"
    );
    std::fs::write(&grants, r#"{"io":{"dirs":[]}}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(output.stderr, b"error: NotGranted\n");
    let bad = file("pub fn main() -> Result[(), ()] { let n: u64 = true; Ok(()) }");
    for source in [&bad, &bad.with_extension("missing.hir")] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["check", "--json"])
            .arg(source)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(!output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).lines().count(),
            1,
            "{output:?}"
        );
        let first = output.stdout.split(|b| *b == b'\n').next().unwrap();
        assert!(
            serde_json::from_slice::<serde_json::Value>(first).is_ok(),
            "{output:?}"
        );
    }
    for p in [source, grants, bad] {
        std::fs::remove_file(p).unwrap();
    }
}
