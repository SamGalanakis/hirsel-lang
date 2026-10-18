use super::*;

#[test]
fn authoring_diagnostics_name_the_relevant_type_field_and_namespace() {
    for (source, rule, message) in [
        (
            "type Job { n: u64, text: String }\nfn example(tasks: Tasks) -> (Sender[Job], Receiver[Job]) { channel.open(tasks) }",
            "CONC-6",
            "Sender[unique T] and Receiver[unique T]",
        ),
        (
            "type Record { n: u64 }\nfn get(list: Record) -> u64 { list.n }",
            "LANG-4",
            "before `.`, `list` names a module",
        ),
        (
            "type TypeShape = Dot(u64) | Empty\nfn make() -> TypeShape { Dot(1) }",
            "LANG-7",
            "type `TypeShape` conflicts with a builtin type name",
        ),
    ] {
        let path = file(source);
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["check", "--json"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostic: serde_json::Value =
            serde_json::from_slice(output.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(diagnostic["rule"], rule);
        assert!(
            diagnostic["message"].as_str().unwrap().contains(message),
            "{diagnostic}"
        );
    }
    let source = "type Reply { status: u16, body: String }\nfn make(take body: String) -> unique Reply uses {} {\n  Reply(status: 200u16, body: take body)\n}\n";
    let path = file(source);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let diagnostic: serde_json::Value =
        serde_json::from_slice(output.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(diagnostic["rule"], "MODE-2");
    let start = source.rfind("take body").unwrap();
    assert_eq!(diagnostic["primary"]["start"], start);
    assert_eq!(diagnostic["primary"]["end"], start + "take body".len());
    assert_eq!(diagnostic["secondary"][0]["span"]["start"], start);
}

#[test]
fn grants_trailing_arguments_are_exact_snapshots_and_records_stay_authoritative() {
    let source = file(
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let mut i: u64 = 0
  while i < arg.count(args) {
    let text = match arg.text(args, i) { Ok(text) => text, Error(_) => "missing" }
    let _ = out.print(output, text)
    let _ = out.print(output, "\n")
    i += 1
  }
  Ok(())
}
"#,
    );
    let grants = file(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":["template"]}"#);
    for (arguments, expected) in [
        (None, "template\n"),
        (Some(vec![]), ""),
        (Some(vec!["a", "", "λ", "--named"]), "a\n\nλ\n--named\n"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&source).arg("--grants").arg(&grants);
        if let Some(arguments) = arguments {
            command.arg("--").args(arguments);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let record = run_record(
        &source,
        serde_json::json!({
            "io": {"stdout":"stdout.bytes@1", "dirs":[]}, "args":["record"]
        }),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"record\n");
    for arguments in [vec![], vec!["replacement"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--run")
            .arg(&record)
            .arg("--")
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with("--run keeps the record's Args snapshot")
        );
        assert!(output.stdout.is_empty());
    }
    // Recording retains the completed Args snapshot; replay needs no template.
    let tape = grants.with_extension("tape");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .arg("--record")
        .arg(&tape)
        .args(["--", "recorded", "λ"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, "recorded\nλ\n".as_bytes());
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--replay")
        .arg(&tape)
        .arg("--max-steps=unbounded")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, "recorded\nλ\n".as_bytes());
    std::fs::remove_file(tape).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--grants")
            .arg(&grants)
            .arg("--")
            .arg(std::ffi::OsString::from_vec(vec![255]))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("args[0]"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn inline_launch_json_works_in_the_cli_and_built_executables() {
    let source = file(
        r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let text = match arg.text(args, 0) { Ok(t) => t, Error(_) => "empty" }
  let _ = out.print(output, text)
  Ok(())
}"#,
    );
    let grants = r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":["inline λ"]}"#;
    let record_path = run_record(&source, serde_json::from_str(grants).unwrap());
    let record = std::fs::read_to_string(&record_path).unwrap();
    let exe = source.with_extension("exe");
    let build = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    for engine in ["interpreted", "native", "built"] {
        let built = engine == "built";
        for (flag, input) in [("grants", grants), ("run", record.as_str())] {
            let mut command = if built {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                if engine == "native" {
                    c.arg("--native");
                }
                c
            };
            command
                .arg(if built {
                    format!("--hirsel-{flag}")
                } else {
                    format!("--{flag}")
                })
                .arg(input);
            if built {
                command.arg("inline λ");
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{built} {flag}: {output:?}");
            assert_eq!(output.stdout, "inline λ".as_bytes());
            assert!(output.stderr.is_empty(), "{output:?}");
        }
        for bad in ["{", r#"{"io":true}"#, r#"{"args":[],"args":[]}"#] {
            let mut command = if built {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                if engine == "native" {
                    c.arg("--native");
                }
                c
            };
            let output = command
                .arg(if built { "--hirsel-grants" } else { "--grants" })
                .arg(bad)
                .output()
                .unwrap();
            assert!(!output.status.success(), "{output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
            assert!(!output.stderr.is_empty());
        }
    }
    std::fs::remove_file(exe).unwrap();
}

#[test]
fn trailing_arguments_alone_grant_only_the_exact_args_snapshot() {
    let source = file(
        r#"pub fn main(args: Args) -> Result[(), ()] {
  failure.check(arg.count(args) == 4, Arithmetic())
  for i in 0u64..arg.count(args) {
    let text = match arg.text(args, i) { Ok(t) => t, Error(_) => "invalid" }
    let expected = match i { 0 => "200", 1 => "", 2 => "λ", _ => "--flag" }
    failure.check(text == expected, Arithmetic())
  }
  Ok(())
}"#,
    );
    let manifest = source.with_extension("manifest.json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema": 1, "package": {"name": "standalone", "version": "0"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": source.file_name().unwrap().to_str().unwrap(), "module": []}],
            "dependencies": []
        })
        .to_string(),
    )
    .unwrap();
    for program in [&source, &manifest] {
        for native in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(program);
            if native {
                command.arg("--native");
            }
            let output = command
                .args(["--", "200", "", "λ", "--flag"])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }
    let empty = file(
        "pub fn main(args: Args) -> Result[(), ()] { failure.check(arg.count(args) == 0, Arithmetic()); Ok(()) }",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&empty)
        .arg("--")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let limited = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&empty)
        .args(["--max-steps", "0", "--"])
        .output()
        .unwrap();
    assert_eq!(limited.status.code(), Some(2), "{limited:?}");
    assert!(String::from_utf8_lossy(&limited.stderr).contains("Steps"));
    for root in ["rt: Io", "env: Env", "permit: Unsafe"] {
        let denied = file(&format!(
            "pub fn main(args: Args, {root}) -> Result[(), ()] {{ Ok(()) }}"
        ));
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&denied)
            .args(["--", "200", "7"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{root}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not_granted"),
            "{output:?}"
        );
        assert!(output.stdout.is_empty());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--")
            .arg(std::ffi::OsString::from_vec(vec![255]))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("args[0]"));
    }
}
