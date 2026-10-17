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
            b"usage: hirsel check [--json] <source|manifest.json> | run [--native | --simulation <json>] <source|manifest.json> [--run <json>] | build [--release | --debug] [--target x86_64-linux | aarch64-linux] <source|manifest.json> -o <exe> | fmt [--check] <file> | export --tir|--run <source|manifest.json> -o <file>\n",
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
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let s: Slots[u8] = slots.empty(); Ok(()) }",
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
fn hello_web_answers_real_requests_interpreted() {
    hello_web(false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_answers_real_requests_native() {
    hello_web(true);
}

fn hello_web(native: bool) {
    use std::io::{BufRead, Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Duration;
    {
        let source = include_str!("../../../examples/hello-web/main.hir")
            .replace("8080u16", "0u16")
            .replace(
                "while true",
                r#"
let bound = net.local_addr(n, listener) ? network_error
let output = io.stdout(rt) ? grant_error
let _ = out.print(output, std@1/u16.to_string(bound.port))
let _ = out.print(output, "\n")
for i in 0u64..3u64
"#,
            );
        let path = file(&source);
        let manifest = path.with_extension("hir.json");
        let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(&manifest,serde_json::json!({"schema":1,"package":{"name":"hello_web","version":"1"},"sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let record = run_record(
            &path,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"0","operation":"listen"}]}},"args":[]}),
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
