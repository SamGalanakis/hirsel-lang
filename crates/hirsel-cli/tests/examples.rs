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
fn literal_diagnostics_use_the_literal_kind() {
    for (literal, ty, message) in [
        (
            "true",
            "bool",
            "boolean literal needs an expected bool type",
        ),
        (
            r#""x""#,
            "String",
            "string literal needs an expected String type",
        ),
        (
            r#"b"x""#,
            "Bytes",
            "byte string literal needs an expected Bytes type",
        ),
        (
            "3",
            "u64",
            "literal needs an expected type or numeric suffix",
        ),
        (
            "3.5",
            "f64",
            "literal needs an expected type or numeric suffix",
        ),
    ] {
        let rejected =
            format!("pub fn main() -> Result[(), ()] {{ let value = {literal}; Ok(()) }}");
        let fixture = Fixture::new("diagnostic", &rejected);
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("check")
            .arg(fixture.0.join("hirsel.json"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[LANG-2]"), "{stderr}");
        assert!(stderr.contains(message), "{stderr}");
        let accepted =
            format!("pub fn main() -> Result[(), ()] {{ let value: {ty} = {literal}; Ok(()) }}");
        std::fs::write(fixture.0.join("main.hir"), accepted).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("check")
            .arg(fixture.0.join("hirsel.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn todo_persists_between_native_launches() {
    let fixture = Fixture::new("todo", include_str!("../../../examples/todo/main.hir"));
    let executable = fixture.native_executable();
    let root = fixture.0.join("native/root");
    for (commands, expected) in [
        ("init\nadd feed sheep\n", "1. feed sheep\n"),
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
                .starts_with("--max-steps requires an unsigned integer\n")
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
