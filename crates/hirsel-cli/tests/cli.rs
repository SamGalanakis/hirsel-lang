use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn help_spellings_print_usage_to_stdout_and_exit_zero() {
    for command in ["--help", "help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg(command)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{command}");
        assert_eq!(
            output.stdout,
            b"usage: hirsel check [--json] <source|manifest.json> | run [--native | --simulation <json>] <source|manifest.json> [--run <json> | --grants <json>] [--max-steps <n>] | build [--release | --debug] [--target x86_64-linux | aarch64-linux] <source|manifest.json> -o <exe> | fmt [--check] <file> | export --tir|--run <source|manifest.json> -o <file>\n",
            "{command}"
        );
        assert!(output.stderr.is_empty(), "{command}");
    }
}

fn file(text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hirsel-cli-{}-{}.hir",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).unwrap();
    if text.contains("std@1/") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let sidecar = format!("{}.json", path.display());
        std::fs::write(sidecar, format!(r#"{{"schema":1,"package":{{"name":"standalone","version":"0"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[{{"manifest":"{}","package":{{"name":"std","version":"1"}},"expose":[]}}]}}"#, path.display(), manifest.display())).unwrap();
    }
    path
}
#[test]
fn export_tir_writes_the_proof_seam_export_and_rejected_source_exits_one() {
    let path = file(
        "fn add(x: u64) -> u64 { x + 1u64 }\npub fn main() -> Result[(), ()] { let n = add(1u64); Ok(()) }\n",
    );
    let out = path.with_extension("tir");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--tir"])
        .arg(&path)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.starts_with("(Export\n  (Header \"hirsel.tir.export.v1\""),
        "{text}"
    );
    assert!(text.contains("\"SEM-1\" \"failure-edge\" 0"), "{text}");
    let bad = file("pub fn main() -> Result[(), ()] { let n: u64 = true; Ok(()) }\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--tir"])
        .arg(&bad)
        .arg("-o")
        .arg(bad.with_extension("tir"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!bad.with_extension("tir").exists());
}
#[test]
fn binary_usage_and_io_errors_have_distinct_codes() {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--bad", "file.hir"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    let missing = std::env::temp_dir().join(format!("hirsel-missing-{}.hir", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("check")
        .arg(missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}
#[test]
fn command_options_and_operand_counts_remain_specific_to_each_command() {
    for args in [
        vec![],
        vec!["--help", "extra"],
        vec!["check"],
        vec!["check", "--native", "file.hir"],
        vec!["check", "file.hir", "--json"],
        vec!["run", "--json", "file.hir"],
        vec!["run", "--simulation", "config.json"],
        vec!["run", "--native", "file.hir", "extra"],
        vec!["fmt", "--json", "file.hir"],
        vec!["fmt", "--check", "-file.hir"],
        vec!["export", "--tir", "file.hir"],
        vec!["export", "--tir", "file.hir", "--output", "out.tir"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .nth(1)
                .is_some_and(|line| line.starts_with("usage: hirsel ")),
            "{args:?}"
        );
    }
    // Standard lexopt spelling reaches IO rather than a usage error.
    for args in [
        vec!["check", "--", "/missing-hirsel-parser-input.hir"],
        vec![
            "export",
            "--tir",
            "/missing-hirsel-parser-input.hir",
            "-ounused.tir",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("usage:"));
    }
}

#[test]
fn real_pipeline_accepts_clean_source_and_checks_launch_shape() {
    let path = file("fn main() {}\n");
    for flags in [vec!["check"], vec!["check", "--json"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(flags)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("entry must be pub fn"));
    std::fs::remove_file(path).unwrap();
    // A program without its entry checks, and running it names the entry.
    let path = file("fn probe() -> u64 {\n  1\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("declares no entry `pub fn main`"),
        "{stderr}"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn lexical_rejection_uses_human_and_json_renderers() {
    let path = file("fn main() {}\n// \u{202e}\n");
    let human = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(1));
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("error[SEM-3]"));
    assert!(stderr.contains(":2:4"));
    let json = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(diagnostic["schema_version"], 1);
    assert_eq!(diagnostic["rule"], "SEM-3");
    assert_eq!(diagnostic["primary"]["line"], 2);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn syntax_rejection_and_real_formatting_cover_exit_codes() {
    let invalid = file("fn main<T>() {}\n");
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&invalid)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(diagnostic["rule"], "LANG-1");
    assert_eq!(diagnostic["primary"]["line"], 1);
    let path = file("fn  main ( ) { }\n");
    let before = std::fs::read(&path).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("fmt")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    for path in [invalid, path] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn spec_opening_example_checks_formats_and_runs_with_simulated_directory() {
    let path = file(include_str!("fixtures/opening.hir"));
    let sidecar = path.with_extension("hir.json");
    let manifest = serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[{"manifest":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
    std::fs::write(&sidecar, manifest.to_string()).unwrap();
    let config =
        file(r#"{"args":["tree"],"files":{"tree/a.txt":"x\r\ny","tree/b.txt":"unchanged"}}"#);
    for flags in [vec!["check"], vec!["check", "--json"], vec!["fmt"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(flags)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"1");
    for path in [path, config, sidecar] {
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn simulation_input_errors_are_tool_errors() {
    let source = file("fn main() {}\n");
    let config = file(r#"{"files":{"a":42}}"#);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected string values"));
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn unavailable_execution_preserves_stdout_before_the_tool_error() {
    let source = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let frozen: Shared[u8] = shared.freeze(7u8); Ok(()) }",
    );
    let config = run_record(
        &source,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}),
    );
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"before unavailable\n");
    assert!(String::from_utf8_lossy(&result.stderr).contains("not implemented"));
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(config).unwrap();
}

#[test]
fn violated_unchecked_contract_is_detected_with_prior_stdout() {
    let source = file(
        "pub fn main(rt: Io, args: Args, permit: Unsafe) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"printed before the call\\n\")\n  let a: u8 = 200\n  let bad: u8 = int.unchecked_add(permit, a, 56)\n  Ok(())\n}\n",
    );
    let config = file(r#"{"grants":["Io","Args","Unsafe"]}"#);
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(result.stdout, b"printed before the call\n");
    let message = String::from_utf8(result.stderr).unwrap();
    assert!(
        message.contains("undefined behaviour detected: int.unchecked_add at line 4"),
        "{message}"
    );
    assert!(message.contains("sum must fit"));
    assert!(!message.contains("runtime failure"));
    assert!(!message.contains("invalid TIR"));
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn invalid_utf8_is_a_located_language_diagnostic_for_every_entry_point() {
    let path = file("");
    let prefix = "// 日\nfn f() {}\n";
    let mut bytes = prefix.as_bytes().to_vec();
    bytes.extend_from_slice(&[0xff, b'x']);
    std::fs::write(&path, &bytes).unwrap();
    let json = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(diagnostic["rule"], "SEM-3");
    assert_eq!(
        diagnostic["primary"]["file"],
        path.to_string_lossy().as_ref()
    );
    assert_eq!(diagnostic["primary"]["start"], prefix.len());
    assert_eq!(diagnostic["primary"]["end"], prefix.len());
    assert_eq!(diagnostic["primary"]["line"], 3);
    assert_eq!(diagnostic["primary"]["column"], 1);
    for command in ["check", "fmt", "run"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg(command)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let human = String::from_utf8(output.stderr).unwrap();
        assert!(human.contains("error[SEM-3]"), "{command}: {human}");
        assert!(human.contains(":3:1"), "{command}: {human}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_file(path).unwrap();
}

/// `run --native` reports what `run` reports: the same first static
/// diagnostic, the same output and exit status, the same failure line. A
/// program native code cannot build yet is a tool outcome (exit 2) whose
/// source verdict is unchanged.
#[test]
fn native_run_matches_the_reference_run() {
    let run = |native: bool, path: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        command.arg(path).output().unwrap()
    };
    let prints = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"hi\\n\")\n  Ok(())\n}\n",
    );
    let fails = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let a: u8 = 255\n  let b = a + 1\n  Ok(())\n}\n",
    );
    let rejected = file("pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  missing\n}\n");
    let map = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let mut m: Map[u64, u64] = map.empty()\n  map.insert(mut m, 4u64, 1u64)\n  Ok(())\n}\n",
    );
    let decimal = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let text = std@1/u64.to_string(4u64)\n  let _ = out.print(io.stdout(rt)?, if text == \"4\" { \"ok\\n\" } else { \"bad\\n\" })\n  Ok(())\n}\n",
    );
    let directory = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let dir = io.dir(rt, path.of(\".\"))\n  Ok(())\n}\n",
    );
    for path in [&prints, &fails, &rejected, &map, &decimal, &directory] {
        let (reference, native) = (run(false, path), run(true, path));
        assert_eq!(native.status.code(), reference.status.code(), "{path:?}");
        assert_eq!(native.stdout, reference.stdout, "{path:?}");
        assert_eq!(
            String::from_utf8_lossy(&native.stderr),
            String::from_utf8_lossy(&reference.stderr),
            "{path:?}"
        );
    }
    assert_eq!(run(true, &prints).stdout, b"hi\n");
    assert_eq!(run(true, &fails).status.code(), Some(1));
    assert_eq!(run(true, &decimal).stdout, b"ok\n");
    // Acquiring Net is supported independently of M2 network operations.
    let network = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let net = io.net(rt)?\n  Ok(())\n}\n",
    );
    let reference = run(false, &network);
    let native = run(true, &network);
    assert_eq!(reference.status.code(), Some(0));
    assert_eq!(native.status.code(), Some(0));
    assert_eq!(native.stdout, reference.stdout);
    assert_eq!(native.stderr, reference.stderr);
}

#[test]
fn source_projection_controls_distinguish_granted_and_empty_io() {
    for op in ["stdout", "tasks", "net", "random"] {
        let source = file(&format!(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{ let output = io.{op}(rt)?; Ok(()) }}"
        ));
        for granted in [false, true] {
            let config = file(
                &serde_json::json!({
                    "grants": ["Io", "Args"],
                    "projections": if granted { vec![op] } else { vec![] },
                    "args": [],
                })
                .to_string(),
            );
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["run", "--simulation"])
                .arg(&config)
                .arg(&source)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if granted { 0 } else { 1 }),
                "{op}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
            std::fs::remove_file(config).unwrap();
        }
        std::fs::remove_file(source).unwrap();
    }
}

/// Generate the real semantic binding through the public CLI, then select grants.
fn run_record(source: &std::path::Path, grants: serde_json::Value) -> std::path::PathBuf {
    let record = source.with_extension("run.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--run"])
        .arg(source)
        .arg("-o")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    for (key, value_grant) in grants.as_object().unwrap() {
        value[key] = value_grant.clone();
    }
    std::fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
    record
}

#[test]
fn hello_web_concurrent_serves_while_another_request_is_incomplete() {
    hello_web(false, true);
}

#[test]
fn hello_web_answers_real_requests_interpreted() {
    hello_web(false, false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_answers_real_requests_native() {
    hello_web(true, false);
}

fn hello_web(native: bool, concurrent: bool) {
    use std::io::{BufRead, Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Duration;
    {
        let original = if concurrent {
            include_str!("../../../examples/hello-web-concurrent/main.hir")
        } else {
            include_str!("../../../examples/hello-web/main.hir")
        };
        let source = original
            .replace("8080u16", "0u16")
            .replace(
                "while true",
                r#"
let bound = net.local_addr(n, listener) ? Network
let output = io.stdout(rt) ? Grant
let _ = out.print(output, std@1/u16.to_string(bound.port))
let _ = out.print(output, "\n")
for i in 0u64..COUNTu64
"#,
            )
            .replace("COUNTu64", if concurrent { "4u64" } else { "3u64" });
        let source = if concurrent {
            source.replace(
                "net.local_addr(n, listener)",
                "net.local_addr(n, accepting)",
            )
        } else {
            source
        };
        let path = file(&source);
        let manifest = path.with_extension("hir.json");
        let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(&manifest,serde_json::json!({"schema":1,"package":{"name":"hello_web","version":"1"},"sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let tasks = if concurrent {
            serde_json::json!("tasks.cooperative@1")
        } else {
            serde_json::Value::Null
        };
        let record = run_record(
            &path,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":tasks,"dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"0","operation":"listen"}]}},"args":[]}),
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&path).arg("--run").arg(&record);
        if native {
            command.arg("--native");
        }
        let child = command
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        struct Server(std::process::Child, bool);
        impl Drop for Server {
            fn drop(&mut self) {
                if !self.1 {
                    unsafe {
                        libc_kill_group(self.0.id());
                    }
                }
                let _ = self.0.wait();
            }
        }
        unsafe fn libc_kill_group(pid: u32) {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(-(pid as i32), 9);
            }
        }
        let mut server = Server(child, false);
        let stdout = server.0.stdout.take().unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let (_, port) = recv
            .recv_timeout(Duration::from_secs(90))
            .unwrap_or_else(|e| panic!("server readiness native={native}: {e}"));
        let port: u16 = port.trim().parse().unwrap();
        // An incomplete first request must not prevent later clients completing.
        let mut held = concurrent.then(|| {
            let mut stream =
                std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream.write_all(b"GET / HTTP/1.1\r\nHost:").unwrap();
            stream
        });
        for (request,expected) in [
            (&b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],&b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world"[..]),
            (b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\nabc",b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world"),
            (b"BAD\0 / HTTP/1.1\r\n\r\n",b"HTTP/1.1 400 \r\nContent-Length: 11\r\nConnection: close\r\n\r\nBad Request"),
        ] {
            let mut stream=std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            stream.write_all(request).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut response=vec![];stream.read_to_end(&mut response).unwrap();assert_eq!(response,expected,"native={native}");
        }
        if let Some(held) = &mut held {
            held.write_all(b" localhost\r\n\r\n").unwrap();
            held.shutdown(std::net::Shutdown::Write).unwrap();
            let mut response = vec![];
            held.read_to_end(&mut response).unwrap();
            assert!(response.ends_with(b"Hello, world"));
        }
        let status = server.0.wait().unwrap();
        server.1 = true;
        let mut stderr = String::new();
        server
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(status.success(), "native={native}: {stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        reader.join().unwrap();
        for path in [path, manifest, record] {
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn run_records_enforce_exact_listen_grants_and_admission_in_both_engines() {
    let source = file(
        r#"
fn expected_error(e: NetError) -> () uses {} {
  failure.check(match e { NotGranted() => true, _ => false }, Arithmetic())
}
fn grant_error(e: GrantError) -> () uses {} { () }
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let n = io.net(rt) ? grant_error
  let listener = net.listen(n, SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)) ? expected_error
  Ok(())
}
"#,
    );
    for native in [false, true] {
        for (address, service, operation, allowed) in [
            ("127.0.0.1", "0", "listen", true),
            ("127.0.0.2", "0", "listen", false),
            ("127.0.0.1", "1", "listen", false),
            ("127.0.0.1", "0", "connect", false),
            ("127.0.0.1", "0", "connect_tls", false),
        ] {
            let record = run_record(
                &source,
                serde_json::json!({"io":{"dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":address,"service":service,"operation":operation}]}},"args":[if allowed {"allow"}else{"deny"}]}),
            );
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source).arg("--run").arg(&record);
            if native {
                command.arg("--native");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(if allowed { 0 } else { 1 }),
                "native={native} {address}:{service}/{operation}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
            std::fs::remove_file(record).unwrap();
        }
        for grants in [
            serde_json::json!({"io":{"dirs":[],"net":{"provider":"net.stream@1","rules":[]}},"args":["deny"]}),
            serde_json::json!({"io":{"dirs":[]},"args":["deny"]}),
        ] {
            let record = run_record(&source, grants);
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source).arg("--run").arg(&record);
            if native {
                command.arg("--native");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::remove_file(record).unwrap();
        }
        // These refuse before the entry can bind a socket.
        for grants in [
            serde_json::json!({"args":[]}),
            serde_json::json!({"io":{"dirs":[]}}),
            serde_json::json!({"io":{"dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":"*","service":"0","operation":"listen"}]}},"args":[]}),
            serde_json::json!({"io":{"dirs":[]},"args":[],"admission":{"expires":0}}),
            serde_json::json!({"io":{"dirs":[]},"args":[],"program":"0000000000000000000000000000000000000000000000000000000000000000"}),
        ] {
            let record = run_record(&source, grants);
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source).arg("--run").arg(&record);
            if native {
                command.arg("--native");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
            std::fs::remove_file(record).unwrap();
        }
    }
    std::fs::remove_file(source).unwrap();
}

#[test]
fn run_records_pin_directory_rights_and_args_in_both_engines() {
    let source = file(
        r#"
fn file_error(e: FileError) -> () uses {} { () }
fn grant_error(e: GrantError) -> () uses {} { () }
fn arg_error(e: ArgError) -> () uses {} { () }
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let d = io.dir(rt, path.of("data")) ? file_error
  let input = arg.path(args, 0) ? arg_error
  let data = file.read_bytes(d, input) ? file_error
  if data != b"input" { return Error(()) }
  let written: bool = match file.write_bytes(d, path.of("output.txt"), b"written") { Ok(()) => true, Error(NotGranted()) => false, Error(_) => return Error(()) }
  let output = io.stdout(rt) ? grant_error
  let _ = out.print(output, if written { "written" } else { "read only" })
  Ok(())
}
"#,
    );
    let root = source.with_extension("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("input.txt"), b"input").unwrap();
    for native in [false, true] {
        for access in ["read", "read_write"] {
            let record = run_record(
                &source,
                serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":root,"access":access}]},"args":["input.txt"]}),
            );
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source).arg("--run").arg(&record);
            if native {
                command.arg("--native");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "native={native}, {access}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                output.stdout,
                if access == "read" {
                    b"read only".as_slice()
                } else {
                    b"written"
                }
            );
            assert!(output.stderr.is_empty());
            if access == "read" {
                assert!(!root.join("output.txt").exists());
            } else {
                assert_eq!(std::fs::read(root.join("output.txt")).unwrap(), b"written");
                std::fs::remove_file(root.join("output.txt")).unwrap();
            }
            std::fs::remove_file(record).unwrap();
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(source).unwrap();
}

fn build_executable(source: &std::path::Path, profile: &[&str]) -> std::path::PathBuf {
    let executable = source.with_extension(format!("{}-exe", profile.join("-")));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .args(profile)
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    executable
}
fn build_run_record(source: &std::path::Path) -> serde_json::Value {
    let mut loaded = hirsel_driver::load_program(source).unwrap();
    loaded.check_for_execution().unwrap();
    let revision = loaded
        .engine
        .semantic_program_revision(&loaded.entry)
        .unwrap();
    serde_json::json!({
        "schema":1,"program":revision.digest().iter().map(|b|format!("{b:02x}")).collect::<String>(),
        "entry":revision.entry(),"launch":"main",
        "inputs":{"hash_seed":0,"random_seed":0,"test_seed":0,"scheduler":null},
        "io":null,"args":null,"env":null,"unsafe":false,"admission":null
    })
}
fn stdout_grant() -> serde_json::Value {
    serde_json::json!({"stdout":"stdout.bytes@1","tasks":null,"monotonic_clock":null,
        "wall_clock":null,"random":null,"dirs":[],"net":null})
}
#[test]
fn standalone_hello_runs_directly_with_fresh_grants_and_only_libc() {
    hirsel_engine::with_compiler_stack(|| {
        let source = file(
            "pub fn main(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"hello, world\\n\"); Ok(()) }",
        );
        let exe = build_executable(&source, &[]);
        let mut record = build_run_record(&source);
        record["io"] = stdout_grant();
        let run = source.with_extension("run.json");
        std::fs::write(&run, record.to_string()).unwrap();
        let output = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, b"hello, world\n");
        assert!(output.stderr.is_empty(), "{output:?}");
        let denied = Command::new(&exe)
            .env("HIRSEL_RT_ROOT", "/")
            .env("HIRSEL_RT_NET", "invalid")
            .output()
            .unwrap();
        assert_eq!(denied.status.code(), Some(3), "{denied:?}");
        assert!(denied.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&denied.stderr).contains("not_granted"),
            "{denied:?}"
        );
        // A fresh config can change grants and inputs without rebuilding the executable.
        record["io"]["stdout"] = serde_json::Value::Null;
        record["inputs"]["hash_seed"] = 42.into();
        std::fs::write(&run, record.to_string()).unwrap();
        let denied = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(denied.status.code(), Some(1), "{denied:?}");
        assert!(denied.stdout.is_empty());
        record["io"] = stdout_grant();
        record["admission"] = serde_json::json!({"not_before":null,"expires":0,"revocation":null});
        std::fs::write(&run, record.to_string()).unwrap();
        let expired = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(expired.status.code(), Some(3), "{expired:?}");
        assert!(expired.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&expired.stderr).contains("expired"),
            "{expired:?}"
        );
        record["admission"] = serde_json::Value::Null;
        record["program"] = "00".repeat(32).into();
        std::fs::write(&run, record.to_string()).unwrap();
        let wrong = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(wrong.status.code(), Some(3), "{wrong:?}");
        assert!(
            String::from_utf8_lossy(&wrong.stderr).contains("binding"),
            "{wrong:?}"
        );
        let readobj = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap())
            .join("bin/llvm-readobj");
        let dynamic = Command::new(readobj)
            .arg("--needed-libs")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(dynamic.status.success(), "{dynamic:?}");
        let text = String::from_utf8(dynamic.stdout).unwrap();
        let libs: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with(".so.6") || line.contains(".so."))
            .collect();
        assert_eq!(libs, ["ld-linux-x86-64.so.2", "libc.so.6"], "{text}");
        for path in [source, exe, run] {
            std::fs::remove_file(path).unwrap();
        }
    });
}
#[test]
fn standalone_reads_a_granted_file_using_process_args_in_source_root_order() {
    hirsel_engine::with_compiler_stack(|| {
        let source = file(
            "pub type Error = Usage(ArgError) | Files(FileError) | Grant(GrantError)\npub fn main(args: Args, rt: Io) -> Result[(), Error] { let root = arg.path(args, 0) ? Usage; let dir = io.dir(rt, root) ? Files; let bytes = file.read_bytes(dir, path.of(\"hello.txt\")) ? Files; let _ = out.print(io.stdout(rt) ? Grant, if bytes == b\"from disk\" { \"file ok\\n\" } else { \"wrong\\n\" }); Ok(()) }",
        );
        let exe = build_executable(&source, &["--release"]);
        let root = source.with_extension("files");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "from disk").unwrap();
        let mut record = build_run_record(&source);
        record["io"] = stdout_grant();
        record["io"]["dirs"] = serde_json::json!([{"path":"data","host":root,"access":"read"}]);
        // A record's Args are replaced by the explicit process snapshot at admission.
        record["args"] = serde_json::json!(["ignored"]);
        let run = source.with_extension("run.json");
        std::fs::write(&run, record.to_string()).unwrap();
        let output = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .arg("data")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, b"file ok\n");
        record["io"]["dirs"] = serde_json::json!([]);
        std::fs::write(&run, record.to_string()).unwrap();
        let denied = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .arg("data")
            .output()
            .unwrap();
        assert_eq!(denied.status.code(), Some(1), "{denied:?}");
        assert!(denied.stdout.is_empty());
        for path in [source, exe, run] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    });
}
#[test]
fn standalone_cross_target_runs_under_pinned_qemu_and_debug_has_line_tables() {
    hirsel_engine::with_compiler_stack(|| {
        let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
        let cross = build_executable(&source, &["--target", "aarch64-linux"]);
        let output = Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap())
            .arg("-L")
            .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
            .arg(&cross)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty(), "{output:?}");
        let manifest = source.with_extension("manifest.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":source,"module":[]}],"dependencies":[]}).to_string()).unwrap();
        let debug = build_executable(&manifest, &["--debug"]);
        let output = Command::new(&debug).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let dwarf = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap())
            .join("bin/llvm-dwarfdump");
        let lines = Command::new(dwarf)
            .arg("--debug-line")
            .arg(&debug)
            .output()
            .unwrap();
        assert!(lines.status.success(), "{lines:?}");
        assert!(
            String::from_utf8_lossy(&lines.stdout)
                .contains(source.file_name().unwrap().to_str().unwrap()),
            "{lines:?}"
        );
        for path in [source, cross, debug, manifest] {
            std::fs::remove_file(path).unwrap();
        }
    });
}

#[test]
fn standalone_admits_selected_roots_and_explicit_argv_without_ambient_grants() {
    hirsel_engine::with_compiler_stack(|| {
        let source = file(
            "pub fn main(environment: Env, args: Args, rt: Io, permit: Unsafe) -> Result[(), ()] { Ok(()) }",
        );
        let exe = build_executable(&source, &[]);
        let run = source.with_extension("run.json");
        let mut record = build_run_record(&source);
        record["io"] = serde_json::json!({"stdout":null,"tasks":null,"monotonic_clock":null,"wall_clock":null,"random":null,"dirs":[],"net":null});
        record["env"] = serde_json::json!({});
        record["unsafe"] = true.into();
        std::fs::write(&run, record.to_string()).unwrap();
        let output = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        record["env"] = serde_json::Value::Null;
        std::fs::write(&run, record.to_string()).unwrap();
        let denied = Command::new(&exe)
            .arg("--hirsel-run")
            .arg(&run)
            .output()
            .unwrap();
        assert_eq!(denied.status.code(), Some(3), "{denied:?}");
        assert!(
            String::from_utf8_lossy(&denied.stderr).contains("Env"),
            "{denied:?}"
        );
        for path in [source, exe, run] {
            std::fs::remove_file(path).unwrap();
        }
        let source = file(
            "pub fn main(args: Args) -> Result[(), ()] { match arg.path(args, 0) { Ok(_) => Ok(()), Error(_) => Error(()) } }",
        );
        let exe = build_executable(&source, &[]);
        let accepted = Command::new(&exe)
            .args(["--", "argument"])
            .output()
            .unwrap();
        assert_eq!(accepted.status.code(), Some(0), "{accepted:?}");
        let empty = Command::new(&exe).arg("--").output().unwrap();
        assert_eq!(empty.status.code(), Some(1), "{empty:?}");
        let absent = Command::new(&exe).output().unwrap();
        assert_eq!(absent.status.code(), Some(3), "{absent:?}");
        use std::os::unix::ffi::OsStringExt;
        let invalid = Command::new(&exe)
            .arg("--")
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(3), "{invalid:?}");
        for path in [source, exe] {
            std::fs::remove_file(path).unwrap();
        }
    });
}

#[test]
fn build_rejects_invalid_options_and_source_before_writing_an_executable() {
    for args in [
        vec!["build", "source.hir"],
        vec![
            "build",
            "--debug",
            "--release",
            "source.hir",
            "-o",
            "unused",
        ],
        vec!["build", "--target", "unknown", "source.hir", "-o", "unused"],
        vec!["build", "source.hir", "extra.hir", "-o", "unused"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
    let source = file("pub fn main() -> Result[(), ()] { let n: u64 = true; Ok(()) }");
    let exe = source.with_extension("exe");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!exe.exists());
    std::fs::write(
        &source,
        "pub fn main() -> Result[(), ()] { let n: u64 = 1u64; Ok(()) }",
    )
    .unwrap();
    let accepted = build_executable(&source, &[]);
    let output = Command::new(&accepted).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    std::fs::remove_file(accepted).unwrap();
    std::fs::remove_file(source).unwrap();
}

#[test]
fn grants_templates_launch_current_source_without_export_and_refuse_bad_inputs() {
    let source = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"template\\n\"); Ok(()) }\n",
    );
    let grants = file(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#);
    for native in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&source).arg("--grants").arg(&grants);
        if native {
            command.arg("--native");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, b"template\n");
    }
    for bad in [
        r#"{}"#,
        r#"{"program":"stale"}"#,
        r#"{"args":[],"args":[]}"#,
    ] {
        std::fs::write(&grants, bad).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{bad}: {output:?}");
        assert!(output.stdout.is_empty());
    }
    for flags in [
        ["--grants", "--run"],
        ["--run", "--grants"],
        ["--simulation", "--grants"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg(flags[0])
            .arg(&grants)
            .arg(flags[1])
            .arg(&grants)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
}

#[test]
fn standalone_grants_bind_embedded_code_without_an_exported_record() {
    let source = file(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/PROG/PROG-3-grants-template-entry-run.hir"),
        )
        .unwrap(),
    );
    let executable = source.with_extension("exe");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let grants = file(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#);
    let output = Command::new(&executable)
        .arg("--hirsel-grants")
        .arg(&grants)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"bound\n");
    for bad in [r#"{}"#, r#"{"entry":"standalone@0/other"}"#] {
        std::fs::write(&grants, bad).unwrap();
        let output = Command::new(&executable)
            .arg("--hirsel-grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn simulation_host_inputs_accept_binary_stdin_and_unset_env_and_route_stderr() {
    let source = file(include_str!(
        "../../../conformance/CAP/CAP-1-host-inputs-run.hir"
    ));
    let simulation = source.with_extension("simulation.json");
    std::fs::write(
        &simulation,
        serde_json::to_vec(&serde_json::json!({
            "stdin":[206,187,10,0,255],
            "args":["","λ","--flag"],
            "env":{"SET":"value","UNSET":null},
            "projections":["stdin","stderr"]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg("--simulation")
        .arg(&simulation)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "λ--flag".as_bytes());
    std::fs::write(
        &simulation,
        br#"{"stdin":"unread","projections":["stderr"]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg("--simulation")
        .arg(&simulation)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    std::fs::remove_file(simulation).unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(format!("{}.json", source.display())).unwrap();
}

#[test]
fn host_io_records_use_piped_stdin_distinct_stderr_and_exact_environment_snapshots() {
    use std::io::Write;
    use std::process::Stdio;
    let source = file(include_str!(
        "../../../conformance/CAP/CAP-1-host-inputs-run.hir"
    ));
    for native in [false, true] {
        for stdin_granted in [false, true] {
            let mut grants = serde_json::json!({
                "io": { "stderr":"stderr.bytes@1", "dirs":[] },
                "args":["","λ","--flag"],
                "env": { "SET":"value", "UNSET":null }
            });
            if stdin_granted {
                grants["io"]["stdin"] = "stdin.bytes@1".into();
            }
            let record = run_record(&source, grants);
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command
                .arg("run")
                .arg("--run")
                .arg(&record)
                .env("SECRET", "must remain unreadable")
                .env("SET", "ambient value is not the admitted snapshot")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if native {
                command.arg("--native");
            }
            command.arg(&source);
            let mut child = command.spawn().unwrap();
            if stdin_granted {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&[0xce, 0xbb, 10, 0, 255])
                    .unwrap();
            } else {
                drop(child.stdin.take());
            }
            let output = child.wait_with_output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(if stdin_granted { 0 } else { 1 }),
                "native={native}, stdin={stdin_granted}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert_eq!(
                output.stderr,
                if stdin_granted {
                    "λ--flag".as_bytes()
                } else {
                    b""
                }
            );
            std::fs::remove_file(record).unwrap();
        }
    }
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(format!("{}.json", source.display())).unwrap();
}

#[test]
fn file_stream_cli_reads_multiple_buffers_and_appends_under_explicit_host_grants() {
    hirsel_engine::with_compiler_stack(|| {
        let source = file(
            r#"
pub type Error = Files(FileError) | Grant(GrantError) | Wrong()
pub fn main(rt: Io) -> Result[(), Error] {
  let d = io.dir(rt, path.of("data")) ? Files
  let mut f = file.open(d, path.of("large.bin")) ? Files
  let mut total = 0u64
  let mut calls = 0u64
  while true {
    let mut buffer: List[u8] = []
    let count = file.read(d, mut f, mut buffer, 4096u64) ? Files
    if count == 0u64 { break }
    if list.len(buffer) != count { return Error(Wrong()) }
    if buffer[0u64] != 65u8 || buffer[count - 1u64] != 65u8 { return Error(Wrong()) }
    total = total + count
    calls = calls + 1u64
  }
  if total != 150003u64 || calls < 2u64 { return Error(Wrong()) }
  file.seek(d, mut f, 150000u64) ? Files
  let mut tail: List[u8] = []
  if file.read(d, mut f, mut tail, 20u64) ? Files != 3u64 { return Error(Wrong()) }
  let mut writer = file.create(d, path.of("copy.txt")) ? Files
  if file.write(d, mut writer, b"first") ? Files != 5u64 { return Error(Wrong()) }
  file.seek(d, mut writer, 0u64) ? Files
  file.append(d, mut writer, b" second") ? Files
  let _ = out.print(io.stdout(rt) ? Grant, "stream ok\n")
  Ok(())
}
"#,
        );
        let root = source.with_extension("files");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("large.bin"), vec![b'A'; 150003]).unwrap();
        let mut record = build_run_record(&source);
        record["io"] = stdout_grant();
        let run = source.with_extension("run.json");
        let exe = build_executable(&source, &[]);
        // The same source exercises the real host interpreter and standalone LLVM binary.
        for native in [false, true] {
            record["io"]["dirs"] =
                serde_json::json!([{"path":"data","host":root,"access":"read_write"}]);
            std::fs::write(&run, record.to_string()).unwrap();
            let mut command = if native {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                c
            };
            command
                .arg(if native { "--hirsel-run" } else { "--run" })
                .arg(&run);
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(0), "native={native}: {output:?}");
            assert_eq!(output.stdout, b"stream ok\n");
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(
                std::fs::read(root.join("copy.txt")).unwrap(),
                b"first second"
            );
            // A read grant permits all incremental reads but denies creation before truncation.
            record["io"]["dirs"][0]["access"] = "read".into();
            std::fs::write(&run, record.to_string()).unwrap();
            let denied = command.output().unwrap();
            assert_eq!(denied.status.code(), Some(1), "{denied:?}");
            assert!(denied.stdout.is_empty());
            assert_eq!(
                std::fs::read(root.join("copy.txt")).unwrap(),
                b"first second"
            );
            record["io"]["dirs"] = serde_json::json!([]);
            std::fs::write(&run, record.to_string()).unwrap();
            let denied = command.output().unwrap();
            assert_eq!(denied.status.code(), Some(1), "{denied:?}");
            assert!(denied.stdout.is_empty());
        }
        for path in [source, exe, run] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn file_errors_cli_reports_permission_denied_for_a_chmod_zero_host_file() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    hirsel_engine::with_compiler_stack(|| {
        let source = file(
            r#"pub fn main(rt: Io) -> Result[(), FileError] {
  let d = io.dir(rt, path.of("data"))?
  match file.open(d, path.of("secret")) { Error(PermissionDenied()) => Ok(()), _ => Error(FileFault(99u32)) }
}
"#,
        );
        if std::fs::metadata(&source).unwrap().uid() == 0 {
            eprintln!("skipped chmod permission test: running as root");
            std::fs::remove_file(source).unwrap();
            return;
        }
        let root = source.with_extension("files");
        std::fs::create_dir(&root).unwrap();
        let secret = root.join("secret");
        std::fs::write(&secret, b"denied").unwrap();
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut record = build_run_record(&source);
        record["io"] = stdout_grant();
        record["io"]["dirs"] = serde_json::json!([{"path":"data","host":root,"access":"read"}]);
        let run = source.with_extension("run.json");
        std::fs::write(&run, record.to_string()).unwrap();
        let exe = build_executable(&source, &[]);
        for native in [false, true] {
            let mut command = if native {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                c
            };
            let output = command
                .arg(if native { "--hirsel-run" } else { "--run" })
                .arg(&run)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "native={native}: {output:?}");
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty(), "{output:?}");
        }
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
        for path in [source, exe, run] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn real_host_child_await() {
    let source = file(include_str!("../../../conformance/CONC/CONC-2-await.hir"));
    let record = run_record(
        &source,
        serde_json::json!({
            "io": {"tasks": "tasks.cooperative@1", "dirs": []}, "args": []
        }),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let native = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .arg("--native")
        .output()
        .unwrap();
    assert!(!native.status.success());
    assert!(
        String::from_utf8_lossy(&native.stderr).contains("io.tasks"),
        "{native:?}"
    );
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_tasks_accept_and_connect_as_siblings() {
    let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let source = file(&format!(
        r#"
fn grant(e: GrantError) -> () uses {{}} {{ () }}
fn network(e: NetError) -> () uses {{}} {{ () }}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  let n = io.net(rt) ? grant
  let tasks = io.tasks(rt) ? grant
  let address = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: {port}u16)
  let listener = net.listen(n, address) ? network
  task.scope(tasks, fn(s: Scope) -> Result[(), ()] {{
    let server = task.async(s, fn() -> Result[(), ()] {{
      let mut owned_listener = take listener
      let mut (stream, _) = net.accept(n, mut owned_listener) ? network
      let mut bytes: List[u8] = []
      let count = net.read(n, mut stream, mut bytes, 1) ? network
      failure.check(count == 1 && bytes[0] == 120u8, Arithmetic())
      net.write_all(n, mut stream, b"y") ? network
      net.close(n, take stream)
      Ok(())
    }})
    let client = task.async(s, fn() -> Result[(), ()] {{
      let mut stream = net.connect_addr(n, address, Unbounded()) ? network
      net.write_all(n, mut stream, b"x") ? network
      let mut bytes: List[u8] = []
      let count = net.read(n, mut stream, mut bytes, 1) ? network
      failure.check(count == 1 && bytes[0] == 121u8, Arithmetic())
      net.close(n, take stream)
      Ok(())
    }})
    task.await(s, take server)?
    task.await(s, take client)?
    Ok(())
  }})
}}
"#
    ));
    let record = run_record(
        &source,
        serde_json::json!({
            "io": {"tasks": "tasks.cooperative@1", "dirs": [], "net": {
                "provider": "net.stream@1", "rules": [
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "listen"},
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "connect"}
                ]
            }}, "args": []
        }),
    );
    drop(reservation);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_cancel_parked_accept_is_level_triggered() {
    let source = file(
        r#"
fn grant(e: GrantError) -> () uses {} { () }
fn network(e: NetError) -> () uses {} { () }
fn canceled(e: NetError) -> () uses {} {
  failure.check(match e { Canceled(_) => true, _ => false }, Arithmetic())
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let n = io.net(rt) ? grant
  let tasks = io.tasks(rt) ? grant
  let listener = net.listen(n, SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)) ? network
  let (sender, receiver): (Sender[u8], Receiver[u8]) = channel.open(tasks)
  task.scope(tasks, fn(s: Scope) -> Result[(), ()] {
    let mut rx = take receiver
    let child = task.async(s, fn() -> Result[(), ()] {
      let mut tx = take sender
      let mut owned_listener = take listener
      let _ = channel.send(tasks, mut tx, 1u8)
      let first = net.accept(n, mut owned_listener)
      match first { Error(e) => canceled(e), Ok(pair) => failure.check(false, Arithmetic()) }
      let second = net.accept(n, mut owned_listener)
      match second { Error(e) => canceled(e), Ok(pair) => failure.check(false, Arithmetic()) }
      Ok(())
    })
    let _ = channel.receive(tasks, mut rx)
    task.cancel(s, child)
    task.await(s, take child)
  })
}
"#,
    );
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks": "tasks.cooperative@1", "dirs": [], "net": {"provider": "net.stream@1", "rules": [
            {"address": "127.0.0.1", "service": "0", "operation": "listen"}
        ]}}, "args": []}),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_canceled_stdin_and_stderr_have_no_effects() {
    use std::io::Write;
    use std::process::Stdio;
    let source = file(
        &include_str!(
            "../../../conformance/CONC/CONC-3-stdin-stderr-canceled-before-effect-run.hir"
        )
        .replace(
            "Ok(_) => true,",
            "Ok(count) => count == 1 && buffer[0] == 120u8,",
        ),
    );
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks":"tasks.cooperative@1", "stdin":"stdin.bytes@1", "stderr":"stderr.bytes@1", "dirs":[]
    }, "args":[]}),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_detached_results_drop_before_scope_exit() {
    let source = file(include_str!(
        "../../../conformance/CONC/CONC-2-detached-result-cleanup-run.hir"
    ));
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks":"tasks.cooperative@1", "dirs":[]
    }, "args":[]}),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn source_sidecars_and_explicit_manifests_select_the_same_dependency_graph() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (case, check, run, rule) in [
        ("FACT/FACT-2-external-constructor-ok", 0, Some(0), None),
        (
            "FACT/FACT-2-external-constructor-bad",
            0,
            Some(1),
            Some("FACT-2"),
        ),
        (
            "LANG/LANG-4-external-full-path-bad",
            1,
            None,
            Some("LANG-4"),
        ),
    ] {
        let source = root.join(format!("conformance/{case}.hir"));
        let manifest = source.with_extension("hir.json");
        for path in [&source, &manifest] {
            let checked = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("check")
                .arg(path)
                .output()
                .unwrap();
            assert_eq!(checked.status.code(), Some(check), "{path:?}: {checked:?}");
            assert!(checked.stdout.is_empty(), "{path:?}: {checked:?}");
            if check != 0 {
                assert!(String::from_utf8_lossy(&checked.stderr).contains(rule.unwrap()));
            }
            if let Some(exit) = run {
                let ran = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .arg(path)
                    .output()
                    .unwrap();
                assert_eq!(ran.status.code(), Some(exit), "{path:?}: {ran:?}");
                assert!(ran.stdout.is_empty(), "{path:?}: {ran:?}");
                if let Some(rule) = rule {
                    assert!(String::from_utf8_lossy(&ran.stderr).contains(rule));
                }
            }
        }
    }
}

#[test]
fn real_host_stdin_read_parks_only_its_task() {
    use std::io::{BufRead, Write};
    use std::os::fd::AsRawFd;
    use std::process::Stdio;
    let source = file(
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let input = io.stdin(rt)?
  let output = io.stdout(rt)?
  task.scope(
    tasks,
    fn(s: Scope) {
      let child = task.async(
        s,
        fn() -> bool {
          let mut bytes: List[u8] = []
          match stdin.read(input, mut bytes, 1) {
            Ok(count) => count == 1 && bytes[0] == 120u8,
            _ => false,
          }
        }
      )
      let _ = out.print(output, "sibling\n")
      let read = match task.await(s, take child) {
        true => "read x\n",
        false => "read failed\n",
      }
      let _ = out.print(output, read)
    }
  )
  Ok(())
}
"#,
    );
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks":"tasks.cooperative@1", "stdin":"stdin.bytes@1", "stdout":"stdout.bytes@1", "dirs":[]
    }, "args":[]}),
    );
    // O_NONBLOCK of the open file description the child shares with this process.
    let nonblocking = |fd: i32| {
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}")).unwrap();
        let flags = info.lines().find_map(|l| l.strip_prefix("flags:")).unwrap();
        u32::from_str_radix(flags.trim(), 8).unwrap() & 0o4000 != 0
    };
    let spawn = |stdin: Stdio| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--run")
            .arg(&record)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines() {
                let _ = send.send(line.unwrap());
            }
        });
        (child, lines)
    };
    let finish = |child: std::process::Child, lines: std::sync::mpsc::Receiver<String>| {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(lines.iter().collect::<Vec<_>>(), ["read x"]);
    };

    // The pipe stays open and empty until the sibling has printed.
    let (reader, mut writer) = std::io::pipe().unwrap();
    let observer = reader.try_clone().unwrap();
    let (mut child, lines) = spawn(reader.into());
    match lines.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(line) => assert_eq!(line, "sibling"),
        Err(e) => {
            let _ = child.kill();
            panic!("the parked stdin read blocked its sibling: {e}");
        }
    }
    assert!(nonblocking(observer.as_raw_fd()));
    writer.write_all(b"x").unwrap();
    drop(writer);
    finish(child, lines);
    assert!(!nonblocking(observer.as_raw_fd()), "blocking mode restored");

    // A regular file cannot block: it is read directly and stays blocking.
    let input = source.with_extension("stdin");
    std::fs::write(&input, b"x").unwrap();
    let (child, lines) = spawn(std::fs::File::open(&input).unwrap().into());
    assert_eq!(lines.recv().unwrap(), "sibling");
    finish(child, lines);
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_parked_stdin_read_observes_cancellation() {
    use std::io::BufRead;
    use std::process::Stdio;
    // The child parks on the empty pipe at the sibling's print; cancel must resume
    // it as Canceled without input ever arriving.
    let source = file(
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let input = io.stdin(rt)?
  let output = io.stdout(rt)?
  task.scope(
    tasks,
    fn(s: Scope) {
      let child = task.async(
        s,
        fn() -> bool {
          let mut bytes: List[u8] = []
          match stdin.read(input, mut bytes, 1) {
            Error(Canceled(Requested)) => true,
            _ => false,
          }
        }
      )
      let _ = out.print(output, "sibling\n")
      task.cancel(s, child)
      let read = match task.await(s, take child) {
        true => "canceled\n",
        false => "not canceled\n",
      }
      let _ = out.print(output, read)
    }
  )
  Ok(())
}
"#,
    );
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks":"tasks.cooperative@1", "stdin":"stdin.bytes@1", "stdout":"stdout.bytes@1", "dirs":[]
    }, "args":[]}),
    );
    let (reader, writer) = std::io::pipe().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&record)
        .stdin(reader)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines() {
            let _ = send.send(line.unwrap());
        }
    });
    for expected in ["sibling", "canceled"] {
        match lines.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(line) => assert_eq!(line, expected),
            Err(e) => {
                let _ = child.kill();
                panic!("canceled stdin read stayed parked: {e}");
            }
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(writer);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}
