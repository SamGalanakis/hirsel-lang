//! `--timings` and `--mem-usage` report on stderr; no corpus file can see them.
use super::std_file;
use std::process::Command;

const HELLO: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let text = std@1/u64.to_string(4u64)\n  let _ = out.print(io.stdout(rt)?, text)\n  Ok(())\n}\n";

/// The intervals and phases every native build of a program using std
/// reaches today; the others stay 0 until their driver step exists.
const BUILD_INTERVALS: [&str; 7] = [
    "image open",
    "S0",
    "parallel",
    "merge",
    "plan",
    "units",
    "link",
];
const BUILD_PHASES: [&str; 15] = [
    "parse",
    "resolve",
    "signatures",
    "check",
    "modes",
    "lower",
    "verify",
    "plan",
    "import",
    "place",
    "rc",
    "reuse",
    "ready",
    "lir validate",
    "codegen",
];
const INTERVALS: [&str; 11] = [
    "image open",
    "S0",
    "front prepare",
    "parallel",
    "GEN-5",
    "S1",
    "parallel2",
    "merge",
    "plan",
    "units",
    "link",
];
const PHASES: [&str; 21] = [
    "parse",
    "resolve",
    "signatures",
    "check",
    "modes",
    "lower",
    "verify",
    "comptime",
    "plan",
    "import",
    "place",
    "rc",
    "reuse",
    "ready",
    "lir validate",
    "codegen",
    "store lookup",
    "debug object",
    "lex",
    "declarations",
    "signature normalize",
];
const COUNTS: [&str; 63] = [
    "definitions",
    "units",
    "V runs",
    "store hits",
    "store misses",
    "object hits",
    "object misses",
    "native imports",
    "import hits",
    "import misses",
    "lowered",
    "dependency parses",
    "recomputes",
    "invalidated by signature",
    "invalidated by failed signature",
    "invalidated by lookup",
    "invalidated by name set",
    "invalidated by representation",
    "invalidated by executed body",
    "invalidated by executed signature",
    "invalidated by cited text",
    "invalidated by declaration candidates",
    "invalidated by validation view",
    "invalidated by universe errors",
    "invalidated by first heap",
    "invalidated by environment",
    "mode memo queries",
    "mode memo hits",
    "S1 claims",
    "S1 parks",
    "S1 duplicates",
    "S1 lock wait ns",
    "S1 park ns",
    "invalidated by validation universe",
    "S1 lower hold evictions",
    "S1 lower hold recomputes",
    "S1 lower hold peak bytes sum",
    "S1 lower query attempts",
    "S1 lower adapter calls",
    "S1 root tasks",
    "S1 discarded roots",
    "S1 registration rounds",
    "S1 scalar roots",
    "native Value structural bytes peak",
    "native active estimated bytes peak",
    "native admission budget bytes",
    "native effective workers peak",
    "native worker stack reservation bytes peak",
    "native retained Ready structural bytes peak",
    "native Value decodes",
    "native Value decode bytes",
    "native Value rebinds",
    "native effect replays",
    "native append/expand",
    "closure body scans",
    "arena body scans",
    "native late keys",
    "image declaration rows decoded",
    "image declaration rows adopted",
    "declaration site reads",
    "declaration sites materialized",
    "image declaration bytes",
    "image entry decode attempts",
];

fn build(source: &std::path::Path, timings: &str) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg(timings)
        .args(["build", "--backend", "cranelift"])
        .arg(source)
        .arg("-o")
        .arg(source.with_extension("exe"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    output
}

/// The schema-1 report `--timings=json` prints as the last stderr line, as
/// compiler-bench reads it: every field present with its type, every
/// interval, phase and count named once, in order.
fn report(stderr: &[u8]) -> serde_json::Value {
    let stderr = String::from_utf8(stderr.to_vec()).unwrap();
    let report: serde_json::Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    let object = report.as_object().unwrap();
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort();
    let mut expected = vec![
        "command",
        "comptime_verify",
        "counts",
        "intervals",
        "phases",
        "runner",
        "schema",
        "untimed_ns",
        "wait_ns",
        "wall_ns",
    ];
    if object.contains_key("memory") {
        expected.push("memory");
        expected.sort();
        let memory = &report["memory"];
        assert!(memory["peak_rss_bytes"].is_u64(), "{memory}");
        for row in memory["held"].as_array().unwrap() {
            assert!(row["name"].is_string() && row["bytes"].is_i64(), "{row}");
        }
    }
    assert_eq!(keys, expected, "{report}");
    assert_eq!(report["schema"], 1);
    assert!(report["command"].is_string());
    for field in ["image_runs", "cached_images"] {
        assert!(report["runner"][field].is_u64(), "{field}: {report}");
    }
    for row in report["runner"]["fallbacks"].as_array().unwrap() {
        assert!(row["reason"].is_string() && row["count"].is_u64(), "{row}");
    }
    for field in ["wall_ns", "untimed_ns", "wait_ns"] {
        assert!(report[field].is_u64(), "{field}: {report}");
    }
    for field in ["work_ns", "cpu_ns", "calls"] {
        assert!(
            report["comptime_verify"][field].is_u64(),
            "{field}: {report}"
        );
    }
    let names = |list: &str, key: &str, fields: &[&str]| -> Vec<String> {
        report[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                for field in fields {
                    assert!(row[field].is_u64(), "{field}: {row}");
                }
                row[key].as_str().unwrap().to_owned()
            })
            .collect()
    };
    assert_eq!(names("intervals", "interval", &["wall_ns"]), INTERVALS);
    assert_eq!(
        names("phases", "phase", &["work_ns", "cpu_ns", "calls"]),
        PHASES
    );
    let counts = report["counts"].as_object().unwrap();
    assert_eq!(
        counts.keys().collect::<Vec<_>>().len(),
        COUNTS.len(),
        "{report}"
    );
    assert!(COUNTS.iter().all(|c| counts[*c].is_u64()), "{report}");
    report
}

#[test]
fn timings_json_intervals_add_up_to_the_wall_time() {
    let source = std_file(HELLO);
    let report = report(&build(&source, "--timings=json").stderr);
    let wall = report["wall_ns"].as_u64().unwrap();
    // Intervals are additive; phases are summed work and are not added up.
    let sum: u64 = report["intervals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["wall_ns"].as_u64().unwrap())
        .sum();
    assert!(
        sum <= wall && wall - sum <= wall / 20,
        "{sum} of {wall}: {report}"
    );
    assert_eq!(report["untimed_ns"].as_u64(), Some(wall - sum));
    assert!(report["counts"]["units"].as_u64().unwrap() > 0, "{report}");
    assert!(report["counts"]["V runs"].as_u64().unwrap() > 0, "{report}");
}

#[test]
fn mem_usage_reports_peak_rss_and_the_tables_held() {
    let source = std_file(HELLO);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--mem-usage", "--timings=json"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = report(&output.stderr);
    let memory = &report["memory"];
    assert!(memory["peak_rss_bytes"].as_u64().unwrap() > 0, "{memory}");
    let held = |name: &str| {
        memory["held"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .and_then(|row| row["bytes"].as_i64())
            .unwrap_or_else(|| panic!("no {name:?} in {memory}"))
    };
    assert!(
        held("std image") > 0 && held("engine: signature snapshot") > 0,
        "{memory}"
    );
    assert!(held("engine: signatures") > 0, "{memory}");
}

/// Native memory reporting separates resident peaks, retained structures and reserved stacks.
#[test]
fn native_memory_separates_phase_rss_structural_bytes_and_stack_reservations() {
    let source = std_file(HELLO);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args([
            "build",
            "--debug",
            "--backend",
            "cranelift",
            "--mem-usage",
            "--timings=json",
        ])
        .env("HIRSEL_JOBS", "2")
        .arg(&source)
        .arg("-o")
        .arg(source.with_extension("exe"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report = report(&output.stderr);
    let points = report["memory"]["checkpoints"].as_array().unwrap();
    assert_eq!(
        points
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "front complete",
            "global Value complete",
            "native workers complete",
            "link complete"
        ]
    );
    for point in points {
        assert!(point["heap_request_delta_bytes"].is_i64(), "{point}");
        assert!(point["rss_bytes"].as_u64().unwrap() > 0, "{point}");
        assert!(point["peak_rss_bytes"].as_u64().unwrap() > 0, "{point}");
    }
    let counts = &report["counts"];
    assert!(
        counts["native Value structural bytes peak"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        counts["native active estimated bytes peak"]
            .as_u64()
            .unwrap()
            > 0
    );
    let workers = counts["native effective workers peak"].as_u64().unwrap();
    assert!((1..=2).contains(&workers));
    assert_eq!(
        counts["native worker stack reservation bytes peak"]
            .as_u64()
            .unwrap(),
        if workers > 1 {
            workers * hirsel_intern::jobs::COMPILER_STACK_BYTES as u64
        } else {
            0
        }
    );
}

/// Release and explicit whole-module LLVM optimization preserve executable behavior
/// and reject combinations that cannot run LLVM optimization.
#[test]
fn release_and_explicit_lto_preserve_behavior() {
    let source = super::file(
        "fn step(x:u64)->u64 { x+1u64 }\npub fn main()->Result[(),()] { failure.check(step(3u64)==4u64, Arithmetic()); Ok(()) }\n",
    );
    let exe = source.with_extension("exe");
    for lto in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .env("HIRSEL_JOBS", "2")
            .args(["build", "--release", "--timings=json"]);
        if lto {
            command.arg("--lto");
        }
        let output = command.arg(&source).arg("-o").arg(&exe).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let launched = Command::new(&exe).output().unwrap();
        assert!(launched.status.success(), "{launched:?}");
        assert!(
            launched.stdout.is_empty() && launched.stderr.is_empty(),
            "{launched:?}"
        );
    }
    for extra in [["--lto", "--debug"], ["--lto", "--backend=cranelift"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("build")
            .args(extra)
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(exe).unwrap();
}
