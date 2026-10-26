hirsel_guard::install!();

use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
include!("modules.rs");

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn verify_cache_recomputes_disk_hits_without_changing_diagnostics() {
    let source = file(
        "fn callee(x: u8) -> u8 { x + 1u8 }\npub fn main() -> Result[(), ()] { let _ = callee(1u8); Ok(()) }\n",
    );
    let store = source.with_extension("results");
    let run = |verify: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .env("HIRSEL_DAEMON", "auto")
            .env("HIRSEL_RESULTS", "dir@1")
            .env("HIRSEL_RESULTS_DIR", &store)
            .env_remove("HIRSEL_VERIFY_CACHE")
            .args(["check", "--json", "--timings=json"]);
        if verify {
            command.arg("--verify-cache");
        }
        let output = command.arg(&source).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        (output.stdout, report)
    };
    let (cold, cold_counts) = run(false);
    let (warm, warm_counts) = run(false);
    let (verified, verified_counts) = run(true);
    assert_eq!(cold, warm);
    assert_eq!(cold, verified);
    assert!(cold_counts["counts"]["definitions"].as_u64().unwrap() > 0);
    assert!(
        warm_counts["counts"]["store hits"].as_u64().unwrap() > 0,
        "{warm_counts}"
    );
    assert_eq!(warm_counts["counts"]["definitions"], 0, "{warm_counts}");
    assert!(
        verified_counts["counts"]["definitions"].as_u64().unwrap() > 0,
        "{verified_counts}"
    );
    let invalid = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["fmt", "--verify-cache"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--verify-cache applies only"));
    let _ = std::fs::remove_dir_all(store);
    let _ = std::fs::remove_file(source);
}
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
            b"usage: hirsel [--verbose] [--timings[=json]] [--mem-usage] check [--verify-cache] [--watch] [--json | --safe] <source|manifest.json> | serve [--stop] [--workspace <dir>] [--idle <seconds>] | test [--no-verify] [--json] [--seed <u64>] [--scheduler <name@1:seed> | --explore <n> [--scheduler <name@1>]] [--race] [--sim-points <root-release,atomic-store>] [--bless | --bench [--bench-time <ms>]] [--max-steps <n|unbounded> | --native [--backend llvm|cranelift]] [--trace-ownership <dir>] <source|manifest.json> [filter...] | run [--verify-cache] [--validate-stages] [--no-verify] [--json] [--native [--backend llvm|cranelift] [--proofs <sidecar> [--trust <tool@version>]...] | --simulation <json>] <source|manifest.json> [(--run <json> | --grants <json>) [--record <tape>] [--revocations <json>] | --replay <tape>] [--max-steps <n|unbounded>] [--cores <n>] [--trace-events <jsonl>] [--trace-ownership <json>] [-- args...] | build [--verify-cache] [--validate-stages] [--no-verify] [--release | --debug] [--lto] [--backend llvm|cranelift] [--fuel] [--target x86_64-linux | aarch64-linux] [--proofs <sidecar> [--trust <tool@version>]...] <source|manifest.json> -o <exe> | new <dir> [--lib] | fmt [--check] <source|manifest.json> | expand <source|manifest.json> | hash <source|manifest.json> | export --tir|--run|--c-header|--native-abi <source|manifest.json> -o <file> | deps card <manifest.json> [--package name@ver[/module]] [--code-store config.json] [--json] | explain RULE [--json] | explain --list [--json] (docs/rule-explain.md) | doc <source|manifest.json> [-o <out.md>] [--json] | view <tape> --capture <id> -o <file> [--perfetto | --source <program> --analysis <id>] [--timing <sidecar>] | precompute <package manifest.json> -o <directory> | tapes check <tape>... | tapes trial <candidate manifest.json> --grants <json> [--from <n>] <tape> | ctl <socket> (cores <n> | metrics) | serve [--no-verify] <source|manifest.json> --host <json>\n",
            "{command}"
        );
        assert!(output.stderr.is_empty(), "{command}");
    }
}

/// Missing ordinary products preserve output and emit no diagnostics.
#[test]
fn missing_products_preserve_output_with_verbose() {
    let lib = file("pub fn value() -> u64 {\n  1\n}\n");
    let lib_manifest = lib.with_extension("json");
    std::fs::write(&lib_manifest, serde_json::json!({"schema":1,"package":{"name":"lib","version":"1"},"sources":[{"path":lib.file_name().unwrap().to_str().unwrap(),"module":["lib"]}],"dependencies":[]}).to_string()).unwrap();
    let path = file(
        "pub fn main() -> Result[(), ()] {\n  failure.check(lib.value() == 1, Arithmetic())\n  Ok(())\n}\n",
    );
    std::fs::write(path.with_extension("hir.json"), serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[{"manifest":lib_manifest,"package":{"name":"lib","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    for verbose in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        if verbose {
            command.arg("--verbose");
        }
        let output = command
            .arg("run")
            .arg(&path)
            .env("HIRSEL_PRODUCTS", "/nonexistent/hirsel-std.products")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

/// The toolchain products supply declarations and signatures as well as TIR.
/// Counting parses catches accidentally routing an ordinary std call through
/// source even when its final checked products still agree.
#[test]
fn checking_hello_parses_no_std_sources() {
    let path = std_file(
        "pub fn main(rt: Io) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, std@1/u64.to_string(42u64))\n  Ok(())\n}\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["--timings=json", "--verbose", "check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "matching products emit no diagnostic: {stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(report["counts"]["dependency parses"], 0, "{report}");
}

fn file(text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hirsel-cli-{}-{}.hir",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).unwrap();
    path
}
#[test]
fn a_run_past_its_memory_limit_ends_with_the_guard_message() {
    // Compiling and running even an empty main needs more than 64M (the
    // compiler's stack alone is 128M), so the run must stop at the limit.
    let path = file("pub fn main() -> Result[(), ()] {\n  Ok(())\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .env("HIRSEL_MEMORY_LIMIT", "64M")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(hirsel_guard::EXIT_STATUS),
        "{stderr}"
    );
    assert!(
        stderr.starts_with("hirsel: memory limit 64M reached (HIRSEL_MEMORY_LIMIT); allocating "),
        "{stderr}"
    );
}
fn std_file(text: &str) -> std::path::PathBuf {
    let path = file(text);
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let sidecar = format!("{}.json", path.display());
    std::fs::write(sidecar, format!(r#"{{"schema":1,"package":{{"name":"standalone","version":"0"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[{{"manifest":"{}","package":{{"name":"std","version":"1"}},"expose":[]}}]}}"#, path.display(), manifest.display())).unwrap();

    path
}
#[test]
fn expand_prints_generated_bodies_in_source_order_and_rejected_source_exits_one() {
    let generator = "fn gen() -> Result[Body[fn(u64) -> u64 uses {}], String] {\n  Ok(codegen.body(\"fn(x: u64) -> u64 uses {} {\\n  x + 1\\n}\"))\n}\n";
    let path = file(&format!(
        "{generator}fn second(n: u64) -> u64 uses {{}} = comptime gen()\nfn first(n: u64) -> u64 uses {{}} = comptime gen()\n"
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("expand")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "// generated body of standalone@0/second\nfn(x: u64) -> u64 uses {} {\n  x + 1\n}\n\n\
         // generated body of standalone@0/first\nfn(x: u64) -> u64 uses {} {\n  x + 1\n}\n"
    );
    let bad = file(&format!(
        "{generator}fn wrong(n: u64) -> u32 uses {{}} = comptime gen()\n"
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("expand")
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("error[LANG-7]"));
}
#[test]
fn hash_prints_definition_hashes_that_renames_keep_and_rejected_source_exits_one() {
    let hash = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("hash")
            .arg(file(source))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let source = "fn add(x: u64) -> u64 { let y = x + 1u64; y }\npub fn main() -> Result[(), ()] { let n = add(1u64); Ok(()) }\n";
    let text = hash(source);
    let lines: Vec<_> = text.lines().map(|l| l.split_once(' ').unwrap()).collect();
    assert_eq!(
        lines.iter().map(|(_, path)| *path).collect::<Vec<_>>(),
        ["standalone@0/add", "standalone@0/main"]
    );
    assert!(lines.iter().all(|(hash, _)| hash.len() == 64));
    let renamed = hash(&source.replace("add", "plus").replace('y', "z"));
    assert_eq!(
        renamed,
        text.replace("standalone@0/add", "standalone@0/plus")
            .lines()
            .rev()
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("hash")
        .arg(file("fn bad() -> u64 { true }\n"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
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
        text.starts_with("(Export\n  (Header \"hirsel.tir.export.v15\" 15\n"),
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
    let missing_scratch = tempfile::tempdir().unwrap();
    let missing = missing_scratch.path().join("missing.hir");
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
        vec!["run", "--json", "--json", "file.hir"],
        vec!["run", "--simulation", "config.json"],
        vec!["run", "--native", "file.hir", "extra"],
        vec!["fmt", "--json", "file.hir"],
        vec!["fmt", "--check", "-file.hir"],
        vec!["export", "--tir", "file.hir"],
        vec!["export", "--tir", "file.hir", "--output", "out.tir"],
        vec!["test", "--scheduler", "random@1", "m.json"],
        vec!["test", "--scheduler", "chaos@1:3", "m.json"],
        vec!["test", "--explore", "0", "m.json"],
        vec![
            "test",
            "--explore",
            "3",
            "--scheduler",
            "random@1:4",
            "m.json",
        ],
        vec!["test", "--explore", "3", "--scheduler", "fifo@1", "m.json"],
        vec!["test", "--bench", "--explore", "3", "m.json"],
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
    assert_eq!(output.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("expected a public, complete, monomorphic Hirsel function")
    );
    std::fs::remove_file(path).unwrap();
    // A program without its entry checks, and running it names the entry.
    let path = file("fn probe() -> u64 {\n  1\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
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
    let path = file(include_str!("../fixtures/opening.hir"));
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
    assert_eq!(output.status.code(), Some(3));
    // 4e992dc7 accepts text or byte arrays and identifies the invalid file field.
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("files.a: expected text or an array of bytes")
    );
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn unavailable_execution_preserves_stdout_before_the_tool_error() {
    let source = file(
        "fn charge[T: copyable](x: T) -> u8 { match arena.bounded(64, fn(a: Arena) -> u8 { let c = copy_to(a, x); 0u8 }) { Ok(n) => n, Error(e) => 1u8 } }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let n = charge(7u8); Ok(()) }",
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
        Some(3),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"before unavailable\n");
    // Detailed interpreter refusals are preserved since a51a3c77.
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("interpretation unavailable: a bounded-arena charge of a generic type"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
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
    assert_eq!(result.status.code(), Some(3));
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
/// PS-12/PS-13 through the binary: a trusted (here deliberately false)
/// verdict for `bump`'s `ensures` removes that check from the native program
/// only, so the interpreter still fails FACT-1 and the native run does not.
#[test]
fn proofs_remove_their_checks_from_native_builds_only() {
    let source = file(
        "fn bump(x: u32, y: u32) -> u32\n  ensures result > x\n{\n  x + y\n}\npub fn main() -> Result[(), ()] {\n  let n = bump(1u32, 0u32)\n  Ok(())\n}\n",
    );
    let mut loaded = hirsel_driver::load_program(&source).unwrap();
    loaded.check_for_execution().unwrap();
    let text = loaded.engine.export_tir(&loaded.entry).unwrap();
    let export = loaded.engine.import_tir(&text).unwrap();
    let hash = |h: [u8; 32]| {
        format!(
            "#{}",
            h.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    };
    let id = "standalone@0/bump#FACT-1/ensures/0";
    let obligation = export
        .definitions
        .iter()
        .flat_map(|d| &d.obligations)
        .find(|o| o.id.to_string() == id)
        .unwrap();
    let sidecar = source.with_extension("proofs.json");
    std::fs::write(
        &sidecar,
        serde_json::json!({"format": "hirsel.proof.sidecar.v1", "records": [{
            "revision": hash(export.header.program_revision.unwrap().0),
            "obligation": id,
            "checksum": hash(obligation.checksum.0),
            "checker": "liar@1",
            "budget": {"unit": "steps", "count": 1},
            "verdict": "proved",
        }]})
        .to_string(),
    )
    .unwrap();
    let hirsel = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap()
    };
    let (src, proofs) = (source.as_os_str(), sidecar.as_os_str());
    let report = format!(
        "hirsel: proof {id}: removed\nhirsel: proofs: 1 removed, 0 kept, 1 without evidence\n"
    );
    for args in [vec!["run"], vec!["run", "--native"]] {
        let mut args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
        args.push(src);
        let output = hirsel(&args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("FACT-1"),
            "{output:?}"
        );
    }
    let output = hirsel(&[
        "run".as_ref(),
        "--native".as_ref(),
        "--proofs".as_ref(),
        proofs,
        "--trust".as_ref(),
        "liar@1".as_ref(),
        src,
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), report);
    let exe = source.with_extension("proofs-exe");
    let output = hirsel(&[
        "build".as_ref(),
        "--proofs".as_ref(),
        proofs,
        "--trust".as_ref(),
        "liar@1".as_ref(),
        src,
        "-o".as_ref(),
        exe.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), report);
    assert_eq!(Command::new(&exe).output().unwrap().status.code(), Some(0));
    // Untrusted, the same evidence keeps the check.
    let output = hirsel(&[
        "run".as_ref(),
        "--native".as_ref(),
        "--proofs".as_ref(),
        proofs,
        src,
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("kept (unreplayable: this build trusts no checker liar@1)"),
        "{stderr}"
    );
    for args in [
        vec!["run", "--proofs", "p.json", "file.hir"],
        vec!["run", "--native", "--trust", "a@1", "file.hir"],
        vec![
            "run", "--native", "--proofs", "p.json", "--trust", "a", "file.hir",
        ],
        vec!["build", "--trust", "a@1", "file.hir", "-o", "exe"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
    for path in [source, sidecar, exe] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn native_run_matches_the_reference_run() {
    let run = |native: bool, path: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if native {
            command.arg("--native");
        }
        command
            .arg(path)
            .arg("--grants")
            .arg(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[],"net":{"provider":"net.stream@1","rules":[]}},"args":[]}"#)
            .output()
            .unwrap()
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
    let decimal = std_file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let text = std@1/u64.to_string(4u64)\n  let _ = out.print(io.stdout(rt)?, if text == \"4\" { \"ok\\n\" } else { \"bad\\n\" })\n  Ok(())\n}\n",
    );
    let directory = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let dir = io.dir(rt, path.of(\".\"))\n  Ok(())\n}\n",
    );
    for path in [&prints, &fails, &rejected, &map, &decimal, &directory] {
        let (reference, native) = (run(false, path), run(true, path));
        assert_eq!(
            native.status.code(),
            reference.status.code(),
            "{path:?}: native stderr: {}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(native.stdout, reference.stdout, "{path:?}");
        assert_eq!(
            String::from_utf8_lossy(&native.stderr),
            String::from_utf8_lossy(&reference.stderr),
            "{path:?}"
        );
    }
    assert_eq!(run(true, &prints).stdout, b"hi\n");
    assert_eq!(run(true, &fails).status.code(), Some(2));
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
            assert_eq!(
                output.stderr,
                if granted {
                    b"".as_slice()
                } else {
                    b"error: NotGranted\n"
                }
            );
            std::fs::remove_file(config).unwrap();
        }
        std::fs::remove_file(source).unwrap();
    }
}

/// A run without a record admits the empty-grant record (determinism.md, Run
/// configuration) in every engine: an Io main is refused at launch and never reaches
/// its directory or stdout, and the refusal names the flags that grant authority.
#[test]
fn bare_run_refuses_an_io_main_identically_in_every_engine() {
    let source = file(
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), String] {
  let _ = match io.dir(rt, path.of("")) { Ok(d) => file.write_bytes(d, path.of("granted"), b""), Error(_) => Ok(()) }
  let output = match io.stdout(rt) { Ok(s) => s, Error(_) => return Error("stdout") }
  let _ = out.print(output, "granted")
  Ok(())
}
"#,
    );
    let engines: [&[&str]; 3] = [&[], &["--native"], &["--native", "--backend", "cranelift"]];
    for flags in engines {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(flags)
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3), "{flags:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{flags:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "hirsel: launch: {\"kind\":\"not_granted\",\"root\":\"Io\"}\n\
             hirsel: the entry takes `Io`, which this run does not grant: grant it with \
             `--grants <grants.json>`, or run interpreted in a simulated world with \
             `--simulation <world.json>`\n\
             hirsel: grant the entry's requested roots with --grants '{\"args\":[],\"io\":{\"dirs\":[],\"stdout\":\"stdout.bytes@1\"}}'\n",
            "{flags:?}"
        );
    }
    std::fs::remove_file(source).unwrap();
}

/// A Test entry uses test admission even when selected by the ordinary run command.
#[test]
fn bare_run_executes_a_test_main_in_every_engine() {
    let source = file("pub fn main(t: Test) -> Result[(), ()] { test.check(t, true)\n Ok(()) }");
    let record = run_record(&source, serde_json::json!({}));
    let exported: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(exported["launch"], "test");
    let engines: [&[&str]; 3] = [&[], &["--native"], &["--native", "--backend", "cranelift"]];
    for flags in engines {
        for config in [
            vec![],
            vec!["--grants".as_ref(), "{}".as_ref()],
            vec!["--run".as_ref(), record.as_os_str()],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .args(flags)
                .arg(&source)
                .args(config)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{flags:?}: {output:?}");
            assert!(output.stdout.is_empty(), "{flags:?}: {output:?}");
            assert!(output.stderr.is_empty(), "{flags:?}: {output:?}");
        }
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
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

/// `hirsel run <source> --run <record>`, compiled when `native`.
fn host_run(source: &std::path::Path, record: &std::path::Path, native: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.arg("run").arg(source).arg("--run").arg(record);
    if native {
        command.arg("--native");
    }
    command
}

#[test]
fn hello_web_concurrent_serves_while_another_request_is_incomplete() {
    hello_web(false, true, false, false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_answers_real_requests_native() {
    hello_web(true, false, false, false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_concurrent_serves_while_another_request_is_incomplete_native() {
    hello_web(true, true, false, false);
}

#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_concurrent_keeps_connections_alive_cranelift() {
    hello_web(true, true, true, false);
}

#[test]
fn hello_web_concurrent_request_read_deadline() {
    hello_web(false, true, false, true);
}
#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_concurrent_request_read_deadline_llvm() {
    hello_web(true, true, false, true);
}
#[test]
#[ignore = "full gate"]
fn full_gate_hello_web_concurrent_request_read_deadline_cranelift() {
    hello_web(true, true, true, true);
}

fn hello_web(native: bool, concurrent: bool, cranelift: bool, deadline: bool) {
    use hello_web_readiness_8::{Server, replace_once};
    use std::io::{Read, Write};
    use std::time::Duration;
    {
        let original = if concurrent {
            include_str!("../../../../examples/hello-web-concurrent/main.hir")
        } else {
            include_str!("../../../../examples/hello-web/main.hir")
        };
        // The existing examples own the listener, bind port zero, and publish
        // the actual local address (Go httptest server.go:215–231,319–346).
        let source = replace_once(original, "8080u16", "0u16");
        let source = replace_once(
            &source,
            if concurrent {
                "// Accept connections until stopped.\n      while true"
            } else {
                "// Accept connections until stopped.\n  while true"
            },
            r#"
let bound = net.local_addr(n, LISTENER) ? Network
let output = io.stdout(rt) ? Grant
let _ = out.print(output, std@1/u16.to_string(bound.port))
let _ = out.print(output, "\n")
for i in 0u64..COUNTu64
"#,
        );
        let source = replace_once(
            &source,
            "LISTENER",
            if concurrent { "accepting" } else { "listener" },
        );
        let source = replace_once(
            &source,
            "COUNTu64",
            if deadline {
                "1u64"
            } else if concurrent {
                "6u64"
            } else {
                "3u64"
            },
        );
        // Successful exchanges (including the held partial request) have no clock
        // budget. Only the timeout case arms a deadline, with a peer that never finishes.
        let source = if concurrent {
            replace_once(
                &source,
                "config.read_timeout = Some(duration.from_parts(30, 0))",
                if deadline {
                    "config.read_timeout = Some(duration.from_parts(0, 500000000))"
                } else {
                    "config.read_timeout = None()"
                },
            )
        } else {
            source
        };
        let source = if concurrent && !deadline {
            replace_once(
                &source,
                "let mut config = http.default_config()",
                "let mut config = http.default_config()\n  config.idle_timeout = None()",
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
        // Both examples project Tasks. Only the concurrent entry projects Clock.
        let record = run_record(&path, hello_web_readiness_8::grants(concurrent));
        let tape = path.with_extension("tape");
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&path).arg("--run").arg(&record);
        if deadline {
            command.arg("--record").arg(&tape);
        }
        if native {
            command.arg("--native");
            if cranelift {
                command.args(["--backend", "cranelift"]);
            }
        }
        let mut server = Server::spawn(&mut command).unwrap_or_else(|e| {
            panic!("server startup native={native} cranelift={cranelift}: {e}")
        });
        let port = server.ready(Duration::from_secs(90)).unwrap_or_else(|e| {
            panic!("server startup native={native} cranelift={cranelift}: {e}")
        });
        if deadline {
            let mut stalled =
                std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
            stalled
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            // Send nothing: even the initial bytes must not race the server's deadline.
            let mut response = vec![];
            stalled.read_to_end(&mut response).unwrap();
            assert_eq!(
                response,
                b"HTTP/1.1 408 \r\nContent-Length: 15\r\nConnection: close\r\n\r\nRequest Timeout"
            );
        } else {
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
            if concurrent {
                let keep = b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: keep-alive\r\n\r\nHello, world";
                let close = b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world";
                // Go TestCloseNotifierPipelined: both requests arrive before the first response.
                let mut pipeline =
                    std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
                pipeline
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                pipeline.write_all(b"POST /one HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\nabcGET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
                let mut responses = vec![];
                pipeline.read_to_end(&mut responses).unwrap();
                assert_eq!(responses, [keep.as_slice(), close.as_slice()].concat());
                // Go testTCPConnectionStaysOpen: drain each body, then reuse the socket.
                let mut sequential =
                    std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
                sequential
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                sequential
                    .write_all(b"GET /one HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .unwrap();
                let mut first = vec![0; keep.len()];
                sequential.read_exact(&mut first).unwrap();
                assert_eq!(first, keep);
                sequential
                    .write_all(b"GET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .unwrap();
                let mut second = vec![];
                sequential.read_to_end(&mut second).unwrap();
                assert_eq!(second, close);
            }
            for (request,expected) in [
            (&b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"[..],&b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world"[..]),
            (b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 3\r\n\r\nabc",b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world"),
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
        }
        server
            .finish()
            .unwrap_or_else(|e| panic!("server completion native={native}: {e}"));
        if deadline {
            let mut replay = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            replay.arg("run").arg(&path).arg("--replay").arg(&tape);
            if native {
                replay.arg("--native");
            }
            if cranelift {
                replay.args(["--backend", "cranelift"]);
            }
            let replayed = replay.output().unwrap();
            assert!(
                replayed.status.success(),
                "{}",
                String::from_utf8_lossy(&replayed.stderr)
            );
            assert_eq!(replayed.stdout, format!("{port}\n").as_bytes());
            assert!(replayed.stderr.is_empty());
            std::fs::remove_file(tape).unwrap();
        }
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
            assert_eq!(
                output.stderr,
                if allowed {
                    b"".as_slice()
                } else {
                    b"error: ()\n"
                }
            );
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
            assert_eq!(output.stderr, b"error: ()\n");
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
                Some(3),
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

#[test]
fn tar_and_zip_extract_never_follow_host_symlinks_out_of_the_grant_in_both_engines() {
    let source = std_file(include_str!("../fixtures/tar-symlink-escape.hir"));
    let root = source.with_extension("root");
    let outside = source.with_extension("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
    std::os::unix::fs::symlink(outside.join("target.txt"), root.join("link.txt")).unwrap();
    for native in [false, true] {
        let record = run_record(
            &source,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":root,"access":"read_write"}]},"args":[]}),
        );
        let output = host_run(&source, &record, native).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "native={native}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"ok\n");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(std::fs::read(root.join("fine/ok.txt")).unwrap(), b"evil");
        assert_eq!(std::fs::read(root.join("fine/zip.txt")).unwrap(), b"evil");
        std::fs::remove_dir_all(root.join("fine")).unwrap();
        std::fs::remove_file(record).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
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
        "schema":1,"depth_semantics":"hirsel.depth.invocations.v2",
        "program":revision.digest().iter().map(|b|format!("{b:02x}")).collect::<String>(),
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
fn standalone_hello_runs_directly_with_fresh_grants_and_only_floor_glibc() {
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
        let readobj = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap())
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
            .filter(|line| line.contains(".so."))
            .collect();
        // Only the glibc 2.28 floor's libraries and loader (the link kit's stubs).
        let floor = [
            "ld-linux-x86-64.so.2",
            "libc.so.6",
            "libdl.so.2",
            "libm.so.6",
            "libpthread.so.0",
            "libresolv.so.2",
            "librt.so.1",
            "libutil.so.1",
        ];
        assert!(libs.contains(&"libc.so.6"), "{text}");
        assert!(libs.iter().all(|lib| floor.contains(lib)), "{text}");
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
        let dwarf = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap())
            .join("bin/llvm-dwarfdump");
        let split = dwarf::debug_file(&debug);
        let lines = Command::new(dwarf)
            .arg("--debug-line")
            .arg(&split)
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
fn x86_build_needs_no_cross_target_or_memcheck_environment() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let executable = source.with_extension("exe");
    for backend in ["llvm", "cranelift"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        for name in [
            "HIRSEL_AARCH64_CC",
            "HIRSEL_AARCH64_SYSROOT",
            "HIRSEL_QEMU_AARCH64",
            "HIRSEL_VALGRIND",
            "VALGRIND_LIB",
            "HIRSEL_VALGRIND_RUNTIME",
        ] {
            command.env_remove(name);
        }
        let output = command
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        let output = Command::new(&executable).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        std::fs::remove_file(&executable).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}

#[test]
fn admit_safe_refuses_an_entry_reaching_an_escape_in_every_launcher() {
    // CAP-6: the run-config code policy is outside the conformance header schema.
    let main = "pub fn main(u: Unsafe) -> Result[(), ()] {\n  let _ = relay(u, 2)\n  Ok(())\n}\n";
    let program = |body: &str| {
        file(&format!(
            "fn relay(u: Unsafe, a: i32) -> i32 {{\n  {body}\n}}\n\n{main}"
        ))
    };
    // Executables call C where the interpreter calls no C, so they reach libc's abs.
    let foreign = |body: &str| {
        let path = file(&format!(
            "/// Reads its argument and retains nothing.\nextern \"c\" fn abs(u: Unsafe, a: i32) -> i32\n\nfn relay(u: Unsafe, a: i32) -> i32 {{\n  {body}\n}}\n\n{main}"
        ));
        let sidecar = format!("{}.json", path.display());
        std::fs::write(&sidecar, format!(r#"{{"schema":1,"package":{{"name":"app","version":"1"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[],"unsafe_ffi":{{"link":[],"headers_checked":false,"wrappers":[]}}}}"#, path.display())).unwrap();
        path
    };
    let policy = |code: &str| {
        file(&format!(
            r#"{{"unsafe":true,"admission":{{"code":"{code}"}}}}"#
        ))
    };
    let (safe, any) = (policy("admit.safe@1"), policy("admit.any@1"));
    let interpreted = |source: &std::path::Path, grants: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(source)
            .arg("--grants")
            .arg(grants)
            .output()
            .unwrap()
    };
    let escaping = program("int.unchecked_add(u, a, 1)");
    let refused = interpreted(&escaping, &safe);
    assert_eq!(refused.status.code(), Some(3), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(r#""kind":"unsafe_code","path":["standalone@0/main","standalone@0/relay","core@1/int/unchecked_add"]"#),
        "{refused:?}"
    );
    for (source, grants) in [(&program("a + 1"), &safe), (&escaping, &any)] {
        let output = interpreted(source, grants);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    }
    let (escaping, twin) = (foreign("abs(u, a)"), foreign("a + 1"));
    for backend in ["llvm", "cranelift"] {
        let launch = |source: &std::path::Path, grants: &std::path::Path| {
            let executable = build_executable(source, &["--debug", "--backend", backend]);
            let output = Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(grants)
                .output()
                .unwrap();
            std::fs::remove_file(executable).unwrap();
            output
        };
        // The executable keeps only the verdict, so it names no path.
        let refused = launch(&escaping, &safe);
        assert_eq!(refused.status.code(), Some(3), "{backend}: {refused:?}");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(r#""kind":"unsafe_code","path":[]"#),
            "{backend}: {refused:?}"
        );
        for (source, grants) in [(&escaping, &any), (&twin, &safe)] {
            let output = launch(source, grants);
            assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        }
    }
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
        assert_eq!(output.status.code(), Some(3), "{bad}: {output:?}");
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
    let source = std_file(include_str!(
        "../../../../conformance/CAP/CAP-1-host-inputs-run.hir"
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
    assert_eq!(output.stderr, b"error: NotGranted\n");
    std::fs::remove_file(simulation).unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(format!("{}.json", source.display())).unwrap();
}

/// The corpus runs this program with empty stdin; here bufio's stdin adapters
/// share one Reader over real bytes, refilling four at a time.
#[test]
fn simulation_bufio_stdin_adapters_share_one_reader() {
    let source = std_file(include_str!(
        "../../../../conformance/CAP/CAP-1-bufio-stdin-run.hir"
    ));
    let simulation = source.with_extension("simulation.json");
    std::fs::write(
        &simulation,
        serde_json::to_vec(&serde_json::json!({
            "stdin":"one\ntwo,three\nfour",
            "projections":["stdin","stdout"]
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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line [one\n]\nuntil 4 [two,]\nexact thr\nmore 4\nread 5 [ee\nfo]\n"
    );
    std::fs::remove_file(simulation).unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(format!("{}.json", source.display())).unwrap();
}

#[test]
fn host_io_records_use_piped_stdin_distinct_stderr_and_exact_environment_snapshots() {
    use std::io::Write;
    use std::process::Stdio;
    let source = std_file(include_str!(
        "../../../../conformance/CAP/CAP-1-host-inputs-run.hir"
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
                    b"error: NotGranted\n"
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
    let source = file(include_str!(
        "../../../../conformance/CONC/CONC-2-await.hir"
    ));
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
    assert!(native.status.success(), "{native:?}");
    assert!(
        native.stdout.is_empty() && native.stderr.is_empty(),
        "{native:?}"
    );
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_tasks_accept_and_connect_as_siblings_record_and_replay_in_either_engine() {
    // Each engine records on a fresh port: the first run's closed connection may hold its own.
    for native in [false, true] {
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
        let tape = source.with_extension("tape");
        let output = host_run(&source, &record, native)
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(output.status.success(), "native={native}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        // The server's accept parked before the client connected, and the tape says so.
        let recorded = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        assert!(
            recorded.contains(" -> blocked 0\n") && recorded.contains("\nready 0 "),
            "native={native}: {recorded}"
        );
        // Replay parks where the recording parked, in either engine, with no socket.
        for engine in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source).arg("--replay").arg(&tape);
            if engine {
                command.arg("--native");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
                "recorded native={native}, replayed native={engine}: {output:?}"
            );
        }
        for path in [source, record, tape] {
            std::fs::remove_file(path).unwrap();
        }
    }
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
    for native in [false, true] {
        let output = host_run(&source, &record, native).output().unwrap();
        assert!(output.status.success(), "native={native}: {output:?}");
        assert!(output.stderr.is_empty());
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_canceled_stdin_and_stderr_have_no_effects() {
    use std::io::Write;
    use std::process::Stdio;
    let source = file(
        &include_str!(
            "../../../../conformance/CONC/CONC-3-stdin-stderr-canceled-before-effect-run.hir"
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
    for native in [false, true] {
        let mut child = host_run(&source, &record, native)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "native={native}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn real_host_detached_results_drop_before_scope_exit() {
    let source = file(include_str!(
        "../../../../conformance/CONC/CONC-2-detached-result-cleanup-run.hir"
    ));
    let record = run_record(
        &source,
        serde_json::json!({"io": {
        "tasks":"tasks.cooperative@1", "dirs":[]
    }, "args":[]}),
    );
    for native in [false, true] {
        let output = host_run(&source, &record, native).output().unwrap();
        assert!(output.status.success(), "native={native}: {output:?}");
        assert!(output.stdout.is_empty(), "native={native}");
        assert!(output.stderr.is_empty(), "native={native}");
    }
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
            Some(2),
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
    let spawn = |stdin: Stdio, native: bool| {
        let mut child = host_run(&source, &record, native)
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

    for native in [false, true] {
        // The pipe stays open and empty until the sibling has printed.
        let (reader, mut writer) = std::io::pipe().unwrap();
        let observer = reader.try_clone().unwrap();
        let (mut child, lines) = spawn(reader.into(), native);
        match lines.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(line) => assert_eq!(line, "sibling"),
            Err(e) => {
                let _ = child.kill();
                panic!("native={native}: the parked stdin read blocked its sibling: {e}");
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
        let (child, lines) = spawn(std::fs::File::open(&input).unwrap().into(), native);
        assert_eq!(lines.recv().unwrap(), "sibling");
        finish(child, lines);
        std::fs::remove_file(input).unwrap();
    }
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
    for native in [false, true] {
        let (reader, writer) = std::io::pipe().unwrap();
        let mut child = host_run(&source, &record, native)
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
                    panic!("native={native}: canceled stdin read stayed parked: {e}");
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
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(record).unwrap();
}

#[test]
fn launch_admission_conditions_and_historical_replay() {
    let source = file(
        "pub fn main(rt: Io) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"entered\\n\"); Ok(()) }\n",
    );
    let exe = build_executable(&source, &[]);
    let grants = source.with_extension("grants.json");
    let revocations = source.with_extension("revocations.json");
    let tape = source.with_extension("tape");
    // Exercise source interpretation, source native launch and the standalone launcher.
    for mode in 0..3 {
        let command = |replay: bool| {
            let mut c = if mode == 2 {
                Command::new(&exe)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                c.arg("run").arg(&source);
                if mode == 1 {
                    c.arg("--native");
                }
                c
            };
            if replay {
                c.arg(if mode == 2 {
                    "--hirsel-replay"
                } else {
                    "--replay"
                })
                .arg(&tape);
            }
            c
        };
        for (admission, list, error) in [
            (
                serde_json::json!({"expires":0}),
                Some(false),
                Some("expired"),
            ),
            (
                serde_json::json!({"not_before":i64::MAX}),
                Some(false),
                Some("not_yet_valid"),
            ),
            (
                serde_json::json!({"revocation":"grant-A"}),
                Some(true),
                Some("revoked"),
            ),
            (
                serde_json::json!({"revocation":"grant-A"}),
                None,
                Some("missing_evidence"),
            ),
            (
                serde_json::json!({"not_before":0,"expires":i64::MAX,"revocation":"grant-A"}),
                Some(false),
                None,
            ),
        ] {
            std::fs::write(
                &grants,
                serde_json::json!({"io":stdout_grant(),"admission":admission}).to_string(),
            )
            .unwrap();
            let mut c = command(false);
            if let Some(revoked) = list {
                std::fs::write(&revocations, serde_json::json!({"schema":1,"revoked":if revoked {vec!["grant-A"]} else {vec![]}}).to_string()).unwrap();
                c.arg(if mode == 2 {
                    "--hirsel-revocations"
                } else {
                    "--revocations"
                })
                .arg(&revocations);
            }
            if error.is_none() {
                c.arg(if mode == 2 {
                    "--hirsel-record"
                } else {
                    "--record"
                })
                .arg(&tape);
            }
            c.arg(if mode == 2 {
                "--hirsel-grants"
            } else {
                "--grants"
            })
            .arg(&grants);
            let output = c.output().unwrap();
            if let Some(error) = error {
                assert!(!output.status.success(), "mode={mode}: {output:?}");
                assert!(
                    output.stdout.is_empty(),
                    "entry ran before refusal: {output:?}"
                );
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains(error),
                    "mode={mode}: {output:?}"
                );
            } else {
                assert!(output.status.success(), "mode={mode}: {output:?}");
                assert_eq!(output.stdout, b"entered\n");
                assert!(output.stderr.is_empty(), "{output:?}");
                let recorded = hirsel_rt::launch::read_recording_text(&tape).unwrap();
                let field =
                    |name: &str| recorded.lines().find_map(|l| l.strip_prefix(name)).unwrap();
                assert!(field("admitted_at ").parse::<i64>().unwrap() > 0);
                let proof: Vec<_> = field("revocation ").split(' ').collect();
                assert_eq!(proof.len(), 3);
                assert_eq!(proof[0], "6772616e742d41"); // grant-A
                assert_eq!(proof[1].len(), 128); // hex-encoded SHA-256 revision
                assert_eq!(proof[2], "0");
                // This exact accepted tape still replays when the host now revokes it,
                // and when both the source of evidence and grants cease to exist.
                std::fs::write(&revocations, r#"{"schema":1,"revoked":["grant-A"]}"#).unwrap();
                let output = command(true).output().unwrap();
                assert!(output.status.success(), "{output:?}");
                assert_eq!(output.stdout, b"entered\n");
                std::fs::remove_file(&revocations).unwrap();
                std::fs::remove_file(&grants).unwrap();
                // A valid historical admission at 10ns with expiry at 20ns. Rebind
                // the tape, retaining its real Io events, to check replay's clock seam
                // deterministically without waiting for a host deadline.
                let decode = |text: &str| {
                    (0..text.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                        .collect::<Vec<_>>()
                };
                let encode =
                    |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
                let canonical = field("run ");
                let mut historical: serde_json::Value =
                    serde_json::from_slice(&decode(canonical)).unwrap();
                historical["admission"]["expires"] = 20.into();
                std::fs::write(&grants, historical.to_string()).unwrap();
                let run = hirsel_engine::read_run_record(&grants).unwrap();
                std::fs::remove_file(&grants).unwrap();
                let historical = recorded
                    .replace(
                        &format!("run {canonical}\n"),
                        &format!("run {}\n", encode(run.canonical())),
                    )
                    .replace(
                        &format!("run_identity {}\n", field("run_identity ")),
                        &format!("run_identity {}\n", encode(&run.identity())),
                    )
                    .replace(
                        &format!("admitted_at {}\n", field("admitted_at ")),
                        "admitted_at 10\n",
                    );
                std::fs::write(&tape, historical).unwrap();
                let output = command(true).output().unwrap();
                assert!(
                    output.status.success(),
                    "historical mode={mode}: {output:?}"
                );
                assert_eq!(output.stdout, b"entered\n");
                assert!(output.stderr.is_empty(), "{output:?}");
            }
        }
    }
    for path in [source, exe, tape] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn real_host_processes_run_granted_echo_and_false_and_refuse_everything_else() {
    let source = file(
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let p = io.process(rt)?
  match proc.run(p, Command(program: path.of("/bin/echo"), args: ["hello", "from echo"], dir: None(), env: None(), own_group: false), b"", 1024, None()) {
    Ok(o) => {
      if o.status == Exited(0) && o.stdout == b"hello from echo\n" && o.stderr == b"" {
        let _ = out.print(out, "echo ok\n")
      }
    },
    Error(_) => (),
  }
  let fails = proc.run(p, Command(program: path.of("/bin/false"), args: [], dir: None(), env: None(), own_group: false), b"", 1024, None())
  if fails == Ok(ProcessOutput(status: Exited(1), stdout: b"", stderr: b"")) {
    let _ = out.print(out, "false exited 1\n")
  }
  if proc.run(p, Command(program: path.of("/bin/sh"), args: ["-c", "true"], dir: None(), env: None(), own_group: false), b"", 1024, None()) == Error(NotGranted()) {
    let _ = out.print(out, "sh not granted\n")
  }
  if proc.run(p, Command(program: path.of("echo"), args: [], dir: None(), env: None(), own_group: false), b"", 1024, None()) == Error(InvalidInput()) {
    let _ = out.print(out, "search name refused\n")
  }
  Ok(())
}
"#,
    );
    let grants = file(
        r#"{"io":{"stdout":"stdout.bytes@1","dirs":[],"process":{"provider":"process.spawn@1","rules":[{"program":"/bin/echo"},{"program":"/bin/false"}]}}}"#,
    );
    let run = |native: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(&source).arg("--grants").arg(&grants);
        if native {
            command.arg("--native");
        }
        command.output().unwrap()
    };
    for native in [false, true] {
        let output = run(native);
        assert_eq!(output.status.code(), Some(0), "native={native}: {output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "echo ok\nfalse exited 1\nsh not granted\nsearch name refused\n",
            "native={native}"
        );
    }
    // Without the process grant there is no Proc: main returns its GrantError.
    std::fs::write(&grants, r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#).unwrap();
    for native in [false, true] {
        let output = run(native);
        assert_eq!(output.status.code(), Some(1), "native={native}: {output:?}");
        assert!(output.stdout.is_empty());
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(grants).unwrap();
}

#[test]
fn host_quiescence_matches_simulation_and_retains_waits_without_cleanup() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["two-task-channel", "await-channel-cycle", "held-sender"] {
        for blocked in [true, false] {
            let suffix = if blocked { "blocked" } else { "run" };
            let original = root.join(format!("conformance/CONC/CONC-8-{name}-{suffix}.hir"));
            let source = file(&std::fs::read_to_string(&original).unwrap());
            let record = run_record(
                &source,
                serde_json::json!({"io":{"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","dirs":[]},"args":[]}),
            );
            let world = source.with_extension("world.json");
            std::fs::write(&world, "{}").unwrap();
            for host in [false, true] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command.arg("run").arg(&source);
                if host {
                    command.arg("--run").arg(&record);
                } else {
                    command.arg("--simulation").arg(&world);
                }
                let output = command.output().unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(if blocked { 4 } else { 0 }),
                    "{name} blocked={blocked} host={host}: {output:?}"
                );
                assert_eq!(
                    output.stdout,
                    if blocked {
                        b"waiting\n".as_slice()
                    } else {
                        b"waiting\ndestroyed\n".as_slice()
                    },
                    "{name} host={host}: {output:?}"
                );
                if blocked {
                    let report = String::from_utf8_lossy(&output.stderr);
                    assert!(
                        report.starts_with("Blocked: [Wait { spawn_path: []"),
                        "{report}"
                    );
                    assert!(report.contains("channel.receive"), "{report}");
                    if name != "held-sender" {
                        assert!(report.contains("task.await"), "{report}");
                    }
                } else {
                    assert!(output.stderr.is_empty(), "{output:?}");
                }
            }
            std::fs::remove_file(source).unwrap();
            std::fs::remove_file(record).unwrap();
            std::fs::remove_file(world).unwrap();
        }
    }
}
#[test]
fn check_reports_each_unsafe_ffi_package_in_both_formats() {
    let path = file(
        "/// Reads its arguments and retains nothing.\nextern \"c\" fn demo_add(u: Unsafe, a: i32, b: i32) -> i32\n",
    );
    let sidecar = format!("{}.json", path.display());
    std::fs::write(&sidecar, format!(r#"{{"schema":1,"package":{{"name":"app","version":"1"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[],"unsafe_ffi":{{"link":["demo"],"headers_checked":false,"wrappers":[]}}}}"#, path.display())).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "note: app@1 is an unsafe ffi package (links -ldemo; headers not checked; threads home)\n"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"schema_version\":1,\"unsafe_ffi_package\":{\"headers_checked\":false,\"link\":[\"-ldemo\"],\"name\":\"app\",\"threads\":\"home\",\"version\":\"1\"}}\n"
    );
}

#[test]
fn check_safe_reports_the_path_to_the_first_escape() {
    // CAP-6 is a host admission verdict, outside the conformance header schema.
    let ffi = |body: &str| {
        let path = file(&format!(
            "/// Reads its arguments and retains nothing.\nextern \"c\" fn demo_add(u: Unsafe, a: i32, b: i32) -> i32\n\n{body}"
        ));
        let sidecar = format!("{}.json", path.display());
        std::fs::write(&sidecar, format!(r#"{{"schema":1,"package":{{"name":"app","version":"1"}},"entry":{{"module":[],"name":"main"}},"sources":[{{"path":"{}","module":[]}}],"dependencies":[],"unsafe_ffi":{{"link":["demo"],"headers_checked":false,"wrappers":[]}}}}"#, path.display())).unwrap();
        path
    };
    let main = "pub fn main(u: Unsafe) -> Result[(), ()] {\n  let _ = relay(u, 2)\n  Ok(())\n}\n";
    let cases = [
        (
            ffi(&format!(
                "fn sum(u: Unsafe, a: i32) -> i32 {{\n  demo_add(u, a, 1)\n}}\n\nfn relay(u: Unsafe, a: i32) -> i32 {{\n  sum(u, a) + 1\n}}\n\n{main}"
            )),
            1,
            "unsafe: app@1/main reaches the extern \"c\" function app@1/demo_add: app@1/main -> app@1/relay -> app@1/sum -> app@1/demo_add\n",
        ),
        // Holding Unsafe, or selecting an unsafe ffi package, is not an escape.
        (
            ffi(&format!(
                "fn sum(u: Unsafe, a: i32) -> i32 {{\n  a + 1\n}}\n\nfn relay(u: Unsafe, a: i32) -> i32 {{\n  sum(u, a) + 1\n}}\n\n{main}"
            )),
            0,
            "safe: app@1/main reaches no extern \"c\" function or Unsafe operation\n",
        ),
        (
            file(&format!(
                "fn relay(u: Unsafe, a: i64) -> i64 {{\n  int.unchecked_add(u, a, 1)\n}}\n\n{main}"
            )),
            1,
            "unsafe: standalone@0/main reaches the Unsafe operation core@1/int/unchecked_add: standalone@0/main -> standalone@0/relay -> core@1/int/unchecked_add\n",
        ),
        // Std uses no Unsafe; core operations behind safe signatures are the runtime's.
        (
            std_file(
                "pub fn main(rt: Io) -> Result[(), String] {\n  let _ = match json.parse(\"{\\\"a\\\": [1, 2]}\") {\n    Ok(v) => v,\n    Error(_) => return Error(\"bad json\"),\n  }\n  let _ = crc32.digest(string.as_bytes(\"abc\"))\n  let _ = sha256.digest(string.as_bytes(\"abc\"))\n  let out = match io.stdout(rt) { Ok(s) => s, Error(_) => return Ok(()) }\n  match out.print(out, \"ok\\n\") { Ok(_) => Ok(()), Error(_) => Error(\"print\") }\n}\n",
            ),
            0,
            "safe: standalone@0/main reaches no extern \"c\" function or Unsafe operation\n",
        ),
    ];
    for (path, code, verdict) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["check", "--safe"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), verdict);
    }
}

#[test]
fn simulation_net_grant_assigns_the_first_ephemeral_listener_port() {
    // The CLI's explicit grant input is outside the conformance header schema.
    let source = file(
        r#"
pub type LaunchError = Grant(GrantError) | Network(NetError)
pub fn main(rt: Io) -> Result[(), LaunchError] {
  let n = io.net(rt) ? Grant
  let l = net.listen(n, SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)) ? Network
  let bound = net.local_addr(n, l) ? Network
  failure.check(bound.port == 49152u16, Arithmetic())
  Ok(())
}
"#,
    );
    let config = file(
        r#"{"grants":["Io"],"projections":["net"],"net":{"provider":"net.script@2","rules":[{"address":"127.0.0.1","service":"0","operation":"listen"}],"peers":[]}}"#,
    );
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&config)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    for path in [source, config] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn runtime_failure_reports_a_complete_site_in_human_and_json() {
    let path = file(
        "fn fail() -> u64 {\n  1u64 / 0u64\n}\npub fn main() -> Result[(), ()] {\n  failure.check(fail() == 0, Arithmetic())\n  Ok(())\n}\n",
    );
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run");
        if json {
            command.arg("--json");
        }
        let output = command.arg(&path).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let text = String::from_utf8(output.stderr).unwrap();
        if json {
            let report: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(report["runtime_failure"]["rule"], "SEM-1");
            let site = &report["runtime_failure"]["site"];
            assert_eq!(site["definition"], "standalone@0/fail");
            assert_eq!(site["file"], path.to_str().unwrap());
            assert_eq!(site["line"], 2);
            assert_eq!(site["column"], 3);
        } else {
            assert_eq!(
                text,
                format!(
                    "runtime failure[SEM-1] at standalone@0/fail ({}:2:3)\n",
                    path.display()
                )
            );
        }
    }
    // Source paths are OS strings. JSON uses the same lossy display as human text.
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.extend(b"\xff.hir");
    let non_utf8 = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));
    std::fs::rename(&path, &non_utf8).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--json"])
        .arg(&non_utf8)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        report["runtime_failure"]["site"]["file"],
        non_utf8.to_string_lossy().as_ref()
    );
    std::fs::remove_file(non_utf8).unwrap();
}

#[test]
fn runtime_failure_reports_the_dependency_file_and_identity() {
    let library = file("pub fn fail() -> u64 {\n  1u64 / 0u64\n}\n");
    let manifest = library.with_extension("json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema": 1, "package": {"name": "dep", "version": "2"},
            "sources": [{"path": library, "module": ["helper"]}], "dependencies": []
        })
        .to_string(),
    )
    .unwrap();
    let source = file(
        "pub fn main() -> Result[(), ()] { failure.check(helper.fail() == 0, Arithmetic()); Ok(()) }\n",
    );
    let sidecar = source.with_extension("hir.json");
    std::fs::write(&sidecar, serde_json::json!({
        "schema": 1, "package": {"name": "app", "version": "1"},
        "sources": [{"path": source, "module": []}],
        "dependencies": [{"manifest": manifest, "package": {"name": "dep", "version": "2"}, "expose": []}],
        "entry": {"module": [], "name": "main"}
    }).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--json"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    let site = &report["runtime_failure"]["site"];
    assert_eq!(site["definition"], "dep@2/helper/fail");
    assert_eq!(site["file"], library.to_str().unwrap());
    assert_eq!(site["line"], 2);
    assert_eq!(site["column"], 3);
    for path in [library, manifest, source, sidecar] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn doc_manifest_json_and_output_file_share_public_api() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = root.join("packages/doc_api@1/hirsel.json");
    let destination = file("").with_extension("md");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["doc", "--json"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema"], 1);
    let entries = json["entries"].as_array().unwrap();
    let expected = hirsel_driver::api_index::render_json(
        &hirsel_driver::api_index::entries(&manifest).unwrap(),
    );
    assert_eq!(
        json,
        serde_json::from_str::<serde_json::Value>(&expected).unwrap()
    );
    let token = entries
        .iter()
        .find(|e| e["identity"] == "doc_api@1/api/token")
        .unwrap();
    assert_eq!(
        token["declaration"],
        "pub fn api.token(value: wire.Token) -> doc_dep@2/model.Token"
    );
    assert_eq!(token["anchor"], "def-doc_5fapi_401_2fapi_2ftoken");
    assert_eq!(token["links"][0]["identity"], "doc_dep@2/model/Token");
    assert_eq!(
        token["links"][0]["url"],
        "../doc_dep@2/api.md#def-doc_5fdep_402_2fmodel_2fToken"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private_function"));
    let saved_json = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["doc", "--json", "-o"])
        .arg(&destination)
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(saved_json.status.code(), Some(0), "{saved_json:?}");
    assert!(saved_json.stdout.is_empty());
    assert!(saved_json.stderr.is_empty());
    assert_eq!(std::fs::read(&destination).unwrap(), output.stdout);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("doc")
        .arg(&manifest)
        .arg("-o")
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        include_str!("../../../../packages/doc_api@1/api.md")
    );
    std::fs::remove_file(destination).unwrap();
}

#[test]
fn doc_requires_a_valid_explicit_package_manifest() {
    for args in [
        vec!["doc"],
        vec!["doc", "--index"],
        vec!["doc", "--root", "."],
        vec!["doc", "--json", "--json", "missing.json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let manifest = file(
        r#"{"schema":1,"package":{"name":"probe","version":"1"},"sources":[],"dependencies":[]}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("doc")
        .arg(&manifest)
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn tracked_runtime_reports_keep_application_and_dependency_operation_sites() {
    let library = file("@track_caller\npub fn fail(n: u8) -> u8 {\n  n + 1\n}\n");
    let manifest = library.with_extension("json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema": 1, "package": {"name": "dep", "version": "2"},
            "sources": [{"path": library, "module": ["helper"]}], "dependencies": []
        })
        .to_string(),
    )
    .unwrap();
    let source =
        file("pub fn main() -> Result[(), ()] {\n  let _ = helper.fail(255u8)\n  Ok(())\n}\n");
    let sidecar = source.with_extension("hir.json");
    std::fs::write(&sidecar, serde_json::json!({
        "schema": 1, "package": {"name": "app", "version": "1"},
        "sources": [{"path": source, "module": []}],
        "dependencies": [{"manifest": manifest, "package": {"name": "dep", "version": "2"}, "expose": []}],
        "entry": {"module": [], "name": "main"}
    }).to_string()).unwrap();
    let expected = format!(
        "runtime failure[SEM-1] at app@1/main ({}:2:11)\n  in dep@2/helper/fail ({}:3:3)\n",
        source.display(),
        library.display()
    );
    let expected_standalone = format!(
        "runtime failure[SEM-1] at app@1/main (app@1/{}:2:11)\n  in dep@2/helper/fail (dep@2/{}:3:3)\n",
        source.file_name().unwrap().to_string_lossy(),
        library.file_name().unwrap().to_string_lossy()
    );
    for backend in [None, Some("llvm"), Some("cranelift")] {
        for json in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run");
            if let Some(backend) = backend {
                command.args(["--native", "--backend", backend]);
            }
            if json {
                command.arg("--json");
            }
            let output = command.arg(&source).output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{backend:?}: {output:?}");
            if json {
                let report: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
                let failure = &report["runtime_failure"];
                assert_eq!(failure["rule"], "SEM-1");
                for (field, definition, path, line, column) in [
                    ("site", "app@1/main", &source, 2, 11),
                    ("operation", "dep@2/helper/fail", &library, 3, 3),
                ] {
                    assert_eq!(failure[field]["definition"], definition);
                    assert_eq!(failure[field]["file"], path.to_str().unwrap());
                    assert_eq!(failure[field]["line"], line);
                    assert_eq!(failure[field]["column"], column);
                }
            } else {
                assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
            }
        }
        if let Some(backend) = backend {
            let exe = build_executable(&source, &["--debug", "--backend", backend]);
            let output = Command::new(&exe).output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{backend}: {output:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                expected_standalone
            );
            std::fs::remove_file(exe).unwrap();
        }
    }
    for path in [library, manifest, source, sidecar] {
        std::fs::remove_file(path).unwrap();
    }
}

/// Both engines end a run the same way and `hirsel run` reports it the same way: a Blocked run
/// (exit 2), a run past the codec's operational budget (environmental, exit 6) and a run whose
/// grants the host refuses at admission (the refusal, exit 1), each after the output it wrote.
/// A native run is admitted once, by the executable, which reports to the driver rather than
/// to the terminal; its twin, granted a directory that exists, runs.
#[test]
fn run_outcomes_are_reported_alike_by_both_engines() {
    let blocks = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let tasks = io.tasks(rt)?\n  let stdout = io.stdout(rt)?\n  let (sender, receiver): (Sender[u8], Receiver[u8]) = channel.open(tasks)\n  let mut rx = take receiver\n  let _ = out.print(stdout, \"blocks waits\\n\")\n  let _ = channel.receive(tasks, mut rx)\n  Ok(())\n}\n",
    );
    let encodes = file(
        "@serializable\ntype Tree {\n  kids: List[Tree],\n}\n\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let stdout = io.stdout(rt)?\n  let mut tree = Tree(kids: [])\n  let mut depth: u64 = 0\n  while depth < 19 {\n    let mut kids: List[Tree] = []\n    list.push(mut kids, copy tree)\n    list.push(mut kids, take tree)\n    tree = Tree(kids: kids)\n    depth += 1\n  }\n  let _ = out.print(stdout, \"encodes\\n\")\n  let bytes = codec.encode(tree)\n  let _ = out.print(stdout, \"encoded\\n\")\n  Ok(())\n}\n",
    );
    let prints = file(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"prints\\n\")\n  Ok(())\n}\n",
    );
    let grants = |dirs: serde_json::Value| {
        file(
            &serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":dirs},"args":[]})
                .to_string(),
        )
    };
    // Its own directory: a grant of the whole temporary directory would hold the native
    // driver's observation record, which admission refuses (recorder overlap).
    let present_scratch = tempfile::Builder::new()
        .prefix("hirsel-cli-present-")
        .tempdir()
        .unwrap();
    let present = present_scratch.path().to_path_buf();
    let missing_scratch = tempfile::tempdir().unwrap();
    let missing = missing_scratch.path().join("missing.hir");
    let plain = grants(serde_json::json!([]));
    let refused_grants =
        grants(serde_json::json!([{"path":"data","host":missing,"access":"read"}]));
    let admitted_grants =
        grants(serde_json::json!([{"path":"data","host":present,"access":"read"}]));
    for native in [false, true] {
        let run = |source: &std::path::Path, grants: &std::path::Path| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run");
            if native {
                command.args(["--native", "--backend", "cranelift"]);
            }
            let output = command
                .arg(source)
                .arg("--grants")
                .arg(grants)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            assert!(!stderr.contains("hirsel-rt:"), "native {native}: {stderr}");
            (output.status.code(), output.stdout, stderr)
        };
        let (code, stdout, stderr) = run(&blocks, &plain);
        assert_eq!(code, Some(4), "native {native}: {stderr}");
        assert_eq!(stdout, b"blocks waits\n", "native {native}");
        assert!(stderr.starts_with("Blocked: "), "native {native}: {stderr}");
        assert!(
            stderr.contains("channel.receive"),
            "native {native}: {stderr}"
        );
        assert!(
            stderr.contains("spawn_path: []"),
            "native {native}: {stderr}"
        );
        assert!(stderr.contains("line: 7"), "native {native}: {stderr}");
        assert!(stderr.contains("targets: ["), "native {native}: {stderr}");
        let (code, stdout, stderr) = run(&encodes, &plain);
        assert_eq!(code, Some(6), "native {native}: {stderr}");
        assert_eq!(stdout, b"encodes\n", "native {native}");
        assert_eq!(
            stderr, "hirsel: environmental failure: codec operational budget exhausted\n",
            "native {native}"
        );
        let (code, stdout, stderr) = run(&prints, &refused_grants);
        assert_eq!(code, Some(3), "native {native}: {stderr}");
        assert!(stdout.is_empty(), "native {native}");
        assert!(
            stderr
                .starts_with("hirsel: launch: {\"kind\":\"unauthorized\",\"field\":\"io.dirs.data"),
            "native {native}: {stderr}"
        );
        let (code, stdout, stderr) = run(&prints, &admitted_grants);
        assert_eq!(code, Some(0), "native {native}: {stderr}");
        assert_eq!(stdout, b"prints\n", "native {native}");
    }
    for path in [
        blocks,
        encodes,
        prints,
        plain,
        refused_grants,
        admitted_grants,
    ] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(present).unwrap();
}

/// Unchecked preconditions are outside deterministic replay, so this checks the
/// native process outcome directly instead of classifying UB as a corpus failure.
#[test]
fn native_unchecked_add_checks_every_integer_width_and_reports_overflow() {
    let source = file(
        r#"pub fn main(rt: Io, args: Args, permit: Unsafe) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, "before unchecked\n")
  failure.check(int.unchecked_add(permit, 200u8, 55u8) == 255u8, Arithmetic())
  failure.check(int.unchecked_add(permit, 65534u16, 1u16) == 65535u16, Arithmetic())
  failure.check(int.unchecked_add(permit, 4294967294u32, 1u32) == 4294967295u32, Arithmetic())
  failure.check(int.unchecked_add(permit, 18446744073709551614u64, 1u64) == 18446744073709551615u64, Arithmetic())
  failure.check(int.unchecked_add(permit, 340282366920938463463374607431768211454u128, 1u128) == 340282366920938463463374607431768211455u128, Arithmetic())
  failure.check(int.unchecked_add(permit, -120i8, -8i8) == -128i8, Arithmetic())
  failure.check(int.unchecked_add(permit, -32767i16, -1i16) == -32768i16, Arithmetic())
  failure.check(int.unchecked_add(permit, -2147483647i32, -1i32) == -2147483648i32, Arithmetic())
  failure.check(int.unchecked_add(permit, -9223372036854775807i64, -1i64) == -9223372036854775808i64, Arithmetic())
  failure.check(int.unchecked_add(permit, -170141183460469231731687303715884105727i128, -1i128) == -170141183460469231731687303715884105728i128, Arithmetic())
  match arg.len(args) {
    1u64 => { let _ = int.unchecked_add(permit, 255u8, 1u8); },
    2u64 => { let _ = int.unchecked_add(permit, 127i8, 1i8); },
    3u64 => { let _ = int.unchecked_add(permit, 340282366920938463463374607431768211455u128, 1u128); },
    4u64 => { let _ = int.unchecked_add(permit, 170141183460469231731687303715884105727i128, 1i128); },
    5u64 => { let _ = int.unchecked_add(permit, -170141183460469231731687303715884105728i128, -1i128); },
    _ => (),
  }
  Ok(())
}
"#,
    );
    for backend in ["llvm", "cranelift"] {
        let executable = build_executable(&source, &["--backend", backend]);
        for count in 0..=5 {
            let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":vec!["overflow";count],"unsafe":true}).to_string();
            let native = Command::new(&executable)
                .args(["--hirsel-grants", &grants])
                .args(vec!["overflow"; count])
                .output()
                .unwrap();
            let interpreted = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&source)
                .args(["--grants", &grants])
                .output()
                .unwrap();
            assert_eq!(
                native.status.code(),
                interpreted.status.code(),
                "{backend} count={count}: {native:?} vs {interpreted:?}"
            );
            assert_eq!(
                native.status.code(),
                Some(if count == 0 { 0 } else { 3 }),
                "{backend}: {native:?}"
            );
            assert_eq!(
                native.stdout, b"before unchecked\n",
                "{backend}: {native:?}"
            );
            assert_eq!(native.stdout, interpreted.stdout);
            let message = String::from_utf8(native.stderr).unwrap();
            if count == 0 {
                assert!(message.is_empty(), "{message}");
            } else {
                assert!(
                    message.contains("undefined behaviour detected: int.unchecked_add at line"),
                    "{message}"
                );
                assert!(message.contains("mathematical sum must fit"), "{message}");
                assert!(!message.contains("runtime failure"), "{message}");
            }
        }
        std::fs::remove_file(executable).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}
