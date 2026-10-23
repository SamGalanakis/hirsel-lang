//! Owned CSV construction: pinned Go vectors, reachable V, four engines and copy events.
use hirsel_driver::{native, *};
use hirsel_engine::*;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/csv-owned-table-37.hir")
}
fn checked(source: &str) -> LoadedProgram {
    let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
    loaded.check_for_execution().unwrap_or_else(|e| {
        if let ToolError::Query {
            error: QueryError::Diagnostics(d),
            sources,
            ..
        } = &e
        {
            for diagnostic in d {
                eprintln!("{}", diagnostic.render_human(sources));
            }
        }
        panic!("{e}")
    });
    loaded
}
fn literal(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}
fn expected(case: &Value) -> String {
    if let Some(rows) = case["expected"]["rows"].as_array() {
        format!(
            "Ok([{}])",
            rows.iter()
                .map(|row| format!(
                    "[{}]",
                    row.as_array()
                        .unwrap()
                        .iter()
                        .map(|s| literal(s.as_str().unwrap()))
                        .collect::<Vec<_>>()
                        .join(",")
                ))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        let error = &case["expected"]["error"];
        let kind = error["kind"].as_str().unwrap();
        if let Some(p) = error["position"].as_array() {
            format!(
                "Error({kind}(std@1/csv.Position(start_line: {}, line: {}, column: {})))",
                p[0], p[1], p[2]
            )
        } else {
            format!("Error({kind}())")
        }
    }
}
fn vectors(part: usize) -> String {
    // Go BSD-3-Clause, ebe2feef reader_test.go:40-110; retained notices and
    // independent expected values live with the existing pinned vector manifest.
    let manifest: Value =
        serde_json::from_str(include_str!("../fixtures/csv-upstream-25/vectors.json")).unwrap();
    let mut source = include_str!("../fixtures/csv-upstream-25/helpers.hir").to_owned();
    source.push_str(r#"
fn compare(take input: String, config: std@1/csv.Config, answer: Result[List[List[String]], std@1/csv.CsvError]) {
  failure.check(std@1/csv.parse_owned(copy input, config) == answer, TextInvariant())
  failure.check(std@1/csv.parse(copy input, config) == answer, TextInvariant())
  failure.check(borrowed(input, config) == answer, TextInvariant())
  let data = string.as_bytes(input)
  let mut sizes: List[u64] = []
  for i in 0u64..bytes.len(data) { list.push(mut sizes, 1u64) }
  failure.check(fed(data, sizes, config) == answer, TextInvariant())
  failure.check(pulled(data, sizes, config) == answer, TextInvariant())
}
pub fn main() -> Result[(), ()] {
"#);
    if part < 4 {
        let start = part * 9;
        let end = (start + 9).min(34);
        for case in &manifest["cases"].as_array().unwrap()[start..end] {
            assert_eq!(case["string_input"], true);
            let raw = case["hex"]
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            let text = literal(std::str::from_utf8(&raw).unwrap());
            let width = case["width"].as_i64().unwrap();
            let width = if width < 0 {
                "None()".into()
            } else {
                format!("Some({width})")
            };
            source.push_str(&format!("compare({text}, std@1/csv.Config(delimiter: {}u8, fields_per_record: {width}, max_record_bytes: {}), {})\n",case["delimiter"],case["limit"],expected(case)));
        }
    } else {
        assert_eq!(part, 4);
        // Independent Hirsel policy answers: bounds precede syntax on the same
        // byte; a completed wrong-width record precedes a malformed suffix.
        source.push_str(
            r#"
  let mut c = std@1/csv.default_config()
  c.max_record_bytes = 3
  compare("a,b\r\n", c, Ok([["a", "b"]]))
  compare("a,bc\n", c, Error(RecordTooLarge()))
  compare("ab\"", c, Error(Syntax(std@1/csv.Position(start_line: 1, line: 1, column: 3))))
  c.max_record_bytes = 2
  compare("ab\"", c, Error(RecordTooLarge()))
  compare("\"x\"!", c, Error(RecordTooLarge()))
  c.max_record_bytes = 3
  compare("\"x\"!", c, Error(RecordTooLarge()))
  c.max_record_bytes = 4
  compare("\"x\"!", c, Error(Syntax(std@1/csv.Position(start_line: 1, line: 1, column: 3))))
  compare("\"x\r", c, Error(Syntax(std@1/csv.Position(start_line: 1, line: 1, column: 3))))
  c.max_record_bytes = 6
  compare("\"x\r\ny\"\r", c, Ok([["x\ny"]]))
  c.fields_per_record = Some(2)
  compare("a\n\"", c, Error(UnequalFields(std@1/csv.Position(start_line: 1, line: 1, column: 1))))
  compare("a,b\n\"x", c, Error(Syntax(std@1/csv.Position(start_line: 2, line: 2, column: 3))))
  compare("\n\r\na,b\n", c, Ok([["a", "b"]]))
  compare("a,b,c\n", c, Error(UnequalFields(std@1/csv.Position(start_line: 1, line: 1, column: 1))))
  c.max_record_bytes = 0
  compare("\r\n\n\r", c, Ok([]))
  compare("a", c, Error(RecordTooLarge()))
  for delimiter in [0u8, 128u8, b'"', b'\r', b'\n'] {
    c.delimiter = delimiter
    compare("\"broken", c, Error(InvalidDelimiter()))
  }
"#,
        );
    }
    source.push_str("Ok(())\n}\n");
    source
}
fn run(engine: &str, part: usize) {
    with_compiler_stack(|| {
        let source = if part == 5 {
            include_str!("../fixtures/csv-owned-table-37.hir").to_owned()
        } else {
            vectors(part)
        };
        let mut loaded = checked(&source);
        eprintln!("CSV_OWNED_37_STARTED part={part} engine={engine}");
        let input = loaded
            .engine
            .export_native_input()
            .expect("reachable V export");
        let validated = loaded
            .engine
            .import_native_input(input)
            .expect("independent V import");
        if engine == "reference" || engine == "checked-h" {
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let execution = if engine == "reference" {
                run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
            } else {
                run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
            }
            .unwrap();
            assert!(
                matches!(
                    execution.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{execution:?}"
            );
            let observation = execution.observation.unwrap();
            assert_eq!(observation.exit_code, Some(0));
            assert!(
                observation.failures.is_empty() && observation.output.is_empty(),
                "{observation:?}"
            );
        } else {
            let matrix = [native::Configuration {
                options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                engine: if engine == "llvm" {
                    native::Engine::Native {
                        opt: native::OptLevel::Speed,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    }
                } else {
                    native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    }
                },
            }];
            let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("csv-owned-table-37-{engine}-{part}"));
            let mut tools =
                native::Toolchain::for_matrix(&scratch.join("runtime"), &matrix).unwrap();
            tools.launch_timeout = std::time::Duration::from_secs(60);
            let expected = native::Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output: vec![],
                failures: vec![],
                roots: vec![],
            };
            let outcomes = native::differential(
                &validated,
                &loaded.entry,
                &expected,
                &matrix,
                &tools,
                &scratch,
            );
            assert_eq!(outcomes.len(), 1);
            for (name, outcome) in outcomes {
                assert_eq!(outcome, native::Outcome::Agree, "{name}: {outcome:?}");
            }
        }
        eprintln!("CSV_OWNED_37_PASS part={part} engine={engine}");
    });
}
#[test]
fn csv_owned_table_37_reference_0() {
    run("reference", 0);
}
#[test]
fn csv_owned_table_37_reference_1() {
    run("reference", 1);
}
#[test]
fn csv_owned_table_37_reference_2() {
    run("reference", 2);
}
#[test]
fn csv_owned_table_37_reference_3() {
    run("reference", 3);
}
#[test]
fn csv_owned_table_37_reference_4() {
    run("reference", 4);
}
#[test]
fn csv_owned_table_37_reference_5() {
    run("reference", 5);
}
#[test]
fn csv_owned_table_37_checked_h_0() {
    run("checked-h", 0);
}
#[test]
fn csv_owned_table_37_checked_h_1() {
    run("checked-h", 1);
}
#[test]
fn csv_owned_table_37_checked_h_2() {
    run("checked-h", 2);
}
#[test]
fn csv_owned_table_37_checked_h_3() {
    run("checked-h", 3);
}
#[test]
fn csv_owned_table_37_checked_h_4() {
    run("checked-h", 4);
}
#[test]
fn csv_owned_table_37_checked_h_5() {
    run("checked-h", 5);
}
#[test]
fn csv_owned_table_37_llvm_0() {
    run("llvm", 0);
}
#[test]
fn csv_owned_table_37_llvm_1() {
    run("llvm", 1);
}
#[test]
fn csv_owned_table_37_llvm_2() {
    run("llvm", 2);
}
#[test]
fn csv_owned_table_37_llvm_3() {
    run("llvm", 3);
}
#[test]
fn csv_owned_table_37_llvm_4() {
    run("llvm", 4);
}
#[test]
fn csv_owned_table_37_llvm_5() {
    run("llvm", 5);
}
#[test]
fn csv_owned_table_37_cranelift_0() {
    run("cranelift", 0);
}
#[test]
fn csv_owned_table_37_cranelift_1() {
    run("cranelift", 1);
}
#[test]
fn csv_owned_table_37_cranelift_2() {
    run("cranelift", 2);
}
#[test]
fn csv_owned_table_37_cranelift_3() {
    run("cranelift", 3);
}
#[test]
fn csv_owned_table_37_cranelift_4() {
    run("cranelift", 4);
}
#[test]
fn csv_owned_table_37_cranelift_5() {
    run("cranelift", 5);
}

#[test]
fn csv_owned_table_37_independent_v_adversary() {
    with_compiler_stack(|| {
        let mut loaded = checked(include_str!("../fixtures/csv-owned-table-37.hir"));
        let input = loaded.engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut seen = 0;
        for definition in input.definitions.values().filter(|d| {
            ["parse_owned", "owned_row", "owned_field"].contains(&d.path.path().0.name.as_str())
        }) {
            hirsel_validate::assert_valid(definition, &input.types, &signatures);
            seen += 1;
        }
        assert_eq!(seen, 3, "all owned producer bodies must be reachable");
        let make = input
            .definitions
            .values()
            .find(|d| d.path.path().0.name == "make")
            .unwrap();
        let mut wrong = (**make).clone();
        let mut changed = 0;
        // Forge a shared use where the producer explicitly consumes its input.
        for body in &mut wrong.bodies {
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &mut block.terminator
                {
                    if path.0.name == "parse_owned" {
                        arguments[0].value.usage = hirsel_types::UseKind::Copy;
                        changed += 1;
                    }
                }
            }
        }
        assert_eq!(changed, 1);
        let report = hirsel_validate::validate(&wrong, &input.types, &signatures, &mut vec![]);
        assert!(
            !report.violations.is_empty(),
            "forged take accepted: {report:?}"
        );
        eprintln!("CSV_OWNED_37_V rejected {:?}", report.violations);
    });
}
fn measured(source: &str, label: &str) -> String {
    let mut loaded = checked(source);
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let ready = native::build(&validated, &native::Options::new(target.clone())).unwrap();
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("csv-owned-table-37-{label}"));
    std::fs::create_dir_all(&dir).unwrap();
    let tools = native::Toolchain::in_tree_with_runtime(
        &dir.join("runtime"),
        &[&target],
        native::RuntimeBuild {
            measurement: true,
            ..Default::default()
        },
    )
    .unwrap();
    let exe = native::link(
        &ready,
        &loaded.entry,
        native::OptLevel::Speed,
        native::Partition::Whole,
        &tools,
        &dir,
    )
    .unwrap();
    let totals = dir.join("totals");
    // Retain the repository's resource wrapper verbatim in the test action.
    let cap = dir.join("capped.py");
    std::fs::write(&cap, include_str!("../../../../tools/capped")).unwrap();
    let output = Command::new("python3")
        .arg(&cap)
        .args(["--timeout", "60"])
        .arg(exe)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HIRSEL_MEMORY_LIMIT", "2G")
        .env("HIRSEL_JOBS", "1")
        .env("HIRSEL_RT_MEASUREMENT", &totals)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report = std::fs::read_to_string(totals).unwrap();
    eprintln!("CSV_OWNED_37_MEASURE {label}\n{report}");
    report
}
fn count(report: &str, key: &str) -> u64 {
    report
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key} ")))
        .unwrap()
        .parse()
        .unwrap()
}
fn allocations(report: &str) -> u64 {
    report
        .lines()
        .filter_map(|l| l.strip_prefix("class "))
        .map(|l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap())
        .sum()
}
#[test]
fn csv_owned_table_37_parse_to_freeze_copy_events() {
    with_compiler_stack(|| {
        let source = |parser: &str, freeze: &str| {
            format!(
                r#"
pub fn main() -> Result[(), ()] {{
  let Ok(rows) = std@1/csv.{parser}("head,value\nalpha,123\nquoted,\"x\"\"y\"\n", std@1/csv.default_config()) else {{ return Error(()) }}
  failure.check(list.len(rows) == 3 && rows[2][1] == "x\"y", TextInvariant())
  {freeze}
  Ok(())
}}
"#
            )
        };
        let parsed = measured(&source("parse_owned", ""), "parsed");
        let frozen = measured(
            &source(
                "parse_owned",
                "let root = shared.freeze(take rows)\nfailure.check(shared.with(root, fn(rows: List[List[String]]) -> bool uses {} { rows[1][0] == \"alpha\" }), TextInvariant())",
            ),
            "frozen",
        );
        let old = measured(
            &source("parse", "let _ = shared.freeze(unique rows)"),
            "legacy-copy",
        );
        assert_eq!(count(&parsed, "deep_copies"), 0, "{parsed}");
        assert_eq!(count(&frozen, "deep_copies"), 0, "{frozen}");
        assert!(
            count(&old, "deep_copies") > 0,
            "positive copy-event control: {old}"
        );
        assert!(
            allocations(&parsed) > 0,
            "field and row construction must allocate: {parsed}"
        );
        assert!(
            allocations(&frozen) <= allocations(&parsed) + 1,
            "freeze must not reconstruct fields: {parsed}\n{frozen}"
        );
        for report in [&parsed, &frozen, &old] {
            assert_eq!(count(report, "live_bytes"), 0, "{report}");
        }
    });
}
