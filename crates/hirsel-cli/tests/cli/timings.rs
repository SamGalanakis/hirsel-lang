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
const PHASES: [&str; 20] = [
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
];
const COUNTS: [&str; 49] = [
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
fn timings_name_every_interval_and_phase_of_a_native_build() {
    let source = std_file(HELLO);
    let stderr = String::from_utf8(build(&source, "--timings").stderr).unwrap();
    // The interval table, then the phase table, each after its header row.
    let section = |header: &str, end: &str| -> Vec<&str> {
        stderr
            .lines()
            .skip_while(|line| !line.starts_with(header))
            .skip(1)
            .take_while(|line| !line.starts_with(end))
            .collect()
    };
    let (intervals, phases) = (
        section("interval ", "phase "),
        section("phase ", "  verify in comptime"),
    );
    let row = |rows: &[&str], name: &str| -> Vec<String> {
        let line = rows
            .iter()
            .find(|line| line.split("  ").next() == Some(name))
            .unwrap_or_else(|| panic!("no {name:?} row in\n{stderr}"));
        line[name.len()..]
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(intervals.len(), INTERVALS.len() + 1, "{stderr}");
    assert_eq!(phases.len(), PHASES.len(), "{stderr}");
    for name in INTERVALS {
        row(&intervals, name);
    }
    for name in PHASES {
        row(&phases, name);
    }
    for interval in BUILD_INTERVALS {
        let wall: f64 = row(&intervals, interval)[0].parse().unwrap();
        assert!(wall > 0.0, "{interval}: {stderr}");
    }
    for phase in BUILD_PHASES {
        let calls: u64 = row(&phases, phase)[2].parse().unwrap();
        assert!(calls > 0, "{phase}: {stderr}");
    }
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
        held("std image") > 0 && held("package cache") > 0,
        "{memory}"
    );
    assert!(held("engine: signatures") > 0, "{memory}");
}

#[test]
fn measurement_options_apply_only_to_compiling_commands() {
    let source = std_file(HELLO);
    for args in [
        &["fmt", "--check", "--timings"][..],
        &["--mem-usage", "hash"],
        &["check", "--timings=yaml"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--timings"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("hirsel timings: check, wall "),
        "{stderr}"
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

#[test]
fn check_timings_separate_serial_preparation_lexing_and_growth() {
    let source = std_file(HELLO);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["check", "--timings=json"])
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report = report(&output.stderr);
    for name in ["S0", "front prepare", "GEN-5"] {
        let row = report["intervals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["interval"] == name)
            .unwrap();
        assert!(row["wall_ns"].as_u64().unwrap() > 0, "{name}: {report}");
    }
    for name in ["lex", "parse", "declarations", "signatures"] {
        let row = report["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["phase"] == name)
            .unwrap();
        assert!(row["calls"].as_u64().unwrap() > 0, "{name}: {report}");
    }
    let intervals: u64 = report["intervals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["wall_ns"].as_u64().unwrap())
        .sum();
    assert_eq!(
        intervals + report["untimed_ns"].as_u64().unwrap(),
        report["wall_ns"]
    );
}
