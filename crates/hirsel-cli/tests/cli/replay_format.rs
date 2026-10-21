//! DET-1: presentation edits preserve binding; semantic/configuration edits refuse
//! before entry. Use the real CLI, binary tapes and all three execution engines.
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ENGINES: [Option<&str>; 3] = [None, Some("llvm"), Some("cranelift")];
const GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#;

fn cli(command: &str, source: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    c.arg(command)
        .arg(source)
        .env("XDG_CACHE_HOME", source.with_extension("objects"))
        .env_remove("HIRSEL_OBJECTS")
        .stdin(Stdio::null());
    c
}

fn run(source: &Path, engine: Option<&str>, args: &[&std::ffi::OsStr]) -> Output {
    let mut c = cli("run", source);
    if let Some(backend) = engine {
        c.args(["--native", "--backend", backend]);
    }
    c.args(args).output().unwrap()
}

fn hash(source: &Path) -> Vec<u8> {
    let out = cli("hash", source).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    out.stdout
}

fn record(source: &Path, tape: &Path) -> Output {
    run(
        source,
        None,
        &[
            "--grants".as_ref(),
            GRANTS.as_ref(),
            "--record".as_ref(),
            tape.as_os_str(),
        ],
    )
}

fn replay(source: &Path, engine: Option<&str>, tape: &Path) -> Output {
    run(source, engine, &["--replay".as_ref(), tape.as_os_str()])
}

fn refuses(out: Output, field: &str) {
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "entry produced an effect: {out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&format!(
            r#"replay refused: {{"kind":"binding","field":"{field}"}}"#
        )),
        "{out:?}"
    );
}

#[test]
fn formatting_comments_crlf_and_declaration_moves_replay_across_engines() {
    let original = include_str!("../../../../examples/kvd/repros/format-before.txt");
    let source = super::file(original);
    let tape = source.with_extension("tape");
    let recorded = record(&source, &tape);
    assert_eq!(recorded.status.code(), Some(0), "{recorded:?}");
    assert_eq!(recorded.stdout, b"ok\n");
    let original_hash = hash(&source);
    let binding = hirsel_rt::launch::parse_recording_bytes(&std::fs::read(&tape).unwrap()).unwrap();
    let formatted = cli("fmt", &source).output().unwrap();
    assert!(formatted.status.success(), "{formatted:?}");
    let formatted = std::fs::read_to_string(&source).unwrap();
    assert_ne!(formatted, original);
    let variants = [
        formatted.clone(),
        original.replace("rt)?", "/* before\nargument */ rt)?"),
        original
            .replace("{out", "{  out")
            .replace(";Ok", "; /* explanation */ Ok"),
        formatted.replace('\n', "\r\n"),
        format!(
            "// entry moved\n{}\n// error declaration moved\n{}\n",
            formatted
                .split_once("pub fn")
                .map(|(_, s)| format!("pub fn{s}"))
                .unwrap(),
            formatted.lines().next().unwrap()
        ),
    ];
    for text in variants {
        std::fs::write(&source, &text).unwrap();
        assert_eq!(hash(&source), original_hash, "{text}");
        let fresh_tape = source.with_extension("fresh.tape");
        let fresh = record(&source, &fresh_tape);
        assert!(fresh.status.success(), "{fresh:?}");
        let fresh =
            hirsel_rt::launch::parse_recording_bytes(&std::fs::read(&fresh_tape).unwrap()).unwrap();
        assert_eq!(fresh.run_identity, binding.run_identity);
        assert_eq!(
            fresh.binding.unwrap().canonical,
            binding.binding.as_ref().unwrap().canonical
        );
        std::fs::remove_file(fresh_tape).unwrap();
        for engine in ENGINES {
            let out = replay(&source, engine, &tape);
            assert_eq!(out.status.code(), Some(0), "{engine:?}: {text}: {out:?}");
            assert_eq!(out.stdout, recorded.stdout, "{engine:?}: {text}");
            assert!(out.stderr.is_empty(), "{out:?}");
        }
    }
    std::fs::remove_dir_all(source.with_extension("objects")).unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(tape).unwrap();
}

#[test]
fn body_callee_finalizer_manifest_and_run_binding_edits_refuse_before_effects() {
    let original = "resource type Token { stdout: Stdout }\ndrop(token) { let _ = out.print(token.stdout, \"drop\\n\"); }\nfn helper() -> String { \"ok\\n\" }\nfn other() -> String { \"other\\n\" }\npub fn main(rt: Io) -> Result[(), GrantError] { let output = io.stdout(rt)?; let token = Token(stdout: output); let _ = out.print(output, helper()); Ok(()) }\npub fn alternate(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"alternate\\n\"); Ok(()) }\n";
    let source = super::file(original);
    let tape = source.with_extension("tape");
    let recorded = record(&source, &tape);
    assert_eq!(recorded.status.code(), Some(0), "{recorded:?}");
    assert_eq!(recorded.stdout, b"ok\ndrop\n");
    for text in [
        original.replace("\"ok\\n\"", "\"changed\\n\""),
        original.replace("print(output, helper())", "print(output, other())"),
        original.replace("\"drop\\n\"", "\"changed drop\\n\""),
    ] {
        std::fs::write(&source, text).unwrap();
        for engine in ENGINES {
            refuses(replay(&source, engine, &tape), "program");
        }
    }
    std::fs::write(&source, original).unwrap();
    // An explicit selection changes the entry while every source byte stays fixed.
    let manifest = source.with_extension("hir.json");
    std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":source.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[],"entry":{"module":[],"name":"alternate"}}).to_string()).unwrap();
    for engine in ENGINES {
        refuses(replay(&source, engine, &tape), "program");
    }
    std::fs::remove_file(manifest).unwrap();
    // A valid changed grant/provider record still cannot use the old header's
    // run digest. Render the legacy fixture syntax so the tape CRC remains valid.
    let recording =
        hirsel_rt::launch::parse_recording_bytes(&std::fs::read(&tape).unwrap()).unwrap();
    let canonical: serde_json::Value =
        serde_json::from_slice(&recording.binding.as_ref().unwrap().canonical).unwrap();
    for (name, value) in [
        ("stdout", serde_json::Value::Null),
        ("random", "random.system@1".into()),
    ] {
        let mut changed = recording.clone();
        let mut record = canonical.clone();
        record["io"][name] = value;
        let run_file = source.with_extension("run.json");
        std::fs::write(&run_file, serde_json::to_vec(&record).unwrap()).unwrap();
        let run = hirsel_engine::read_run_record(&run_file).unwrap();
        std::fs::remove_file(run_file).unwrap();
        assert_ne!(run.identity(), recording.run_identity);
        changed.binding.as_mut().unwrap().canonical = run.canonical().to_vec();
        let changed_tape = source.with_extension("changed.tape");
        std::fs::write(
            &changed_tape,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        for engine in ENGINES {
            refuses(
                replay(&source, engine, &changed_tape),
                "recording.run_identity",
            );
        }
        std::fs::remove_file(changed_tape).unwrap();
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(tape).unwrap();
}

#[test]
fn selected_dependency_constants_and_manifest_versions_refuse_before_effects() {
    let marker = super::file("");
    let root = marker.with_extension("dependency-replay");
    std::fs::remove_file(marker).unwrap();
    std::fs::create_dir(&root).unwrap();
    let source = root.join("main.hir");
    std::fs::write(&source, "pub fn main(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, messages.message()); Ok(()) }\n").unwrap();
    let library = "const answer:u64=1u64\npub fn message()->String{if answer==1u64{\"ok\\n\"}else{\"other\\n\"}}\n";
    std::fs::write(root.join("lib.hir"), library).unwrap();
    let manifests = |version: &str| {
        std::fs::write(root.join("lib.json"), serde_json::json!({"schema":1,"package":{"name":"library","version":version},"sources":[{"path":"lib.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(source.with_extension("hir.json"), serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"lib.json","package":{"name":"library","version":version},"expose":[{"head":"messages","module":[]}]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    };
    manifests("1");
    let tape = root.join("run.tape");
    let recorded = record(&source, &tape);
    assert!(recorded.status.success(), "{recorded:?}");
    assert_eq!(recorded.stdout, b"ok\n");
    for text in [
        library.replace("=1u64", "=2u64"),
        library.replace("\"ok\\n\"", "\"changed\\n\""),
    ] {
        std::fs::write(root.join("lib.hir"), text).unwrap();
        for engine in ENGINES {
            refuses(replay(&source, engine, &tape), "program");
        }
    }
    std::fs::write(root.join("lib.hir"), library).unwrap();
    manifests("2");
    for engine in ENGINES {
        refuses(replay(&source, engine, &tape), "program");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn formatted_failures_replay_at_current_physical_and_generated_sites() {
    for (body, definition, generated) in [
        (
            "fn divide(x:u64)->u64{9u64/x}\npub fn main()->Result[(),()]{let _=divide(0u64);Ok(())}\n",
            "divide",
            false,
        ),
        (
            "fn body()->Result[Body[fn(u64)->u64 uses {}],String]{Ok(codegen.body(\"fn(x: u64) -> u64 uses {} {\\n  9u64 / x\\n}\"))}\nfn generated(x:u64)->u64 uses {}=comptime body()\npub fn main()->Result[(),()]{let _=generated(0u64);Ok(())}\n",
            "generated",
            true,
        ),
    ] {
        let source = super::file(body);
        let tape = source.with_extension("tape");
        let recorded = record(&source, &tape);
        assert_eq!(recorded.status.code(), Some(2), "{recorded:?}");
        assert!(cli("fmt", &source).output().unwrap().status.success());
        let formatted = std::fs::read_to_string(&source).unwrap();
        std::fs::write(&source, format!("// relocated declaration\n\n{formatted}")).unwrap();
        let expected = serde_json::json!({"schema_version":1,"runtime_failure":{"rule":"SEM-1","site":{"definition":format!("standalone@0/{definition}"),"file":if generated {format!("<generated:standalone@0/{definition}>")} else {source.to_string_lossy().into_owned()},"line":if generated {2} else {4},"column":3}}});
        for engine in ENGINES {
            let mut args = vec!["--json".as_ref(), "--replay".as_ref(), tape.as_os_str()];
            let out = run(&source, engine, &args);
            assert_eq!(out.status.code(), Some(2), "{engine:?}: {out:?}");
            assert!(out.stdout.is_empty(), "{out:?}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&out.stderr).unwrap(),
                expected,
                "{engine:?}"
            );
            // Compare an independent fresh launch with the exact replay report.
            args = vec!["--json".as_ref(), "--grants".as_ref(), "{}".as_ref()];
            let fresh = run(&source, engine, &args);
            assert_eq!(fresh.status.code(), Some(2), "{fresh:?}");
            assert_eq!(fresh.stderr, out.stderr, "{engine:?}");
        }
        std::fs::remove_dir_all(source.with_extension("objects")).unwrap();
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(tape).unwrap();
    }
}
