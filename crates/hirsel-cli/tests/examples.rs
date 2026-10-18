//! End-to-end examples: exercise CLI manifests, std selection, actual output,
//! and native linking rather than duplicating language conformance tests.
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, source: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-example-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.hir"), source).unwrap();
        if name == "diagnostic" {
            let manifest = serde_json::json!({"schema":1,"package":{"name":name,"version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]});
            std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
            return Self(dir);
        }
        std::fs::write(
            dir.join("common.hir"),
            include_str!("../../../examples/common.hir"),
        )
        .unwrap();
        let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let mut manifest: serde_json::Value = match name {
            "wordscan" => {
                serde_json::from_str(include_str!("../../../examples/wordscan/hirsel.json"))
                    .unwrap()
            }
            "csvjson" => {
                serde_json::from_str(include_str!("../../../examples/csvjson/hirsel.json")).unwrap()
            }
            "jsonquery" => {
                serde_json::from_str(include_str!("../../../examples/jsonquery/hirsel.json"))
                    .unwrap()
            }
            "logsummary" => {
                serde_json::from_str(include_str!("../../../examples/logsummary/hirsel.json"))
                    .unwrap()
            }
            "calculator" => {
                serde_json::from_str(include_str!("../../../examples/calculator/hirsel.json"))
                    .unwrap()
            }
            "markdown" => {
                serde_json::from_str(include_str!("../../../examples/markdown/hirsel.json"))
                    .unwrap()
            }
            "adventure" => {
                serde_json::from_str(include_str!("../../../examples/adventure/hirsel.json"))
                    .unwrap()
            }
            "sortuniq" => {
                serde_json::from_str(include_str!("../../../examples/sortuniq/hirsel.json"))
                    .unwrap()
            }
            "todo" => {
                serde_json::from_str(include_str!("../../../examples/todo/hirsel.json")).unwrap()
            }
            "httpget" => {
                serde_json::from_str(include_str!("../../../examples/httpget/hirsel.json")).unwrap()
            }
            _ => {
                serde_json::json!({"schema":1,"package":{"name":name,"version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"common.hir","module":["common"]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]})
            }
        };
        manifest["sources"][1]["path"] = serde_json::json!("common.hir");
        manifest["dependencies"][0]["manifest"] = serde_json::json!(stdlib);
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        Self(dir)
    }
    fn run(&self, native: bool, simulation: Option<serde_json::Value>, expected: &str, code: i32) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        if let Some(config) = simulation {
            let path = self.0.join("run.json");
            std::fs::write(&path, config.to_string()).unwrap();
            command.arg("--simulation").arg(path);
        }
        let output = command.arg(self.0.join("hirsel.json")).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // Exercise explicit Args/root/Net through the native fixture boundary.
    fn native_executable(&self) -> PathBuf {
        hirsel_engine::with_compiler_stack(|| {
            use hirsel_driver::native::{
                OptLevel, Options, Partition, TargetSpec, Toolchain, build, link,
            };
            let mut loaded = hirsel_driver::load_program(&self.0.join("hirsel.json")).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let program = build(&validated, &Options::new(target.clone())).unwrap();
            let work = self.0.join("native");
            std::fs::create_dir_all(work.join("root")).unwrap();
            let toolchain = Toolchain::in_tree(&self.0.join("runtime"), &[&target]).unwrap();
            link(
                &program,
                &loaded.entry,
                OptLevel::Speed,
                Partition::Whole,
                &toolchain,
                &work,
            )
            .unwrap()
        })
    }
    fn native_files(&self, args: &[&str], files: &[(&str, &str)], expected: &str) {
        let executable = self.native_executable();
        let root = self.0.join("native/root");
        for (path, data) in files {
            std::fs::write(root.join(path), data).unwrap();
        }
        let output = Command::new(executable)
            .args(args)
            .env_clear()
            .env("HIRSEL_RT_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(output.stderr.is_empty());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
macro_rules! example {
    ($name:ident, $source:expr, $config:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let fixture = Fixture::new(stringify!($name), $source);
            fixture.run(
                false,
                Some(serde_json::from_str($config).unwrap()),
                $expected,
                0,
            );
            fixture.run(true, None, $expected, 0);
        }
    };
}
example!(
    wordscan,
    include_str!("../../../examples/wordscan/main.hir"),
    include_str!("../../../examples/wordscan/demo.json"),
    "lines=3 words=6 bytes=29\nred sheep\nred fox\n"
);
example!(
    csvjson,
    include_str!("../../../examples/csvjson/main.hir"),
    include_str!("../../../examples/csvjson/demo.json"),
    "[{\"name\":\"Ada\",\"note\":\"hello, sheep\"},{\"name\":\"Lin\",\"note\":\"a \\\"quote\\\"\"}]\n"
);
example!(
    jsonquery,
    include_str!("../../../examples/jsonquery/main.hir"),
    include_str!("../../../examples/jsonquery/demo.json"),
    "[\"ewe\",\"lamb\"]\n"
);
example!(
    logsummary,
    include_str!("../../../examples/logsummary/main.hir"),
    include_str!("../../../examples/logsummary/demo.json"),
    "INFO 2\nWARN 2\nERROR 1\n"
);
example!(
    calculator,
    include_str!("../../../examples/calculator/main.hir"),
    include_str!("../../../examples/calculator/demo.json"),
    "38\n-4\n99\n"
);
example!(
    markdown,
    include_str!("../../../examples/markdown/main.hir"),
    include_str!("../../../examples/markdown/demo.json"),
    "<h1>Flock</h1>\n<p>Sheep &amp; &lt;grass&gt;</p>\n<ul>\n<li>ewe</li>\n<li>lamb</li>\n</ul>\n"
);
example!(
    adventure,
    include_str!("../../../examples/adventure/main.hir"),
    include_str!("../../../examples/adventure/demo.json"),
    "Meadow: north is a cave, east a gate.\nA dark cave.\nKey collected.\nBack in the meadow.\nHome. You win!\n"
);
example!(
    sortuniq,
    include_str!("../../../examples/sortuniq/main.hir"),
    include_str!("../../../examples/sortuniq/demo.json"),
    "apple\nbanana\npear\n"
);
example!(
    todo,
    include_str!("../../../examples/todo/main.hir"),
    include_str!("../../../examples/todo/demo.json"),
    "1. mend gate\n"
);

fn input(text: &str, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["input.txt"];
    args.extend_from_slice(extra);
    serde_json::json!({"grants":["Io","Args"],"projections":["stdout","dir"],"args":args,"files":{"input.txt":text}})
}
#[test]
fn sortuniq_large_input() {
    let fixture = Fixture::new(
        "sortuniq",
        include_str!("../../../examples/sortuniq/main.hir"),
    );
    let data = "zebra\napple\nλamb\napple\n".repeat(1024);
    fixture.run(false, Some(input(&data, &[])), "\napple\nzebra\nλamb\n", 0);
    fixture.native_files(
        &["input.txt"],
        &[("input.txt", &data)],
        "\napple\nzebra\nλamb\n",
    );
}
#[test]
fn csvjson_quoted_crlf_and_empty_fields() {
    let fixture = Fixture::new(
        "csvjson",
        include_str!("../../../examples/csvjson/main.hir"),
    );
    fixture.run(
        false,
        Some(input("a,b\r\n\"x\r\ny\",\r\n", &[])),
        "[{\"a\":\"x\\r\\ny\",\"b\":\"\"}]\n",
        0,
    );
}
#[test]
fn calculator_recovers_after_bad_expression() {
    let fixture = Fixture::new(
        "calculator",
        include_str!("../../../examples/calculator/main.hir"),
    );
    fixture.run(
        false,
        Some(input("1 / 0\n3 ? 2\n5 + 2\n", &[])),
        "error\nerror\n7\n",
        0,
    );
}
#[test]
fn todo_reads_existing_state_before_atomic_save() {
    let fixture = Fixture::new("todo", include_str!("../../../examples/todo/main.hir"));
    let mut config = input("done 1\nadd check fence\nlist\n", &["state.txt"]);
    config["files"]["state.txt"] = serde_json::json!("feed sheep\nmend gate");
    fixture.run(false, Some(config), "1. mend gate\n2. check fence\n", 0);
}

#[test]
fn httpget() {
    use std::io::{Read, Write};
    let fixture = Fixture::new(
        "httpget",
        include_str!("../../../examples/httpget/main.hir"),
    );
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/httpget/demo.json")).unwrap();
    let mut simulation = hirsel_engine::SimulationConfig::default();
    simulation.args = vec![config["args"][0].as_str().unwrap().to_owned()];
    simulation.net = Some(serde_json::from_value(config["net"].clone()).unwrap());
    let mut output = Vec::new();
    let result = hirsel_engine::with_compiler_stack(|| {
        hirsel_driver::run_path_with_simulation(
            &fixture.0.join("hirsel.json"),
            simulation,
            &mut output,
            &mut Vec::new(),
        )
    })
    .unwrap();
    assert_eq!(result, hirsel_engine::RunOutcome::Exit(0));
    assert_eq!(output, b"sheep\n");
    let executable = fixture.native_executable();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Nonblocking accept keeps a failed client from leaving a stuck server.
    listener.set_nonblocking(true).unwrap();
    let child = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(e) => panic!("HTTP client did not connect: {e}"),
            }
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        assert!(request.starts_with(b"GET /flock HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nsheep\n")
            .unwrap();
    });
    let grant = serde_json::json!({"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]});
    let output = Command::new(executable)
        .arg(format!("http://127.0.0.1:{port}/flock"))
        .env_clear()
        .env("HIRSEL_RT_ROOT", fixture.0.join("native/root"))
        .env("HIRSEL_RT_NET", grant.to_string())
        .output()
        .unwrap();
    child.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"sheep\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn todo_persists_between_native_launches() {
    let fixture = Fixture::new("todo", include_str!("../../../examples/todo/main.hir"));
    let executable = fixture.native_executable();
    let root = fixture.0.join("native/root");
    for (commands, expected) in [
        ("add feed sheep\n", "1. feed sheep\n"),
        ("add mend gate\nlist\n", "1. feed sheep\n2. mend gate\n"),
    ] {
        std::fs::write(root.join("commands.txt"), commands).unwrap();
        let output = Command::new(&executable)
            .args(["commands.txt", "state.txt"])
            .env_clear()
            .env("HIRSEL_RT_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        std::fs::read_to_string(root.join("state.txt")).unwrap(),
        "feed sheep\nmend gate"
    );
}

#[test]
fn wordscan_utf8_and_ascii_controls() {
    let fixture = Fixture::new(
        "wordscan",
        include_str!("../../../examples/wordscan/main.hir"),
    );
    let data = "é\u{c}blue\nred";
    let expected = "lines=1 words=3 bytes=11\nred\n";
    fixture.run(false, Some(input(data, &[])), expected, 0);
    fixture.native_files(&["input.txt"], &[("input.txt", data)], expected);
    // Exercise the documented production launch as well as the fixture path.
    std::fs::write(fixture.0.join("input.txt"), data).unwrap();
    let grants = fixture.0.join("grants.json");
    std::fs::write(
        &grants,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"","host":fixture.0,"access":"read"}]},"args":["input.txt"]}).to_string(),
    ).unwrap();
    for native in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        let output = command
            .arg(fixture.0.join("hirsel.json"))
            .arg("--grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn logsummary_splits_ascii_whitespace() {
    let fixture = Fixture::new(
        "logsummary",
        include_str!("../../../examples/logsummary/main.hir"),
    );
    let data = "\tinfo boot\n \nwarn\u{c}heat\nINFO ready\n";
    let expected = "INFO 2\nWARN 1\n";
    fixture.run(false, Some(input(data, &[])), expected, 0);
    fixture.native_files(&["input.txt"], &[("input.txt", data)], expected);
}

#[test]
fn csvjson_rejects_malformed_input() {
    let fixture = Fixture::new(
        "csvjson",
        include_str!("../../../examples/csvjson/main.hir"),
    );
    for data in ["a,a\n", "a,b\nx\n", "a\n\"unterminated", "a\n\"x\"bad\n"] {
        fixture.run(false, Some(input(data, &[])), "", 1);
    }
}

#[test]
fn cli_interpreter_step_limits() {
    let fixture = Fixture::new(
        "diagnostic",
        "pub fn main() -> Result[(), ()] { let mut i: u64 = 0; while i < 20 { i += 1 }; Ok(()) }",
    );
    let manifest = fixture.0.join("hirsel.json");
    let invoke = |flags: &[&str], code: i32, message: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&manifest)
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8_lossy(&output.stderr), message);
    };
    invoke(&[], 0, "");
    invoke(&["--max-steps", "1"], 2, "evaluator tooling limit: Steps\n");
    invoke(&["--max-steps=10000"], 0, "");
    invoke(&["--max-steps", "unbounded"], 0, "");
    invoke(&["--max-steps=0"], 2, "evaluator tooling limit: Steps\n");
    let simulation = fixture.0.join("simulation.json");
    std::fs::write(&simulation, "{}").unwrap();
    invoke(
        &[
            "--simulation",
            simulation.to_str().unwrap(),
            "--max-steps",
            "1",
        ],
        2,
        "evaluator tooling limit: Steps\n",
    );
    for values in [
        vec!["bad"],
        vec!["-1"],
        vec!["18446744073709551616"],
        vec![],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&manifest)
            .arg("--max-steps")
            .args(values)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with("--max-steps requires an unsigned integer or unbounded\n")
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native", "--max-steps", "1"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .starts_with("--max-steps is only supported for interpreted run\n")
    );
    // An explicit host record must use the same bound as simulated execution.
    let record = fixture.0.join("host.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--run"])
        .arg(&manifest)
        .arg("-o")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    invoke(
        &["--run", record.to_str().unwrap(), "--max-steps=1"],
        2,
        "evaluator tooling limit: Steps\n",
    );
    invoke(&["--run", record.to_str().unwrap()], 0, "");
    let grants = fixture.0.join("grants.json");
    std::fs::write(&grants, "{}").unwrap();
    invoke(
        &["--grants", grants.to_str().unwrap(), "--max-steps=1"],
        2,
        "evaluator tooling limit: Steps\n",
    );
    invoke(&["--grants", grants.to_str().unwrap()], 0, "");
    invoke(
        &[
            "--grants",
            grants.to_str().unwrap(),
            "--max-steps=unbounded",
        ],
        0,
        "",
    );
    invoke(
        &[
            "--simulation",
            simulation.to_str().unwrap(),
            "--max-steps=unbounded",
        ],
        0,
        "",
    );
}

#[test]
fn cli_default_budget_exceeds_the_conformance_budget() {
    let fixture = Fixture::new(
        "diagnostic",
        "pub fn main() -> Result[(), ()] { let mut i: u64 = 0; while i < 100000 { i += 1 }; Ok(()) }",
    );
    let manifest = fixture.0.join("hirsel.json");
    let bounded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&manifest)
        .args(["--max-steps", "1000000"])
        .output()
        .unwrap();
    assert_eq!(bounded.status.code(), Some(2));
    assert_eq!(bounded.stderr, b"evaluator tooling limit: Steps\n");
    let ordinary = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        ordinary.status.success(),
        "{}",
        String::from_utf8_lossy(&ordinary.stderr)
    );
    assert!(ordinary.stdout.is_empty());
    assert!(ordinary.stderr.is_empty());
}

#[test]
fn calculator_stdin_session() {
    use std::io::Write;
    use std::process::Stdio;
    let fixture = Fixture::new(
        "calculator",
        include_str!("../../../examples/calculator/main.hir"),
    );
    let text = "1 + 2\n1 / 0\n\n7 - 3";
    let expected = "3\nerror\n4\n";
    fixture.run(false, Some(serde_json::json!({"grants":["Io","Args"],"projections":["stdout","stdin"],"args":["-"],"stdin":text})), expected, 0);
    fixture.run(false, Some(serde_json::json!({"grants":["Io","Args"],"projections":["stdout","stdin"],"args":["-"],"stdin":[49,32,43,32,50,10,255]})), "3\n", 1);
    let manifest = fixture.0.join("hirsel.json");
    let record = fixture.0.join("host-stdin.json");
    let exported = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--run"])
        .arg(&manifest)
        .arg("-o")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    let template: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/calculator/stdin-grants.json"
    ))
    .unwrap();
    config["io"] = template["io"].clone();
    config["args"] = template["args"].clone();
    std::fs::write(&record, config.to_string()).unwrap();
    for native in [false, true] {
        for (bytes, expected, code) in [
            (text.as_bytes(), expected, 0),
            (&b"1 + 2\n\xff"[..], "3\n", 1),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run");
            if native {
                command.arg("--native");
            }
            let mut child = command
                .arg(&manifest)
                .arg("--run")
                .arg(&record)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(bytes).unwrap();
            let output = child.wait_with_output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(code),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, expected.as_bytes());
            assert!(output.stderr.is_empty());
        }
    }
}

/// The todo API server, killed on drop; `port` is the one it printed.
struct TodoServer {
    child: std::process::Child,
    port: u16,
}

impl TodoServer {
    fn start(manifest: &std::path::Path, grants: &std::path::Path) -> Self {
        use std::io::BufRead;
        let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(manifest)
            .arg("--grants")
            .arg(grants)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            let _ = send.send(line);
        });
        let line = recv
            .recv_timeout(std::time::Duration::from_secs(90))
            .expect("server readiness");
        let port = line
            .strip_prefix("listening on 127.0.0.1:")
            .unwrap_or_else(|| panic!("readiness line {line:?}"))
            .trim()
            .parse()
            .unwrap();
        Self { child, port }
    }

    /// Send one raw request and return the status code, the head and the body.
    fn send(&self, request: &[u8]) -> (u16, String, String) {
        use std::io::{Read, Write};
        let mut stream =
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(60)))
            .unwrap();
        stream.write_all(request).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head[9..12].parse().unwrap();
        (status, head.to_owned(), body.to_owned())
    }

    fn call(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let request = match body {
            Some(body) => format!(
                "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
            None => format!("{method} {path} HTTP/1.1\r\nHost: x\r\n\r\n"),
        };
        let (status, head, body) = self.send(request.as_bytes());
        if body.is_empty() {
            assert!(!head.contains("Content-Type"), "{head}");
        } else {
            assert!(
                head.contains("\r\nContent-Type: application/json\r\n"),
                "{head}"
            );
        }
        (status, body)
    }
}

impl Drop for TodoServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Copy the example into a fresh directory with the shipped grants, granting its data
/// directory and an ephemeral port; returns the directory, manifest and grants paths.
fn todo_api_fixture() -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-example-{}-{}-todo-api",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        dir.join("main.hir"),
        include_str!("../../../examples/todo-api/main.hir"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/todo-api/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = serde_json::json!(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
    );
    std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
    // The shipped grants, with this run's data directory and an ephemeral port.
    let mut grants: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/todo-api/grants.json")).unwrap();
    grants["io"]["dirs"][0]["host"] = serde_json::json!(data.canonicalize().unwrap());
    grants["io"]["net"]["rules"][0]["service"] = serde_json::json!("0");
    grants["args"] = serde_json::json!(["0"]);
    std::fs::write(dir.join("grants.json"), grants.to_string()).unwrap();
    (
        dir.clone(),
        dir.join("hirsel.json"),
        dir.join("grants.json"),
    )
}

/// Two clients create todos while an incomplete request holds its own task; returns the
/// list the held request finally received.
fn todo_api_concurrent_creates(server: &TodoServer, first_id: u64) -> String {
    use std::io::{Read, Write};
    let mut held =
        std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, server.port)).unwrap();
    held.set_read_timeout(Some(std::time::Duration::from_secs(60)))
        .unwrap();
    held.write_all(b"GET /todos HTTP/1.1\r\nHost:").unwrap();
    let created: Vec<String> = std::thread::scope(|scope| {
        let clients: Vec<_> = ["sheep", "goats"]
            .map(|title| {
                scope.spawn(move || {
                    server.call("POST", "/todos", Some(&format!(r#"{{"title":"{title}"}}"#)))
                })
            })
            .into_iter()
            .collect();
        clients
            .into_iter()
            .map(|client| {
                let (status, body) = client.join().unwrap();
                assert_eq!(status, 201);
                body
            })
            .collect()
    });
    let mut ids: Vec<u64> = created
        .iter()
        .map(|body| body[6..body.find(',').unwrap()].parse().unwrap())
        .collect();
    ids.sort();
    assert_eq!(ids, [first_id, first_id + 1], "{created:?}");
    held.write_all(b" x\r\n\r\n").unwrap();
    held.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    held.read_to_string(&mut response).unwrap();
    let listed = response.split_once("\r\n\r\n").unwrap().1.to_string();
    for body in &created {
        assert!(listed.contains(body.as_str()), "{listed}");
    }
    listed
}

#[test]
fn todo_api_serves_clients_concurrently_and_persists() {
    let (dir, manifest, grants) = todo_api_fixture();
    // A missing state file starts empty.
    let server = TodoServer::start(&manifest, &grants);
    assert_eq!(server.call("GET", "/todos", None), (200, "[]".into()));
    let listed = todo_api_concurrent_creates(&server, 1);
    drop(server);
    let server = TodoServer::start(&manifest, &grants);
    assert_eq!(server.call("GET", "/todos", None), (200, listed));
    drop(server);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "full gate"]
fn full_gate_todo_api_serves_every_route_and_persists() {
    let (dir, manifest, grants) = todo_api_fixture();
    let state = dir.join("data/todos.json");
    // A missing state file starts empty.
    let server = TodoServer::start(&manifest, &grants);
    assert_eq!(server.call("GET", "/todos", None), (200, "[]".into()));
    assert!(!state.exists());
    let created = r#"{"id":1,"title":"feed the \"sheep\"","done":false}"#;
    assert_eq!(
        server.call("POST", "/todos", Some(r#"{"title":"feed the \"sheep\""}"#)),
        (201, created.into())
    );
    assert_eq!(server.call("GET", "/todos/1", None), (200, created.into()));
    let done = r#"{"id":1,"title":"feed the \"sheep\"","done":true}"#;
    assert_eq!(
        server.call("PATCH", "/todos/1?verbose", Some(r#"{"done":true}"#)),
        (200, done.into())
    );
    assert_eq!(
        server.call("POST", "/todos", Some(r#"{"title":"mend gate"}"#)),
        (201, r#"{"id":2,"title":"mend gate","done":false}"#.into())
    );
    assert_eq!(
        server.call("DELETE", "/todos/2", None),
        (204, String::new())
    );
    assert_eq!(
        server.call("GET", "/todos", None),
        (200, format!("[{done}]"))
    );

    let not_found = (404, r#"{"error":"not found"}"#.to_string());
    for path in ["/todos/2", "/todos/x", "/todos/1/x", "/", "/nope"] {
        assert_eq!(server.call("GET", path, None), not_found, "{path}");
    }
    assert_eq!(server.call("DELETE", "/todos/2", None), not_found);
    assert_eq!(
        server.call("PATCH", "/todos/9", Some(r#"{"done":true}"#)),
        not_found
    );
    for (body, error) in [
        ("{", "body must be a JSON object"),
        ("[]", "body must be a JSON object"),
        (r#"{"title":1}"#, "title must be a string"),
        (r#"{"title":""}"#, "title must not be empty"),
    ] {
        assert_eq!(
            server.call("POST", "/todos", Some(body)),
            (400, format!(r#"{{"error":"{error}"}}"#)),
            "{body}"
        );
    }
    assert_eq!(
        server.call("PATCH", "/todos/1", Some(r#"{"done":"yes"}"#)),
        (400, r#"{"error":"done must be a boolean"}"#.into())
    );
    for (path, allow) in [("/todos", "GET, POST"), ("/todos/1", "GET, PATCH, DELETE")] {
        let (status, head, body) = server.send(format!("PUT {path} HTTP/1.1\r\n\r\n").as_bytes());
        assert_eq!(status, 405);
        assert!(head.contains(&format!("\r\nAllow: {allow}\r\n")), "{head}");
        assert_eq!(body, r#"{"error":"method not allowed"}"#);
    }
    let unsupported = r#"{"error":"Content-Type must be application/json"}"#;
    for content_type in ["", "Content-Type: text/plain\r\n"] {
        let body = r#"{"title":"x"}"#;
        let (status, _, response) = server.send(
            format!(
                "POST /todos HTTP/1.1\r\n{content_type}Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        assert_eq!((status, response.as_str()), (415, unsupported));
    }
    let (status, _, body) = server.send(
        b"PATCH /todos/1 HTTP/1.1\r\nContent-Type: Application/JSON; charset=utf-8\r\nContent-Length: 14\r\n\r\n{\"done\":false}",
    );
    assert_eq!((status, body.as_str()), (200, created));
    let (status, _, body) = server.send(b"BAD\0 / HTTP/1.1\r\n\r\n");
    assert_eq!(
        (status, body.as_str()),
        (400, r#"{"error":"malformed HTTP request"}"#)
    );

    let listed = todo_api_concurrent_creates(&server, 3);
    assert_eq!(listed.matches("\"id\":").count(), 3, "{listed}");

    // The state file holds the latest list and next id; a restart serves it unchanged.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
    assert_eq!(saved["next_id"], 5);
    assert_eq!(saved["todos"].as_array().unwrap().len(), 3);
    drop(server);
    let server = TodoServer::start(&manifest, &grants);
    assert_eq!(server.call("GET", "/todos", None), (200, listed));
    assert_eq!(
        server.call("POST", "/todos", Some(r#"{"title":"shear"}"#)),
        (201, r#"{"id":5,"title":"shear","done":false}"#.into())
    );
    drop(server);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn filesystem_cli_records_host_lifecycle_and_replays_after_the_mount_disappears() {
    let fixture = Fixture::new(
        "filesystem",
        include_str!("../../../examples/filesystem/main.hir"),
    );
    let executable = fixture.native_executable();
    let root = fixture.0.join("native/root");
    let output_dir = root.join("output");
    std::fs::create_dir(&output_dir).unwrap();
    let tape = fixture.0.join("filesystem.tape");
    let recorded = Command::new(&executable)
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_TAPE", &tape)
        .output()
        .unwrap();
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let text = String::from_utf8(recorded.stdout.clone()).unwrap();
    assert!(text.starts_with("build/empty directory "), "{text}");
    assert!(text.ends_with("build/value regular 5\n"), "{text}");
    assert_eq!(text.lines().count(), 2);
    assert!(std::fs::read_dir(&output_dir).unwrap().next().is_none());
    std::fs::remove_dir_all(&root).unwrap();
    let replay = Command::new(&executable)
        .env_clear()
        .env("HIRSEL_RT_REPLAY", &tape)
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(replay.stdout, recorded.stdout);
    assert!(replay.stderr.is_empty());
}

/// A copied example directory: its sources, its manifest with std at this checkout,
/// and a `data/` directory the executables' grants mount. Interpreted runs use the
/// same files through the simulation, in this process, so std is checked once.
struct Example {
    dir: PathBuf,
    files: Vec<(String, Vec<u8>)>,
}

/// An exit code, stdout and stderr.
type Outcome = (i32, String, String);

impl Example {
    fn new(name: &str, sources: &[(&str, &str)], manifest: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-example-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        for (path, source) in sources {
            std::fs::write(dir.join(path), source).unwrap();
        }
        let mut manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
        manifest["dependencies"][0]["manifest"] = serde_json::json!(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json")
                .canonicalize()
                .unwrap()
        );
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        Self {
            dir,
            files: Vec::new(),
        }
    }

    fn data(&mut self, path: &str, contents: impl AsRef<[u8]>) {
        std::fs::write(self.dir.join("data").join(path), contents.as_ref()).unwrap();
        self.files.push((path.into(), contents.as_ref().to_vec()));
    }

    /// A grants template: stdout, stderr and `access` to `data/`.
    fn grants(&self, access: &str) -> PathBuf {
        let path = self.dir.join("grants.json");
        let grants = serde_json::json!({"io": {
            "stdout": "stdout.bytes@1",
            "stderr": "stderr.bytes@1",
            "dirs": [{"path": "", "host": self.dir.join("data"), "access": access}],
        }});
        std::fs::write(&path, grants.to_string()).unwrap();
        path
    }

    /// `hirsel build` for `target`, in this process.
    fn build(&self, target: hirsel_driver::native::TargetSpec) -> PathBuf {
        let exe = self.dir.join(format!("program-{}", target.triple));
        hirsel_engine::with_compiler_stack(|| {
            hirsel_driver::native::build_path_native(
                &self.dir.join("hirsel.json"),
                &exe,
                target,
                Default::default(),
                None,
            )
        })
        .unwrap()
        .unwrap();
        exe
    }

    fn interpret(&self, args: &[&str]) -> Outcome {
        let mut simulation = hirsel_engine::SimulationConfig::default();
        simulation.grants = ["Io", "Args"].map(String::from).into();
        simulation.projections = ["stdout", "stderr", "dir"].map(String::from).into();
        simulation.args = args.iter().map(|a| a.to_string()).collect();
        simulation.files = self.files.iter().cloned().collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = hirsel_engine::with_compiler_stack(|| {
            hirsel_driver::run_path_with_simulation(
                &self.dir.join("hirsel.json"),
                simulation,
                &mut stdout,
                &mut stderr,
            )
        })
        .unwrap();
        let hirsel_engine::RunOutcome::Exit(code) = result else {
            panic!("{args:?}: {result:?}");
        };
        (
            code,
            String::from_utf8_lossy(&stdout).into(),
            String::from_utf8_lossy(&stderr).into(),
        )
    }

    /// Launch an executable with the grants template; AArch64 runs under the pinned qemu.
    fn launch(&self, exe: &std::path::Path, grants: &std::path::Path, args: &[&str]) -> Outcome {
        let mut command = if exe.to_string_lossy().contains("aarch64") {
            let mut qemu = Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap());
            qemu.arg("-L")
                .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                .arg(exe);
            qemu
        } else {
            Command::new(exe)
        };
        let output = command
            .arg("--hirsel-grants")
            .arg(grants)
            .args(args)
            .env_clear()
            .output()
            .unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into(),
            String::from_utf8_lossy(&output.stderr).into(),
        )
    }
}

impl Drop for Example {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Each case runs interpreted and as the built x86-64 executable (`aarch64`: as the
/// AArch64 executable under qemu instead), with one expectation.
fn check_cases(example: &Example, access: &str, aarch64: bool, cases: &[Case]) {
    use hirsel_driver::native::TargetSpec;
    let grants = example.grants(access);
    let exe = example.build(if aarch64 {
        TargetSpec::aarch64_unknown_linux_gnu()
    } else {
        TargetSpec::x86_64_unknown_linux_gnu()
    });
    for &(args, (code, stdout, stderr)) in cases {
        let expected = (code, stdout.to_string(), stderr.to_string());
        if !aarch64 {
            assert_eq!(example.interpret(args), expected, "interpreted {args:?}");
        }
        assert_eq!(
            example.launch(&exe, &grants, args),
            expected,
            "{} {args:?}",
            exe.display()
        );
    }
}

/// Arguments, then the expected exit code, stdout and stderr.
type Case<'a> = (&'a [&'a str], (i32, &'a str, &'a str));

fn logq(aarch64: bool) {
    let mut example = Example::new(
        "logq",
        &[("main.hir", include_str!("../../../examples/logq/main.hir"))],
        include_str!("../../../examples/logq/hirsel.json"),
    );
    example.data(
        "app.log",
        concat!(
            "{\"level\":\"info\",\"msg\":\"server started\",\"port\":8080}\n",
            "{\"level\":\"warn\",\"msg\":\"disk almost full\",\"disk\":\"sda\"}\n",
            "INFO request served\n",
            "ERROR disk write failed\n",
            "{\"level\":\"error\",\"msg\":\"disk write failed\",\"disk\":\"sdb\"}\n",
            "{\"level\":\"info\",\"message\":\"request served\",\"path\":\"/kv\"}\n",
            "\n",
            "warn  disk almost full\n",
            "info request served\n",
            "{\"level\":\"debug\",\"msg\":\"ünïcode ✓\",\"ok\":true}",
        ),
    );
    example.data("bad.log", "INFO fine\n{\"level\":\"info\"\n");
    example.data("controls.log", "INFO \u{0}ok\nWARN \u{7f}\n");
    example.data("latin1.log", b"INFO caf\xe9\n");
    let usage =
        "error: usage: logq [--level LEVEL] [--match TEXT] [--field KEY=VALUE] [--top N] FILE\n";
    check_cases(
        &example,
        "read",
        aarch64,
        &[
            (
                &["app.log"],
                (
                    0,
                    "matched 9\nDEBUG 1\nERROR 2\nINFO 4\nWARN 2\ntop messages:\n  3 request served\n  2 disk almost full\n  2 disk write failed\n  1 server started\n  1 ünïcode ✓\n",
                    "",
                ),
            ),
            (
                &["--level", "Warn", "--top", "1", "app.log"],
                (
                    0,
                    "matched 2\nWARN 2\ntop messages:\n  2 disk almost full\n",
                    "",
                ),
            ),
            (
                &["app.log", "--match", "disk", "--field", "disk=sdb"],
                (
                    0,
                    "matched 1\nERROR 1\ntop messages:\n  1 disk write failed\n",
                    "",
                ),
            ),
            (
                &["--field", "ok=true", "app.log"],
                (0, "matched 1\nDEBUG 1\ntop messages:\n  1 ünïcode ✓\n", ""),
            ),
            (
                &["--match", "nothing", "app.log"],
                (0, "matched 0\ntop messages:\n", ""),
            ),
            (
                &["controls.log"],
                (
                    0,
                    "matched 2\nINFO 1\nWARN 1\ntop messages:\n  1 \u{0}ok\n  1 \u{7f}\n",
                    "",
                ),
            ),
            (
                &["bad.log"],
                (1, "", "error: line 2: invalid JSON object\n"),
            ),
            (&["latin1.log"], (1, "", "error: line 1: invalid UTF-8\n")),
            (
                &["missing.log"],
                (1, "", "error: no such file: missing.log\n"),
            ),
            (
                &["--top", "x", "app.log"],
                (1, "", "error: --top needs a number\n"),
            ),
            (
                &["--field", "disk", "app.log"],
                (1, "", "error: --field needs KEY=VALUE\n"),
            ),
            (
                &["--bogus", "app.log"],
                (1, "", "error: unknown flag --bogus\n"),
            ),
            (
                &["app.log", "--level"],
                (1, "", "error: --level needs a value\n"),
            ),
            (&[], (1, "", usage)),
            (&["a.log", "b.log"], (1, "", usage)),
        ],
    );
}

#[test]
fn logq_filters_counts_and_ranks_mixed_logs() {
    logq(false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_logq_aarch64() {
    logq(true);
}
