//! End-to-end examples: exercise CLI manifests, std selection, actual output,
//! and native linking rather than duplicating language conformance tests.

hirsel_guard::install!();

#[path = "../src/execution_policy.rs"]
mod execution_policy;
#[path = "support/tasks_cache.rs"]
mod tasks_cache;
use execution_policy::DEFAULT_RUN_MAX_STEPS;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);

// Check only the complete recipes changed by friction-truth-20. Historical
// captures and the existing application/runtime matrices have separate controls.
fn friction_recipe<'a>(document: &'a str, heading: &str, index: usize) -> &'a str {
    let section = document.split_once(heading).unwrap().1;
    let section = section.split("\n##").next().unwrap();
    section
        .split("```hirsel\n")
        .skip(1)
        .nth(index)
        .unwrap()
        .split_once("```")
        .unwrap()
        .0
}

fn export_friction_recipe(name: &str, source: &str) {
    let fixture = Fixture::new(name, source);
    let checked = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_JOBS", "1")
        .args(["check", "--json"])
        .arg(fixture.0.join("hirsel.json"))
        .output()
        .unwrap();
    assert!(checked.status.success(), "{name}: {checked:?}");
    let artifact = fixture.0.join("recipe.tir");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_JOBS", "1")
        .args(["export", "--tir"])
        .arg(fixture.0.join("hirsel.json"))
        .arg("-o")
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(output.status.success(), "{name}: {output:?}");
    let text = std::fs::read_to_string(artifact).unwrap();
    hirsel_engine::with_compiler_stack(|| {
        let mut program = hirsel_driver::load_program(&fixture.0.join("hirsel.json")).unwrap();
        program.check_for_execution().unwrap();
        program
            .engine
            .import_tir(&text)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    });
}

#[test]
fn friction_truth_20_prefix_recipes() {
    let document = include_str!("../../../examples/wc2/FRICTION.md");
    let inferred = friction_recipe(document, "### Prefix inference and names", 0);
    assert_eq!(
        inferred,
        include_str!("../../../examples/wc2/repros/incident-3.hir")
    );
    export_friction_recipe("friction_prefix", inferred);
    let contextual = friction_recipe(document, "### Prefix inference and names", 1);
    export_friction_recipe("friction_context", contextual);
    // Removing context from each insufficient prefix must still reject. A
    // later suffixed value or constructor cannot repair the first element.
    for annotation in [": List[String]", ": List[u64]", ": List[Option[String]]"] {
        let source = contextual.replace(annotation, "");
        let fixture = Fixture::new("friction_insufficient_prefix", &source);
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_JOBS", "1")
            .args(["check", "--json"])
            .arg(fixture.0.join("hirsel.json"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("\"rule\":\"LANG-2\""));
    }
}

#[test]
fn friction_truth_20_task_result_recipe() {
    let document = include_str!("../../../examples/kvd/FRICTION.md");
    let source = friction_recipe(document, "## Current task result recipe", 0);
    export_friction_recipe("friction_task_result", source);
}

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
            "fetchjson" => {
                serde_json::from_str(include_str!("../../../examples/fetchjson/hirsel.json"))
                    .unwrap()
            }
            "httpget" => {
                serde_json::from_str(include_str!("../../../examples/httpget/hirsel.json")).unwrap()
            }
            "udp-query" => {
                serde_json::from_str(include_str!("../../../examples/udp-query/hirsel.json"))
                    .unwrap()
            }
            "unix-http" => {
                serde_json::from_str(include_str!("../../../examples/unix-http/hirsel.json"))
                    .unwrap()
            }
            _ => {
                serde_json::json!({"schema":1,"package":{"name":name,"version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"common.hir","module":["common"]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]})
            }
        };
        for source in manifest["sources"].as_array_mut().unwrap() {
            if source["module"] == serde_json::json!(["common"]) {
                source["path"] = serde_json::json!("common.hir");
            }
        }
        manifest["dependencies"][0]["manifest"] = serde_json::json!(stdlib);
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        Self(dir)
    }
    fn run(&self, native: bool, simulation: Option<serde_json::Value>, expected: &str, code: i32) {
        self.run_with_report(native, simulation, expected, code, "");
    }
    fn run_with_report(
        &self,
        native: bool,
        simulation: Option<serde_json::Value>,
        expected: &str,
        code: i32,
        report: &str,
    ) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        // Without a simulated world the demo is granted host stdout, a fresh empty
        // read-write directory and empty Args.
        let (flag, config) = match simulation {
            Some(config) => ("--simulation", config),
            None => {
                let root = self.0.join("granted");
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir(&root).unwrap();
                (
                    "--grants",
                    serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"","host":root,"access":"read_write"}]},"args":[]}),
                )
            }
        };
        let path = self.0.join("run.json");
        std::fs::write(&path, config.to_string()).unwrap();
        command.arg(flag).arg(path);
        let output = command.arg(self.0.join("hirsel.json")).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
        assert_eq!(output.stderr, report.as_bytes());
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
    /// W1: the shipped optimized runtime, on both emitters, with every
    /// suspension changing the task's compute worker.
    fn migrated(&self, backend: &str, expected: &str) {
        let executable = self.0.join(format!("migrate-{backend}"));
        let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        build.args(["build", "--backend", backend]);
        if backend == "llvm" {
            build.arg("--release");
        }
        let built = build
            .arg(self.0.join("hirsel.json"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(built.status.success(), "{backend}: {built:?}");
        let root = self.0.join(format!("migrate-root-{backend}"));
        std::fs::create_dir(&root).unwrap();
        let grants = self.0.join(format!("migrate-grants-{backend}.json"));
        std::fs::write(
            &grants,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"","host":root,"access":"read_write"}]},"args":[]}).to_string(),
        ).unwrap();
        let ran = Command::new(executable)
            .arg("--hirsel-grants")
            .arg(grants)
            .env_clear()
            .env("HIRSEL_RT_MIGRATE", "1")
            .output()
            .unwrap();
        assert!(ran.status.success(), "{backend}: {ran:?}");
        assert_eq!(ran.stdout, expected.as_bytes(), "{backend}: {ran:?}");
        assert!(ran.stderr.is_empty(), "{backend}: {ran:?}");
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
        mod $name {
            #[test]
            fn migration() {
                let fixture = super::Fixture::new(stringify!($name), $source);
                for backend in ["llvm", "cranelift"] {
                    fixture.migrated(backend, $expected);
                }
            }
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
        "[{\"a\":\"x\\ny\",\"b\":\"\"}]\n",
        0,
    );
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
        hirsel_driver::run_path_with_step_budget(
            &fixture.0.join("hirsel.json"),
            simulation,
            Some(DEFAULT_RUN_MAX_STEPS),
            hirsel_engine::Verify::On,
            &mut output,
            &mut Vec::new(),
        )
    })
    .unwrap();
    assert_eq!(
        result,
        hirsel_engine::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
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
        fixture.run_with_report(false, Some(input(data, &[])), "", 1, "error: Invalid\n");
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
    invoke(&["--max-steps", "1"], 3, "evaluator tooling limit: Steps\n");
    invoke(&["--max-steps=10000"], 0, "");
    invoke(&["--max-steps", "unbounded"], 0, "");
    invoke(&["--max-steps=0"], 3, "evaluator tooling limit: Steps\n");
    let simulation = fixture.0.join("simulation.json");
    std::fs::write(&simulation, "{}").unwrap();
    invoke(
        &[
            "--simulation",
            simulation.to_str().unwrap(),
            "--max-steps",
            "1",
        ],
        3,
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
        3,
        "evaluator tooling limit: Steps\n",
    );
    invoke(&["--run", record.to_str().unwrap()], 0, "");
    let grants = fixture.0.join("grants.json");
    std::fs::write(&grants, "{}").unwrap();
    invoke(
        &["--grants", grants.to_str().unwrap(), "--max-steps=1"],
        3,
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
    assert_eq!(bounded.status.code(), Some(3));
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
    fixture.run_with_report(false, Some(serde_json::json!({"grants":["Io","Args"],"projections":["stdout","stdin"],"args":["-"],"stdin":[49,32,43,32,50,10,255]})), "3\n", 1, "error: Input(InvalidUtf8)\n");
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
            assert_eq!(
                output.stderr,
                if code == 0 {
                    b"".as_slice()
                } else {
                    b"error: Input(InvalidUtf8)\n"
                }
            );
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
        Self::launch(manifest, grants, false)
    }

    /// Run the server, compiled with `native`, until it prints its readiness line.
    fn launch(manifest: &std::path::Path, grants: &std::path::Path, native: bool) -> Self {
        use std::io::BufRead;
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        let mut child = command
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
        // A native run compiles first, and its first build also builds the runtime.
        let line = recv
            .recv_timeout(std::time::Duration::from_secs(if native {
                300
            } else {
                90
            }))
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
fn full_gate_todo_api_serves_clients_concurrently_and_persists_native() {
    let (dir, manifest, grants) = todo_api_fixture();
    // The compiled server: a task per connection, the store behind a Mutex and the writer
    // task over a channel run on the native scheduler.
    let server = TodoServer::launch(&manifest, &grants, true);
    assert_eq!(server.call("GET", "/todos", None), (200, "[]".into()));
    let listed = todo_api_concurrent_creates(&server, 1);
    drop(server);
    let server = TodoServer::launch(&manifest, &grants, true);
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
    assert_eq!((status, body.as_str()), (400, "Bad Request"));

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
            hirsel_driver::native::build_path_native_backend_with_validation(
                &self.dir.join("hirsel.json"),
                &exe,
                target,
                Default::default(),
                hirsel_driver::native::Backend::Llvm,
                None,
                false,
                hirsel_engine::Verify::On,
                hirsel_driver::native::Validation::EveryStage,
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
            hirsel_driver::run_path_with_step_budget(
                &self.dir.join("hirsel.json"),
                simulation,
                Some(DEFAULT_RUN_MAX_STEPS),
                hirsel_engine::Verify::On,
                &mut stdout,
                &mut stderr,
            )
        })
        .unwrap();
        let hirsel_engine::RunOutcome::Returned(_) = result else {
            panic!("{args:?}: {result:?}");
        };
        (
            result.exit_code(),
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
            .arg("--")
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

fn mdtoc(aarch64: bool) {
    let mut example = Example::new(
        "mdtoc",
        &[("main.hir", include_str!("../../../examples/mdtoc/main.hir"))],
        include_str!("../../../examples/mdtoc/hirsel.json"),
    );
    example.data(
        "guide.md",
        concat!(
            "# Hirsel Guide\n\nIntro.\n\n## Getting Started\n\n```sh\n# not a heading\n```\n\n",
            "## Getting Started\n\n### Ünïcode & Café ✓ ##\n\n#hashtag is text\n\n~~~\n## fenced\n~~~\n\n",
            "#### Deep_dive: 1-2-3 ###\n\n##   Spaced   Out   \r\n## Getting Started\n####### seven\n",
        ),
    );
    example.data("unicode.md", "## Ünïcode\n");
    // A decomposed é (e + U+0301) slugs as its NFC composition.
    example.data("nfd.md", "## Cafe\u{301}\n");
    example.data("empty.md", "no headings\n");
    example.data("latin1.md", b"# caf\xe9\n");
    check_cases(
        &example,
        "read",
        aarch64,
        &[
            (
                &["guide.md"],
                (
                    0,
                    concat!(
                        "- [Hirsel Guide](#hirsel-guide)\n",
                        "  - [Getting Started](#getting-started)\n",
                        "  - [Getting Started](#getting-started-1)\n",
                        "    - [Ünïcode & Café ✓](#ünïcode--café-✓)\n",
                        "      - [Deep_dive: 1-2-3](#deep_dive-1-2-3)\n",
                        "  - [Spaced   Out](#spaced---out)\n",
                        "  - [Getting Started](#getting-started-2)\n",
                    ),
                    "",
                ),
            ),
            (&["unicode.md"], (0, "- [Ünïcode](#ünïcode)\n", "")),
            (&["nfd.md"], (0, "- [Cafe\u{301}](#café)\n", "")),
            (&["empty.md"], (0, "", "")),
            (&["latin1.md"], (1, "", "error: latin1.md is not UTF-8\n")),
            (
                &["missing.md"],
                (1, "", "error: no such file: missing.md\n"),
            ),
            (&[], (1, "", "error: usage: mdtoc FILE\n")),
        ],
    );
}

#[test]
fn mdtoc_slugs_unicode_headings_outside_fences() {
    mdtoc(false);
}

fn bench_lite(aarch64: bool) {
    let example = Example::new(
        "bench-lite",
        &[(
            "main.hir",
            include_str!("../../../examples/bench-lite/main.hir"),
        )],
        include_str!("../../../examples/bench-lite/hirsel.json"),
    );
    let small = "words 40\ndifferent 35\nbytes 152\nchecksum 3136792867699900722\n  fo 3\n  ce 2\n  di 2\n  xu 2\n  ba 1\nheap and arena agree\n";
    check_cases(
        &example,
        "read",
        aarch64,
        &[
            (&["40", "7"], (0, small, "")),
            (
                &["200", "7"],
                (
                    0,
                    "words 200\ndifferent 150\nbytes 806\nchecksum 11645186517551137659\n  ce 6\n  ba 5\n  di 5\n  fo 5\n  ne 5\nheap and arena agree\n",
                    "",
                ),
            ),
            (
                &["0"],
                (
                    0,
                    "words 0\ndifferent 0\nbytes 0\nchecksum 14695981039346656037\nheap and arena agree\n",
                    "",
                ),
            ),
            (&["x"], (1, "", "error: not a number: x\n")),
            (
                &["1", "2", "3", "4"],
                (
                    1,
                    "",
                    "error: usage: bench-lite [WORDS [SEED [ARENA_BYTES]]]\n",
                ),
            ),
        ],
    );
}

#[test]
fn bench_lite_heap_and_arena_summaries_agree() {
    bench_lite(false);
}

/// examples/kvstore in a fresh directory with a data directory and grants that
/// listen on an ephemeral port; returns it with its manifest and grants.
fn kvstore_fixture() -> (Example, PathBuf, PathBuf) {
    let example = Example::new(
        "kvstore",
        &[(
            "main.hir",
            include_str!("../../../examples/kvstore/main.hir"),
        )],
        include_str!("../../../examples/kvstore/hirsel.json"),
    );
    let dir = example.dir.clone();
    let grants = serde_json::json!({
        "io": {
            "stdout": "stdout.bytes@1",
            "tasks": "tasks.cooperative@1",
            "dirs": [{"path": "data", "host": dir.join("data"), "access": "read_write"}],
            "net": {"provider": "net.stream@1", "rules": [{"address": "127.0.0.1", "service": "0", "operation": "listen"}]},
        },
        "args": ["0"],
    });
    std::fs::write(dir.join("grants.json"), grants.to_string()).unwrap();
    (example, dir.join("hirsel.json"), dir.join("grants.json"))
}

fn kv(server: &TodoServer, method: &str, path: &str, body: Option<&[u8]>) -> (u16, String, String) {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n").into_bytes();
    if let Some(body) = body {
        request.extend(format!("Content-Length: {}\r\n", body.len()).bytes());
    }
    request.extend(b"\r\n");
    request.extend(body.unwrap_or_default());
    server.send(&request)
}

#[test]
fn kvstore_serves_clients_concurrently_and_persists() {
    kvstore_session(false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_kvstore_serves_clients_concurrently_and_persists_native() {
    // The compiled server: a task per connection, the store behind a Mutex and the writer
    // task over a channel run on the native scheduler.
    kvstore_session(true);
}

/// Concurrent writers beside a held request, every route, and a restart, with the
/// server run by the interpreter or compiled with `native`.
fn kvstore_session(native: bool) {
    let (example, manifest, grants) = kvstore_fixture();
    let server = TodoServer::launch(&manifest, &grants, native);
    assert_eq!(kv(&server, "GET", "/kv", None).2, "{}");
    // An incomplete request holds its own task while two clients write.
    let mut held =
        std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, server.port)).unwrap();
    {
        use std::io::Write;
        held.write_all(b"GET /kv/sheep HTTP/1.1\r\nHost:").unwrap();
    }
    std::thread::scope(|scope| {
        let writers: Vec<_> = [("sheep", "woolly"), ("caf%C3%A9", "βeta")]
            .map(|(key, value)| {
                let server = &server;
                scope.spawn(move || {
                    kv(server, "PUT", &format!("/kv/{key}"), Some(value.as_bytes())).0
                })
            })
            .into_iter()
            .collect();
        for writer in writers {
            assert_eq!(writer.join().unwrap(), 201);
        }
    });
    {
        use std::io::{Read, Write};
        held.write_all(b" x\r\n\r\n").unwrap();
        held.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        held.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
        assert!(response.ends_with("\r\n\r\nwoolly"), "{response}");
    }
    let (status, head, body) = kv(&server, "PUT", "/kv/sheep", Some(b"shorn"));
    assert_eq!((status, body.as_str()), (200, ""), "{head}");
    let (status, head, body) = kv(&server, "GET", "/kv/sheep", None);
    assert_eq!((status, body.as_str()), (200, "shorn"));
    assert!(
        head.contains("\r\nContent-Type: text/plain; charset=utf-8\r\n"),
        "{head}"
    );
    assert_eq!(kv(&server, "DELETE", "/kv/sheep", None).0, 204);
    assert_eq!(kv(&server, "DELETE", "/kv/sheep", None).0, 404);
    assert_eq!(kv(&server, "GET", "/kv/sheep", None).0, 404);
    assert_eq!(kv(&server, "PUT", "/kv/x", Some(b"\xff")).0, 400);
    assert_eq!(kv(&server, "GET", "/kv/%FF", None).0, 400);
    assert_eq!(kv(&server, "GET", "/nope", None).0, 404);
    let (status, head, _) = kv(&server, "POST", "/kv/x", Some(b"v"));
    assert_eq!(status, 405);
    assert!(head.contains("\r\nAllow: GET, PUT, DELETE\r\n"), "{head}");
    let listed = kv(&server, "GET", "/kv", None);
    assert_eq!((listed.0, listed.2.as_str()), (200, r#"{"café":"βeta"}"#));
    assert_eq!(
        std::fs::read_to_string(example.dir.join("data/kv.json")).unwrap(),
        r#"{"café":"βeta"}"#
    );
    // A restart serves the saved entries.
    drop(server);
    let server = TodoServer::launch(&manifest, &grants, native);
    assert_eq!(kv(&server, "GET", "/kv/caf%C3%A9", None).2, "βeta");
}

#[test]
fn csvjson_streams_multiline_base64_and_hashes_decoded_bytes() {
    let fixture = Fixture::new(
        "csvjson",
        include_str!("../../../examples/csvjson/main.hir"),
    );
    fixture.run(false, Some(serde_json::json!({
        "grants":["Io","Args"], "projections":["stdout"], "args":["--help"]
    })), "Usage: csvjson [options] [--] [arguments]\n  -h, --help\tshow usage\n  -b, --base64-field <value>\tdecode/re-encode this field and add its _sha256 digest\n", 0);
    fixture.run(false, Some(serde_json::json!({
        "grants":["Io","Args"], "projections":["stdin","stdout","stderr"],
        "args":["--base64-field","body","-"],
        "stdin":"id,body,note\n1,YWJj,\"a\nb\"\n"
    })), "[{\"id\":\"1\",\"body\":\"YWJj\",\"note\":\"a\\nb\",\"body_sha256\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\"}]\n", 0);
}

#[test]
fn csvjson_errors_are_one_escaped_structured_record() {
    let fixture = Fixture::new(
        "csvjson",
        include_str!("../../../examples/csvjson/main.hir"),
    );
    let config = serde_json::json!({"grants":["Io","Args"],"projections":["stdin","stdout","stderr"],"args":["-b","body","-"],"stdin":"id,body\n1,\"bad\"\"\nx\"\n"});
    let path = fixture.0.join("error.json");
    std::fs::write(&path, config.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(path)
        .arg(fixture.0.join("hirsel.json"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr.iter().filter(|byte| **byte == b'\n').count(),
        2
    );
    let split = output
        .stderr
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap();
    assert_eq!(&output.stderr[split + 1..], b"error: Invalid\n");
    let record: serde_json::Value = serde_json::from_slice(&output.stderr[..split]).unwrap();
    assert_eq!(record["level"], "ERROR");
    assert_eq!(record["msg"], "csvjson failed");
    assert_eq!(record["reason"], "invalid base64 field: bad\"\nx");
}

#[test]
fn testrun_streams_a_test_and_stops_it_at_its_deadline() {
    use std::os::unix::fs::PermissionsExt;
    let example = Example::new(
        "testrun",
        &[(
            "main.hir",
            include_str!("../../../examples/testrun/main.hir"),
        )],
        include_str!("../../../examples/testrun/hirsel.json"),
    );
    let root = example.dir.canonicalize().unwrap();
    let pid = root.join("hang.pid");
    let script = |name: &str, body: String| {
        let path = root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    // No outcome races the clock: a test that ends by itself has an hour, and the hung test
    // prints nothing and never ends by itself, so only its deadline can stop it.
    let cases = [
        (
            script(
                "pass.sh",
                "echo one\nprintf 'two\\nthr'\nsleep 0.1\necho ee\n".into(),
            ),
            "3600000",
            "| one\n| two\n| three\nPASS\n",
        ),
        (
            script("fail.sh", "echo bad\nexit 3\n".into()),
            "3600000",
            "| bad\nFAIL: exit 3\n",
        ),
        (
            script(
                "hang.sh",
                format!("echo $$ > {}\nexec sleep 3600\n", pid.display()),
            ),
            "300",
            "TIMEOUT\n",
        ),
    ];
    for (program, limit, expected) in &cases {
        let grants = root.join("grants.json");
        std::fs::write(
            &grants,
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1",
                "dirs": [],
                "process": {"provider": "process.spawn@1", "rules": [{"program": program}]},
            }, "args": [program, limit]})
            .to_string(),
        )
        .unwrap();
        for native in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(root.join("hirsel.json"));
            if native {
                command.arg("--native");
            }
            let output = command.arg("--grants").arg(&grants).output().unwrap();
            assert_eq!(
                (
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout)
                ),
                (Some(0), (*expected).into()),
                "native={native} {}: {}",
                program.display(),
                String::from_utf8_lossy(&output.stderr)
            );
            if program.ends_with("hang.sh") {
                // The deadline killed and reaped the hung test before testrun exited.
                let hung = std::fs::read_to_string(&pid).unwrap();
                let alive = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(format!("kill -0 {} 2>/dev/null", hung.trim()))
                    .status()
                    .unwrap();
                assert!(
                    !alive.success(),
                    "native={native}: the hung test {} survived",
                    hung.trim()
                );
                std::fs::remove_file(&pid).unwrap();
            }
        }
    }
}

fn check_example_tests(name: &str, source: &str, count: u64) {
    let fixture = Fixture::new(&name.replace('-', "_"), source);
    check_package_tests(name, &fixture.0.join("hirsel.json"), count);
}

/// `hirsel test` on a package manifest, interpreted, with LLVM and with Cranelift.
fn check_package_tests(name: &str, manifest: &std::path::Path, count: u64) {
    check_package_tests_on(
        name,
        manifest,
        count,
        &[None, Some("llvm"), Some("cranelift")],
    );
}

/// A package's own tests on the interpreter and LLVM only: Cranelift parity is gated by
/// testing.rs's native_test_engines_match_interpreted_* and the Cranelift campaign.
fn check_interpreted_and_llvm_package_tests(name: &str, manifest: &std::path::Path, count: u64) {
    check_package_tests_on(name, manifest, count, &[None, Some("llvm")]);
}

fn check_package_tests_on(
    name: &str,
    manifest: &std::path::Path,
    count: u64,
    backends: &[Option<&str>],
) {
    for &backend in backends {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.args(["test", "--json"]);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.arg(manifest).output().unwrap();
        assert!(output.status.success(), "{name} {backend:?}: {output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            json["summary"]["total"], count,
            "{name} {backend:?}: {json}"
        );
        assert_eq!(json["summary"]["failed"], 0, "{name} {backend:?}: {json}");
    }
}

#[test]
fn example_packages_run_their_declared_tests() {
    check_example_tests(
        "calculator",
        include_str!("../../../examples/calculator/main.hir"),
        3,
    );
    check_example_tests("mdtoc", include_str!("../../../examples/mdtoc/main.hir"), 2);
}

// Keep each package independently selectable in the shared pool's focus mode.
macro_rules! example_tests {
    ($case:ident, $package:literal, $count:literal) => {
        #[test]
        fn $case() {
            check_example_tests(
                $package,
                include_str!(concat!("../../../examples/", $package, "/main.hir")),
                $count,
            );
        }
    };
}
example_tests!(adventure_package_tests, "adventure", 1);
example_tests!(bench_lite_package_tests, "bench-lite", 1);
example_tests!(csvjson_package_tests, "csvjson", 4);
example_tests!(jsonquery_package_tests, "jsonquery", 2);
example_tests!(kvstore_package_tests, "kvstore", 2);
example_tests!(logq_package_tests, "logq", 2);
example_tests!(logsummary_package_tests, "logsummary", 1);
example_tests!(markdown_package_tests, "markdown", 1);
example_tests!(sortuniq_package_tests, "sortuniq", 1);
example_tests!(testrun_package_tests, "testrun", 1);
example_tests!(todo_package_tests, "todo", 1);
example_tests!(todo_api_package_tests, "todo-api", 2);
example_tests!(wordscan_package_tests, "wordscan", 1);

/// Schedule exploration (docs/building.md, "Schedules"), which no corpus header can select:
/// the race example's lost addition, which fifo@1 never schedules, is found by random@1 at
/// seed 1 and by pct@1 at seed 7, alike interpreted and on both native backends, and the
/// printed replay command reproduces the same failure and report.
#[test]
fn explore_finds_the_race_examples_lost_addition_and_its_replay_reproduces_it() {
    let fixture = Fixture::new("race", include_str!("../../../examples/race/main.hir"));
    let manifest = fixture.0.join("hirsel.json");
    let first_test = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {output:?}"));
        json["tests"][0].clone()
    };
    let manifest_arg = manifest.to_str().unwrap();
    for engine in [
        &[][..],
        &["--native", "--backend", "llvm"],
        &["--native", "--backend", "cranelift"],
    ] {
        let mut args = vec!["test", manifest_arg];
        args.extend(engine);
        let fifo = first_test(&args);
        assert_eq!(fifo["status"], "passed", "{engine:?}: {fifo}");
        for (policy, found) in [("random@1", 1u64), ("pct@1", 7)] {
            let mut explore = args.clone();
            explore.extend(["--explore", "20", "--scheduler", policy]);
            let explored = first_test(&explore);
            assert_eq!(explored["status"], "failed", "{engine:?}: {explored}");
            assert_eq!(explored["scheduler"], format!("{policy}:{found}"));
            assert_eq!(explored["schedules"], found + 1);
            assert!(
                explored["failure"].as_str().unwrap().contains("LANG-9"),
                "{explored}"
            );
            let replay = explored["replay"].as_str().unwrap();
            let words: Vec<&str> = replay.split(' ').collect();
            assert_eq!(words[..2], ["hirsel", "test"], "{replay}");
            let replayed = first_test(&words[1..]);
            for field in ["name", "status", "failure", "report", "scheduler"] {
                assert_eq!(replayed[field], explored[field], "{replay}: {field}");
            }
            assert_eq!(replayed["schedules"], 1);
        }
    }
}

/// A seeded choice is a recorded scheduling input (docs/spec/determinism.md, "Scheduler"):
/// under random@1 seed 1 the race example loses an addition, its tape carries `schedule`
/// events, and the tape replays that output in every engine, whichever engine recorded it,
/// while a tape whose first choice is altered is refused.
#[test]
fn seeded_schedules_record_and_replay_across_engines() {
    let fixture = Fixture::new("race", include_str!("../../../examples/race/main.hir"));
    let manifest = fixture.0.join("hirsel.json");
    let grants = fixture.0.join("grants.json");
    std::fs::write(
        &grants,
        r#"{"inputs":{"hash_seed":0,"random_seed":0,"test_seed":0,"scheduler":{"algorithm":"random","version":1,"seed":1}},"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[]}"#,
    )
    .unwrap();
    let expected = b"lost an addition\n";
    let engines: [&[&str]; 3] = [
        &[],
        &["--native", "--backend", "llvm"],
        &["--native", "--backend", "cranelift"],
    ];
    let replay = |engine: &[&str], tape: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(engine)
            .arg(&manifest)
            .arg("--replay")
            .arg(tape)
            .output()
            .unwrap()
    };
    for (index, recorder) in engines[..2].iter().enumerate() {
        let tape = fixture.0.join(format!("race-{index}.tape"));
        let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(*recorder)
            .arg(&manifest)
            .arg("--grants")
            .arg(&grants)
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(live.status.success(), "{recorder:?}: {live:?}");
        assert_eq!(live.stdout, expected, "{recorder:?}");
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        assert!(
            text.lines().any(|line| line.starts_with("schedule 2 1 ")),
            "{recorder:?}: {text}"
        );
        for engine in engines {
            let replayed = replay(engine, &tape);
            assert!(
                replayed.status.success(),
                "{recorder:?} -> {engine:?}: {replayed:?}"
            );
            assert_eq!(replayed.stdout, expected, "{recorder:?} -> {engine:?}");
        }
        let altered = fixture.0.join(format!("race-{index}-altered.tape"));
        std::fs::write(&altered, text.replacen("schedule 2 1 ", "schedule 2 0 ", 1)).unwrap();
        for engine in engines {
            let replayed = replay(engine, &altered);
            assert!(
                !replayed.status.success(),
                "{recorder:?} -> {engine:?}: {replayed:?}"
            );
            assert_ne!(replayed.stdout, expected, "{recorder:?} -> {engine:?}");
        }
    }
}

// The corpus checks codecs; this runs the example's manifest and grant surface.
#[test]
fn fetchjson_reuses_one_scripted_connection() {
    let fixture = Fixture::new(
        "fetchjson",
        include_str!("../../../examples/fetchjson/main.hir"),
    );
    let config =
        serde_json::from_str(include_str!("../../../examples/fetchjson/demo.json")).unwrap();
    fixture.run(false, Some(config), "0\n1\n", 0);
}

/// Production package admission and tapes cross both native backends, which corpus
/// fixtures cannot check through the CLI's grants-template path.
#[test]
fn udp_query_records_and_replays_across_cli_backends() {
    let fixture = Fixture::new(
        "udp-query",
        include_str!("../../../examples/udp-query/main.hir"),
    );
    let expected = "sent 5; received 5 from port 19001; truncated false\n";
    fixture.run(
        false,
        Some(serde_json::from_str(include_str!("../../../examples/udp-query/demo.json")).unwrap()),
        expected,
        0,
    );
    let grants = fixture.0.join("grants.json");
    std::fs::write(
        &grants,
        include_str!("../../../examples/udp-query/grants.json"),
    )
    .unwrap();
    let tape = fixture.0.join("udp.tape");
    let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(fixture.0.join("hirsel.json"))
        .arg("--grants")
        .arg(&grants)
        .arg("--record")
        .arg(&tape)
        .output()
        .unwrap();
    assert!(
        live.status.success(),
        "{}",
        String::from_utf8_lossy(&live.stderr)
    );
    assert_eq!(live.stdout, expected.as_bytes());
    assert!(live.stderr.is_empty());
    // The live world and grants are gone: only the tape remains.
    std::fs::remove_file(grants).unwrap();
    for backend in ["llvm", "cranelift"] {
        let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(fixture.0.join("hirsel.json"))
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(
            replay.status.success(),
            "{backend}: {}",
            String::from_utf8_lossy(&replay.stderr)
        );
        assert_eq!(replay.stdout, expected.as_bytes(), "{backend}");
        assert!(replay.stderr.is_empty());
    }
}

/// A real Unix socket the program serves itself: the std client's `unix_socket` transport
/// reaches the program's own listener in the interpreter and on both native backends, a
/// stale socket file is replaced only under the granted rule, closing the listener removes
/// the file, and the interpreter's tape replays natively with no socket at all.
#[test]
fn unix_http_serves_itself_over_a_real_socket_and_replays_across_cli_backends() {
    use std::os::unix::fs::FileTypeExt;
    let fixture = Fixture::new(
        "unix-http",
        include_str!("../../../examples/unix-http/main.hir"),
    );
    // Socket paths are at most 107 bytes, so the socket lives in a short directory.
    let sockets = PathBuf::from(format!(
        "/tmp/hirsel-unix-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&sockets).unwrap();
    let socket = sockets.join("s.sock");
    let socket_text = socket.to_str().unwrap();
    let grants_text = include_str!("../../../examples/unix-http/grants.json")
        .replace("/tmp/hirsel-unix-http.sock", socket_text);
    let grants = fixture.0.join("grants.json");
    let expected = "request /status; status 200; body served /status\n";
    let run = |args: &[&str], grants_text: &str| {
        std::fs::write(&grants, grants_text).unwrap();
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(args)
            .arg(fixture.0.join("hirsel.json"))
            .arg("--grants")
            .arg(&grants)
            .output()
            .unwrap()
    };
    // A listener that exits without closing leaves its socket file behind.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(
        std::fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket()
    );
    let narrowed = grants_text.replace("unix_listen_remove_stale", "unix_listen");
    eprintln!("unix-http phase: stale-listener refusal");
    let refused = run(&[], &narrowed);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("AddrInUse"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(std::fs::symlink_metadata(&socket).is_ok());
    let tape = fixture.0.join("unix.tape");
    let tape_text = tape.to_str().unwrap().to_owned();
    for args in [
        vec!["--record", tape_text.as_str()],
        vec!["--native", "--backend", "llvm"],
        vec!["--native", "--backend", "cranelift"],
    ] {
        eprintln!("unix-http phase: live {args:?}");
        let live = run(&args, &grants_text);
        assert!(
            live.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&live.stderr)
        );
        assert_eq!(live.stdout, expected.as_bytes(), "{args:?}");
        assert!(live.stderr.is_empty(), "{args:?}");
        assert!(std::fs::symlink_metadata(&socket).is_err(), "{args:?}");
    }
    std::fs::remove_file(grants).unwrap();
    std::fs::remove_dir(&sockets).unwrap();
    for backend in ["llvm", "cranelift"] {
        eprintln!("unix-http phase: replay {backend}");
        let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(fixture.0.join("hirsel.json"))
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(
            replay.status.success(),
            "{backend}: {}",
            String::from_utf8_lossy(&replay.stderr)
        );
        assert_eq!(replay.stdout, expected.as_bytes(), "{backend}");
        assert!(replay.stderr.is_empty());
    }
}

/// A package in this repository, by its manifest's path from the root.
fn repository_manifest(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
        .canonicalize()
        .unwrap()
}

// Multi-file packages run in place: their manifests name siblings and dependencies by
// relative path.
#[test]
fn datastar_package_tests() {
    check_interpreted_and_llvm_package_tests(
        "datastar",
        &repository_manifest("packages/datastar@1/hirsel.json"),
        7,
    );
}

#[test]
fn datastar_api_reference_is_current() {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("doc")
        .arg(repository_manifest("packages/datastar@1/hirsel.json"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("../../../packages/datastar@1/api.md"),
        "regenerate with `hirsel doc packages/datastar@1/hirsel.json -o packages/datastar@1/api.md`"
    );
}

#[test]
fn protobuf_package_tests() {
    check_interpreted_and_llvm_package_tests(
        "protobuf",
        &repository_manifest("packages/protobuf@1/hirsel.json"),
        5,
    );
}

#[test]
fn grpc_package_tests() {
    check_interpreted_and_llvm_package_tests(
        "grpc",
        &repository_manifest("packages/grpc@1/hirsel.json"),
        6,
    );
}

#[test]
fn protobuf_and_grpc_api_references_are_current() {
    for (package, current) in [
        (
            "protobuf",
            include_str!("../../../packages/protobuf@1/api.md"),
        ),
        ("grpc", include_str!("../../../packages/grpc@1/api.md")),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("doc")
            .arg(repository_manifest(&format!(
                "packages/{package}@1/hirsel.json"
            )))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            current,
            "regenerate with `hirsel doc packages/{package}@1/hirsel.json -o packages/{package}@1/api.md`"
        );
    }
}

/// protoc@1 followed by `hirsel fmt` regenerates the checked-in greeter source byte
/// for byte, service stubs included, from the example's .proto.
#[test]
fn protoc_regenerates_the_greeter() {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-protoc-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helloworld.proto"),
        include_str!("../../../examples/grpc-greeter/helloworld.proto"),
    )
    .unwrap();
    let grants = dir.join("grants.json");
    std::fs::write(
        &grants,
        serde_json::json!({"io": {"dirs": [{"path": "", "host": dir, "access": "read_write"}]}})
            .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(repository_manifest(
            "packages/protobuf@1/protoc/hirsel.json",
        ))
        .arg("--grants")
        .arg(&grants)
        .args(["--", "--grpc", "greeter.hir", "helloworld.proto"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("fmt")
        .arg(dir.join("greeter.hir"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("greeter.hir")).unwrap(),
        include_str!("../../../examples/grpc-greeter/greeter.hir"),
        "regenerate examples/grpc-greeter/greeter.hir with protoc@1 --grpc, then hirsel fmt"
    );
}

#[test]
fn tasks_package_tests() {
    check_interpreted_and_llvm_package_tests(
        "tasks",
        &repository_manifest("examples/tasks/hirsel.json"),
        25,
    );
}

#[test]
fn dogfood5_wc2_package_tests() {
    let example = Example::new(
        "wc2",
        &[("main.hir", include_str!("../../../examples/wc2/main.hir"))],
        include_str!("../../../examples/wc2/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("wc2", &example.dir.join("hirsel.json"), 4);
}

#[test]
fn wc2_original_nested_file_loop_runs_on_both_backends() {
    for (name, source, args, expected) in [
        (
            "wc2_loop",
            include_str!("../../../examples/wc2/repros/invalid-origin.hir"),
            &[][..],
            "",
        ),
        (
            "wc2",
            include_str!("../../../examples/wc2/main.hir"),
            &["x", "y"][..],
            "3 3 10\n",
        ),
    ] {
        let fixture = Fixture::new(name, source);
        let data = fixture.0.join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("x"), b"one\ntwo\n").unwrap();
        std::fs::write(data.join("y"), b"z\n").unwrap();
        let grants = fixture.0.join("grants.json");
        std::fs::write(
            &grants,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[{"path":"data","host":data,"access":"read"}]},"args":args})
                .to_string(),
        )
        .unwrap();
        for engine in [
            &[][..],
            &["--native", "--backend", "llvm"][..],
            &["--native", "--backend", "cranelift"][..],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .env("XDG_CACHE_HOME", fixture.0.join("cache"))
                .arg("run")
                .args(engine)
                .arg(fixture.0.join("hirsel.json"))
                .arg("--grants")
                .arg(&grants)
                .output()
                .unwrap();
            assert!(output.status.success(), "{name}/{engine:?}: {output:?}");
            assert_eq!(output.stdout, expected.as_bytes(), "{name}/{engine:?}");
            assert!(output.stderr.is_empty(), "{name}/{engine:?}: {output:?}");
        }
    }
}

#[test]
fn dogfood5_csvstats_package_tests() {
    let example = Example::new(
        "csvstats",
        &[(
            "main.hir",
            include_str!("../../../examples/csvstats/main.hir"),
        )],
        include_str!("../../../examples/csvstats/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("csvstats", &example.dir.join("hirsel.json"), 4);
}

#[test]
fn dogfood5_linkcheck_package_tests() {
    let example = Example::new(
        "linkcheck",
        &[(
            "main.hir",
            include_str!("../../../examples/linkcheck/main.hir"),
        )],
        include_str!("../../../examples/linkcheck/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("linkcheck", &example.dir.join("hirsel.json"), 4);
}

#[test]
fn dogfood5_kvd_package_tests() {
    let example = Example::new(
        "kvd",
        &[("main.hir", include_str!("../../../examples/kvd/main.hir"))],
        include_str!("../../../examples/kvd/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("kvd", &example.dir.join("hirsel.json"), 3);
}

#[test]
fn dogfood5_logtail_package_tests() {
    let example = Example::new(
        "logtail",
        &[(
            "main.hir",
            include_str!("../../../examples/logtail/main.hir"),
        )],
        include_str!("../../../examples/logtail/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("logtail", &example.dir.join("hirsel.json"), 3);
}

#[test]
fn dogfood5_mdsite_package_tests() {
    let example = Example::new(
        "mdsite",
        &[
            (
                "main.hir",
                include_str!("../../../examples/mdsite/main.hir"),
            ),
            (
                "catalog.hir",
                include_str!("../../../examples/mdsite/catalog.hir"),
            ),
            (
                "render.hir",
                include_str!("../../../examples/mdsite/render.hir"),
            ),
            (
                "assets.hir",
                include_str!("../../../examples/mdsite/assets.hir"),
            ),
            (
                "publish.hir",
                include_str!("../../../examples/mdsite/publish.hir"),
            ),
            (
                "model.hir",
                include_str!("../../../examples/mdsite/model.hir"),
            ),
        ],
        include_str!("../../../examples/mdsite/hirsel.json"),
    );
    check_interpreted_and_llvm_package_tests("mdsite", &example.dir.join("hirsel.json"), 3);
}

/// A process whose stdout is read line by line on a thread, killed if still running
/// when dropped.
struct Running {
    child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Running {
    fn spawn(mut command: Command, stderr: &std::path::Path) -> Self {
        use std::io::BufRead;
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::fs::File::create(stderr).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn line(&self, what: &str) -> String {
        self.lines
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap_or_else(|e| panic!("{what}: no line: {e}"))
    }

    /// Closes stdin and waits for the exit status.
    fn finish(mut self, what: &str) -> std::process::ExitStatus {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "{what}: did not exit");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// examples/tasks over HTTP: the real server, interpreted or built with LLVM, driven by
/// the interpreted Hirsel client in examples/tasks/e2e, which knows only the routes.
/// The client runs its `first` phase, holds a stream open while the server stops on
/// stdin EOF (`watch`), and runs `second` after a restart on the same port over the
/// same data directory.
/// examples/tasks built once per test process with LLVM, shared by the native tests
/// that launch the server.
fn tasks_llvm() -> &'static PathBuf {
    // Keep both the executable and its private cache alive for every consumer.
    static EXE: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
    &EXE.get_or_init(|| {
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-example-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        let exe = dir.join("tasks");
        let output = tasks_cache::compiler(&dir_scratch)
            .args(["build", "--backend", "llvm"])
            .arg(repository_manifest("examples/tasks/hirsel.json"))
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(output.status.success(), "build tasks: {output:?}");
        (dir_scratch, exe)
    })
    .1
}

fn tasks_end_to_end(native: bool) {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-tasks-e2e-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let server = repository_manifest("examples/tasks/hirsel.json");
    let client = repository_manifest("examples/tasks/e2e/hirsel.json");
    // The server is built once and launched directly; the client always runs
    // interpreted, since natively it would build std's HTTP client too.
    let built: Option<PathBuf> = native.then(|| tasks_llvm().clone());
    let launch =
        |manifest: &PathBuf, exe: Option<&PathBuf>, grants: &serde_json::Value, args: &[&str]| {
            let path = dir.join(format!(
                "grants-{}.json",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, grants.to_string()).unwrap();
            let mut command = match exe {
                Some(exe) => {
                    let mut command = Command::new(exe);
                    command.arg("--hirsel-grants").arg(&path);
                    command
                }
                None => {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                    command
                        .arg("run")
                        .arg(manifest)
                        .arg("--grants")
                        .arg(&path)
                        .arg("--");
                    command
                }
            };
            command.args(args);
            command
        };
    // A free port; the restarted server binds it again at once.
    let port = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string();
    let start = |port: &str| {
        // The monotonic clock times requests; without the wall clock log lines carry
        // no time.
        let grants = serde_json::json!({
            "io": {
                "stdout": "stdout.bytes@1",
                "stderr": "stderr.bytes@1",
                "stdin": "stdin.bytes@1",
                "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1",
                "dirs": [
                    {"path": "data", "host": data.canonicalize().unwrap(), "access": "read_write"},
                    {"path": "static", "host": repository_manifest("examples/tasks/static"), "access": "read"},
                ],
                "net": {"provider": "net.stream@1", "rules": [{"address": "127.0.0.1", "service": port, "operation": "listen"}]},
            },
            "args": [],
            "env": {"TASKS_HOST": null, "TASKS_PORT": null, "TASKS_DURABILITY": null, "TASKS_LOG_LEVEL": null},
        });
        let command = launch(&server, built.as_ref(), &grants, &["--port", port]);
        let running = Running::spawn(command, &dir.join(format!("server-{port}.log")));
        assert_eq!(
            running.line("server readiness"),
            format!("listening on http://127.0.0.1:{port}/"),
            "native={native}"
        );
        running
    };
    let client_command = |phase: &str, port: &str| {
        let grants = serde_json::json!({
            "io": {
                "stdout": "stdout.bytes@1",
                "dirs": [],
                "net": {"provider": "net.stream@1", "rules": [{"address": "127.0.0.1", "service": port, "operation": "connect"}]},
            },
            "args": [],
        });
        launch(&client, None, &grants, &[phase, port])
    };
    let run_client = |phase: &str, port: &str, expected: &str| {
        let output = client_command(phase, port).output().unwrap();
        assert!(
            output.status.success(),
            "{phase} native={native}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "native={native}"
        );
    };
    let snapshots = || {
        let mut names: Vec<String> = std::fs::read_dir(&data)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    let first = start(&port);
    run_client("first", &port, "ok: 96 checks\n");
    let watching = Running::spawn(client_command("watch", &port), &dir.join("watch.log"));
    assert_eq!(watching.line("watch"), "watching");
    assert!(first.finish("first server").success(), "native={native}");
    assert_eq!(watching.line("watch"), "ended");
    assert_eq!(watching.line("watch"), "ok: 5 checks");
    assert!(watching.finish("watch").success(), "native={native}");
    // Each of the first run's seven changes saved a snapshot; all but two were removed.
    assert_eq!(
        snapshots(),
        [
            "board-00000000000000000006.json",
            "board-00000000000000000007.json"
        ]
    );
    let second = start(&port);
    run_client("second", &port, "ok: 14 checks\n");
    assert!(second.finish("second server").success(), "native={native}");
    assert_eq!(
        snapshots(),
        [
            "board-00000000000000000007.json",
            "board-00000000000000000008.json"
        ]
    );
}

#[test]
fn tasks_end_to_end_interpreted() {
    tasks_end_to_end(false);
}

#[test]
fn tasks_end_to_end_llvm() {
    tasks_end_to_end(true);
}

/// examples/tasks/e2e uses std's HTTP client throughout, and its `first_run` holds
/// two open streams across about a hundred checks. Each check's early releases once
/// repeated the streams' finalization in its own failure cleanup, and the LLVM build
/// took five minutes and 2.35 GB (`rc_scaling`'s
/// `early_releases_share_the_cleanup_of_live_resources` gates that cause). It now
/// peaks near 1 GB; the bound is the compiler's own peak RSS.
#[test]
fn tasks_e2e_client_builds_with_llvm_in_bounded_memory() {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-tasks-e2e-build-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path();
    let output = tasks_cache::compiler(&dir_scratch)
        .args([
            "build",
            "--timings=json",
            "--mem-usage",
            "--backend",
            "llvm",
        ])
        .arg(repository_manifest("examples/tasks/e2e/hirsel.json"))
        .arg("-o")
        .arg(dir.join("client"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "build the e2e client: {stderr}");
    let report: serde_json::Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    let peak = report["memory"]["peak_rss_bytes"].as_u64().unwrap();
    assert!(
        peak < 1536 << 20,
        "the LLVM build of the e2e client peaked at {} MiB",
        peak >> 20
    );
}

/// The gRPC greeter calls its own Greeter service over a real TCP connection in the
/// interpreter and natively: a unary call, a server-streaming call and a refused call
/// whose status reaches the client. Its packages resolve from the repository.
#[test]
fn grpc_greeter_serves_and_calls_itself_over_tcp() {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-example-{}-{}-grpc-greeter",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.hir"),
        include_str!("../../../examples/grpc-greeter/main.hir"),
    )
    .unwrap();
    std::fs::write(
        dir.join("greeter.hir"),
        include_str!("../../../examples/grpc-greeter/greeter.hir"),
    )
    .unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/grpc-greeter/hirsel.json")).unwrap();
    for dependency in manifest["dependencies"].as_array_mut().unwrap() {
        let relative = dependency["manifest"]
            .as_str()
            .unwrap()
            .trim_start_matches("../../");
        let path = root.join(relative).canonicalize().unwrap();
        dependency["manifest"] = serde_json::json!(path);
    }
    std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
    let expected = "Hello, Hirsel!\nHello 1 of 3, Hirsel!\nHello 2 of 3, Hirsel!\n\
                    Hello 3 of 3, Hirsel!\nINVALID_ARGUMENT: name is required\n";
    for args in [vec![], vec!["--native", "--backend", "llvm"]] {
        // A port free now; the program listens on it and dials it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let grants = dir.join("grants.json");
        std::fs::write(
            &grants,
            include_str!("../../../examples/grpc-greeter/grants.json")
                .replace("50051", &port.to_string()),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(&args)
            .arg(dir.join("hirsel.json"))
            .arg("--grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "{args:?}"
        );
    }
}

/// The peak resident set of a running process, in bytes.
fn peak_resident(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .expect("VmHWM");
    line.split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}

/// Reads one server-sent event (up to its blank line) from `stream`, keeping what
/// follows in `pending`.
fn next_event(stream: &mut std::net::TcpStream, pending: &mut Vec<u8>) -> String {
    use std::io::Read;
    loop {
        if let Some(end) = pending.windows(2).position(|w| w == b"\n\n") {
            let event = String::from_utf8(pending[..end].to_vec()).unwrap();
            pending.drain(..end + 2);
            return event;
        }
        let mut chunk = [0u8; 16384];
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0, "the event stream closed");
        pending.extend_from_slice(&chunk[..n]);
    }
}

/// examples/tasks with many open event streams over a large board, natively: each
/// stream renders under the board's lock, which lends the board, so a stream holds no
/// copy of it while it waits. 200 streams over 2,000 tasks once held a 1 MB board copy
/// each (about 200 MiB); they must now stay within a few MiB of the server's
/// readiness, through a change that re-renders every stream.
#[test]
fn tasks_event_streams_hold_no_board_llvm() {
    use std::io::Write;
    const TASKS: u64 = 2000;
    const STREAMS: usize = 200;
    const GROWTH: u64 = 24 << 20;
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-tasks-streams-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let tasks: Vec<serde_json::Value> = (1..=TASKS)
        .map(|n| {
            serde_json::json!({
                "id": n, "title": format!("Task {n}"), "description": "Benchmark description",
                "status": "Todo", "priority": "Medium", "due": null,
                "tags": ["bench", format!("group-{}", n % 10)],
            })
        })
        .collect();
    std::fs::write(
        data.join("board-00000000000000000001.json"),
        serde_json::json!({"next_id": TASKS + 1, "revision": 1, "tasks": tasks}).to_string(),
    )
    .unwrap();
    let exe = tasks_llvm().clone();
    let port = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string();
    let grants = serde_json::json!({
        "io": {
            "stdout": "stdout.bytes@1",
            "stderr": "stderr.bytes@1",
            "stdin": "stdin.bytes@1",
            "tasks": "tasks.cooperative@1",
            "monotonic_clock": "clock.monotonic.ns@1",
            "dirs": [
                {"path": "data", "host": data.canonicalize().unwrap(), "access": "read_write"},
                {"path": "static", "host": repository_manifest("examples/tasks/static"), "access": "read"},
            ],
            "net": {"provider": "net.stream@1", "rules": [{"address": "127.0.0.1", "service": port, "operation": "listen"}]},
        },
        "args": [],
        "env": {"TASKS_HOST": null, "TASKS_PORT": null, "TASKS_DURABILITY": null, "TASKS_LOG_LEVEL": null},
    });
    let grants_path = dir.join("grants.json");
    std::fs::write(&grants_path, grants.to_string()).unwrap();
    let mut command = Command::new(&exe);
    command
        .arg("--hirsel-grants")
        .arg(&grants_path)
        .args(["--port", &port]);
    let server = Running::spawn(command, &dir.join("server.log"));
    assert_eq!(
        server.line("server readiness"),
        format!("listening on http://127.0.0.1:{port}/")
    );
    let pid = server.child.id();
    let ready = peak_resident(pid);
    // A search that matches one task, as a browser sends its signals.
    let target = "/updates?datastar=%7B%22filter%22%3A%7B%22status%22%3A%22%22%2C%22tag%22%3A%22%22%2C%22text%22%3A%22Task%201999%22%2C%22sort%22%3A%22manual%22%7D%2C%22selected%22%3A0%7D";
    let mut streams = Vec::new();
    for _ in 0..STREAMS {
        let mut stream =
            std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(60)))
            .unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n"
        )
        .unwrap();
        // The response head ends with a blank line too.
        let mut pending = Vec::new();
        let head = loop {
            if let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8(pending[..end].to_vec()).unwrap();
                pending.drain(..end + 4);
                break head;
            }
            let mut chunk = [0u8; 16384];
            let n = std::io::Read::read(&mut stream, &mut chunk).unwrap();
            assert!(n > 0, "no response head");
            pending.extend_from_slice(&chunk[..n]);
        };
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let event = next_event(&mut stream, &mut pending);
        assert!(event.contains("Task 1999"), "{event}");
        streams.push((stream, pending));
    }
    let opened = peak_resident(pid);
    let mut change =
        std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
    write!(
        change,
        "POST /tasks/1/advance HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    std::io::Read::read_to_string(&mut change, &mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 204"), "{reply}");
    for (stream, pending) in &mut streams {
        let event = next_event(stream, pending);
        assert!(event.contains("Task 1999"), "{event}");
    }
    let changed = peak_resident(pid);
    assert!(
        opened - ready < GROWTH && changed - ready < GROWTH,
        "{STREAMS} streams grew the peak resident set from {ready} to {opened} bytes, \
         and the change to {changed}"
    );
    drop(streams);
    assert!(server.finish("server").success());
}

/// examples/ledger-service (docs/replay-a-service.md): a store at a `sim@1` place, its
/// client's calls under a crash before a commit and a lost reply, every tape replayed
/// alone and the set joined, then the trading calls trialled against a fork of the store
/// under the candidate revision whose transfer charges a fee.
#[test]
fn ledger_service_survives_faults_replays_and_trials_a_changed_transfer() {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-ledger-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    std::fs::create_dir_all(dir.join("db")).unwrap();
    let example = repository_manifest("examples/ledger-service");
    for template in ["grants.json", "faults.json", "fork.json"] {
        std::fs::copy(example.join(template), dir.join(template)).unwrap();
    }
    let manifest = example.join("hirsel.json");
    let hirsel = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap()
    };
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let run = |grants: &str, tape: Option<&std::path::Path>| {
        let grants = dir.join(grants);
        let mut args = vec![
            "run".as_ref(),
            manifest.as_os_str(),
            "--grants".as_ref(),
            grants.as_os_str(),
        ];
        if let Some(tape) = tape {
            args.extend(["--record".as_ref(), tape.as_os_str()]);
        }
        hirsel(&args)
    };
    let opened = run("grants.json", None);
    assert!(opened.status.success(), "{}", text(&opened.stderr));
    assert_eq!(
        text(&opened.stdout),
        "1: opened 1 with 500\n2: opened 2 with 100\n"
    );
    let copy = Command::new("cp")
        .arg("-r")
        .arg(dir.join("db"))
        .arg(dir.join("fork"))
        .status()
        .unwrap();
    assert!(copy.success());
    let tape = dir.join("T");
    let traded = run("faults.json", Some(&tape));
    assert!(traded.status.success(), "{}", text(&traded.stderr));
    // Position 3's first attempt crashed before its commit, which ran again; position
    // 6's commit happened and its reply was lost, so the re-send read its outcome.
    let trades = "3: in doubt, sending it again\n3: moved 200 from 1 to 2, fee 0\n\
         4: opened 3 with 0\n5: moved 50 from 2 to 3, fee 0\n\
         6: in doubt, sending it again\n6: moved 250 from 2 to 1, fee 0\n\
         7: insufficient: 550\n";
    assert_eq!(text(&traded.stdout), trades);
    let call = |n: u32| dir.join(format!("T.call-{n}"));
    let crashed = hirsel_rt::launch::read_recording_text(call(0)).unwrap();
    assert!(crashed.contains("\nstopped -> ready\n"), "{crashed}");
    let replay = |tape: &std::path::Path| {
        hirsel(&[
            "run".as_ref(),
            manifest.as_os_str(),
            "--replay".as_ref(),
            tape.as_os_str(),
        ])
    };
    let caller = replay(&tape);
    assert!(caller.status.success(), "{}", text(&caller.stderr));
    assert_eq!(text(&caller.stdout), trades);
    // The crashed callee replays to where its place stopped it.
    let stopped = replay(&call(0));
    assert_eq!(stopped.status.code(), Some(6), "{}", text(&stopped.stderr));
    assert!(
        text(&stopped.stderr).contains("its place stopped the run"),
        "{}",
        text(&stopped.stderr)
    );
    // The callee whose reply was lost committed, and replays to its outcome.
    let lost = replay(&call(4));
    assert!(lost.status.success(), "{}", text(&lost.stderr));
    let mut tapes = vec!["tapes".as_ref(), "check".as_ref(), tape.as_os_str()];
    let calls: Vec<_> = (0..7).map(call).collect();
    tapes.extend(calls.iter().map(|c| c.as_os_str()));
    let checked = hirsel(&tapes);
    assert!(checked.status.success(), "{}", text(&checked.stderr));
    assert!(
        text(&checked.stdout).ends_with("tapes: 7 links checked, 0 unchecked\n"),
        "{}",
        text(&checked.stdout)
    );
    let candidate = example.join("trial/hirsel.json");
    let fork = dir.join("fork.json");
    let trial = hirsel(&[
        "tapes".as_ref(),
        "trial".as_ref(),
        candidate.as_os_str(),
        "--grants".as_ref(),
        fork.as_os_str(),
        tape.as_os_str(),
    ]);
    let report = text(&trial.stdout);
    assert_eq!(
        trial.status.code(),
        Some(1),
        "{report}{}",
        text(&trial.stderr)
    );
    for line in [
        "call 0 ledger_service@1/transfer (changed code): unrecorded\n  recorded InDoubt(Abandoned())\n",
        "call 1 ledger_service@1/transfer (changed code): DIVERGED\n  recorded Ok(Receipt(from: 1, to: 2, moved: 200, fee: 0))\n  trial    Ok(Receipt(from: 1, to: 2, moved: 200, fee: 2))\n",
        "call 2 ledger_service@1/open_account (same code): same\n",
        "call 3 ledger_service@1/transfer (changed code): same\n",
        "call 6 ledger_service@1/transfer (changed code): DIVERGED\n  recorded Failed(Call(Tx(Aborted(Insufficient(550)))))\n  trial    Failed(Call(Tx(Aborted(Insufficient(298)))))\n",
    ] {
        assert!(report.contains(line), "{line}\n{report}");
    }
    assert!(
        report.ends_with("trial: 7 calls, first divergence at call 1\n"),
        "{report}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// Each registration executes its own package tests on all three engines, then
// checks real host inputs and exact stdout/stderr/terminal outcomes on replay.
fn dogfood6_example(
    name: &str,
    sources: &[(&str, &str)],
    manifest: &str,
    grants: &str,
    run: Option<&str>,
    count: u64,
) {
    let example = Example::new(name, sources, manifest);
    std::fs::write(example.dir.join("grants.json"), grants).unwrap();
    if let Some(run) = run {
        let mut launch: serde_json::Value = serde_json::from_str(run).unwrap();
        let package: serde_json::Value =
            serde_json::from_slice(&std::fs::read(example.dir.join("hirsel.json")).unwrap())
                .unwrap();
        launch["dependencies"] = package["dependencies"].clone();
        std::fs::write(example.dir.join("run.json"), launch.to_string()).unwrap();
    }
    check_package_tests(name, &example.dir.join("hirsel.json"), count);
    let script = example.dir.join("verify.py");
    std::fs::write(
        &script,
        include_str!("../../../examples/verify-dogfood6.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(script)
        .args([
            "--hirsel",
            env!("CARGO_BIN_EXE_hirsel"),
            "--tool",
            name,
            "--root",
        ])
        .arg(&example.dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dogfood6_zipinspect() {
    dogfood6_example(
        "zipinspect",
        &[(
            "main.hir",
            include_str!("../../../examples/zipinspect/main.hir"),
        )],
        include_str!("../../../examples/zipinspect/hirsel.json"),
        include_str!("../../../examples/zipinspect/grants.json"),
        None,
        3,
    );
}

#[test]
fn dogfood6_snapshift() {
    dogfood6_example(
        "snapshift",
        &[
            (
                "main.hir",
                include_str!("../../../examples/snapshift/main.hir"),
            ),
            (
                "driver.hir",
                include_str!("../../../examples/snapshift/driver.hir"),
            ),
        ],
        include_str!("../../../examples/snapshift/hirsel.json"),
        include_str!("../../../examples/snapshift/grants.json"),
        Some(include_str!("../../../examples/snapshift/run.json")),
        3,
    );
}

#[test]
fn dogfood6_httpmirror() {
    dogfood6_example(
        "httpmirror",
        &[(
            "main.hir",
            include_str!("../../../examples/httpmirror/main.hir"),
        )],
        include_str!("../../../examples/httpmirror/hirsel.json"),
        include_str!("../../../examples/httpmirror/grants.json"),
        None,
        4,
    );
}

#[test]
fn dogfood6_logdigest() {
    dogfood6_example(
        "logdigest",
        &[(
            "main.hir",
            include_str!("../../../examples/logdigest/main.hir"),
        )],
        include_str!("../../../examples/logdigest/hirsel.json"),
        include_str!("../../../examples/logdigest/grants.json"),
        None,
        3,
    );
}

#[path = "support/native_argv_recipe_6.rs"]
mod native_argv_recipe_6;

#[test]
fn binary_pipe_recipe() {
    let example = Example::new(
        "binary-pipe",
        &[
            (
                "main.hir",
                include_str!("../../../examples/binary-pipe/main.hir"),
            ),
            (
                "scanner_test.hir",
                include_str!("../../../examples/binary-pipe/scanner_test.hir"),
            ),
        ],
        include_str!("../../../examples/binary-pipe/hirsel.json"),
    );
    check_package_tests("binary-pipe", &example.dir.join("hirsel.json"), 5);
    std::fs::write(
        example.dir.join("grants.json"),
        include_str!("../../../examples/binary-pipe/grants.json"),
    )
    .unwrap();
    let script = example.dir.join("verify.py");
    std::fs::write(
        &script,
        include_str!("../../../examples/binary-pipe/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(script)
        .args(["--hirsel", env!("CARGO_BIN_EXE_hirsel"), "--root"])
        .arg(&example.dir)
        .arg("--output")
        .arg(example.dir.join("evidence"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

#[path = "support/wc2_unique_row_contract_21/modules.rs"]
mod wc2_unique_row_contract_21;
