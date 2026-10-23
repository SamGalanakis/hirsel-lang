//! Independent finite-decimal expectations; each partition executes identical
//! source and expected bytes on reference, checked H, LLVM and Cranelift.
use hirsel_driver::{load_program_source, native};
use hirsel_engine::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fmt::Write, path::Path};

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

const PRELUDE: &str = r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
fn make(coefficient: String, scale: i32) -> std@1/decimal.Decimal {
  let Ok(c) = std@1/bigint.parse(coefficient) else {
    failure.check(false, Arithmetic())
    return std@1/decimal.new(std@1/bigint.from_u64(0), scale)
  }
  std@1/decimal.new(c, scale)
}
fn emit(output: Stdout, id: String, value: std@1/decimal.Decimal) -> Result[(), AppError] {
  let coefficient = std@1/bigint.to_string(std@1/decimal.coefficient(value))
  let scale = std@1/decimal.scale(value)
  let text = std@1/decimal.to_string(value)
  out.print(output, f"{id}|{coefficient}|{scale}|{text}\n")?
  Ok(())
}
pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt)?
"#;

fn source(rows: &[Value]) -> String {
    let mut source = PRELUDE.to_owned();
    for row in rows {
        let text = |key: &str| row[key].as_str().unwrap();
        let number = |key: &str| row[key].as_i64().unwrap();
        let id = text("id");
        let op = text("op");
        let a = format!("make({:?}, {})", text("a"), number("sa"));
        let b = format!("make({:?}, {})", text("b"), number("sb"));
        let value = match op {
            "add" | "sub" | "mul" => format!("std@1/decimal.{op}({a}, {b})"),
            "div" => format!(
                "std@1/decimal.div({a}, {b}, {}, {}())",
                number("target"),
                text("mode")
            ),
            "rescale" => format!(
                "std@1/decimal.rescale({a}, {}, {}())",
                number("target"),
                text("mode")
            ),
            "normalize" => format!("std@1/decimal.normalize({a})"),
            "compare" => {
                writeln!(source, "  {{ let a = {a}\n let b = {b}\n let numeric = std@1/decimal.compare(a, b)\n let equal = a == b\n out.print(output, f\"{id}|{{numeric}}|{{equal}}\\n\")? }}").unwrap();
                continue;
            }
            "parse" => {
                // Only input lexemes reach the parser. Expected coefficients,
                // scales and output bytes remain on the host.
                writeln!(source, "  match std@1/decimal.parse({}) {{\n Ok(value) => {{ emit(output, \"{id}\", value)? }},\n Error(Invalid()) => {{ out.print(output, \"{id}|Invalid\\n\")? }},\n Error(OutOfRange()) => {{ out.print(output, \"{id}|OutOfRange\\n\")? }},\n }}", serde_json::to_string(&row["lexical"]).unwrap()).unwrap();
                continue;
            }
            "zero-divisor" => {
                "std@1/decimal.div(make(\"1\", 0), make(\"0\", 0), 0, HalfEven())".into()
            }
            "scale-overflow-high" => {
                "std@1/decimal.mul(make(\"0\", 2147483647), make(\"1\", 1))".into()
            }
            "scale-overflow-low" => {
                "std@1/decimal.mul(make(\"1\", -2147483648), make(\"0\", -1))".into()
            }
            _ => panic!("unknown operation {op}"),
        };
        writeln!(source, "  emit(output, {id:?}, {value})?").unwrap();
    }
    source.push_str("  Ok(())\n}\n");
    source
}

fn partition(name: &str, file: &str, fixture: &str) {
    with_compiler_stack(|| {
        let manifest: Value = serde_json::from_str(include_str!(
            "../fixtures/decimal-upstream-25/manifest.json"
        ))
        .unwrap();
        assert_eq!(
            manifest["files"][file],
            format!("{:x}", Sha256::digest(fixture.as_bytes()))
        );
        let rows: Vec<Value> = serde_json::from_str(fixture).unwrap();
        assert!(!rows.is_empty());
        for row in &rows {
            let mut input = serde_json::Map::new();
            for key in [
                "id", "op", "a", "sa", "b", "sb", "target", "mode", "lexical",
            ] {
                if let Some(value) = row.get(key) {
                    input.insert(key.into(), value.clone());
                }
            }
            let input_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&input).unwrap()));
            let expected_hash = format!(
                "{:x}",
                Sha256::digest(row["output"].as_str().unwrap().as_bytes())
            );
            assert_eq!(row["input_sha256"], input_hash);
            assert_eq!(row["expected_sha256"], expected_hash);
        }
        let expected: String = rows.iter().map(|r| r["output"].as_str().unwrap()).collect();
        let failure = rows.iter().any(|r| r["expected"] == "SEM-1");
        assert!(!failure || rows.len() == 1, "policy witnesses run alone");
        let source = source(&rows);
        let scratch = tempfile::tempdir().unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let mut loaded = load_program_source(&path, Some(&source))
            .unwrap_or_else(|e| panic!("{name}: source load {e:?}\n{source}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{name}: source check {e:?}\n{source}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let matrix = [
            native::Engine::Heap { campaign: false },
            native::Engine::Native {
                opt: native::OptLevel::None,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let runner = history::Runner::with_matrix(scratch.path(), matrix).unwrap();
        let report = runner.compare(&validated, &loaded.entry, &reference, name);
        let mut failures = Vec::new();
        eprintln!(
            "DECIMAL_SOURCE {} {:?} {:?}",
            name,
            hirsel_syntax::ContentHash::of_bytes(source.as_bytes()),
            rows.iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect::<Vec<_>>()
        );
        for cell in &report.cells {
            let mut passed = cell.category == history::Category::Observation;
            if let Some(observation) = &cell.observation {
                // Every engine is held directly to the external expectation.
                passed &= observation.exit_code == if failure { None } else { Some(0) }
                    && observation.failures.len() == usize::from(failure)
                    && observation.failures.iter().all(|f| f.rule == "SEM-1")
                    && !observation.blocked
                    && observation.budget.is_none()
                    && observation.signaled.is_none()
                    && observation.roots.is_empty();
            } else {
                passed = false;
            }
            eprintln!(
                "DECIMAL_OBSERVATION {name} {} {:?}",
                cell.engine, cell.observation
            );
            for row in &rows {
                let row_passed = passed
                    && cell.observation.as_ref().is_some_and(|observation| {
                        if failure {
                            return observation.output.is_empty();
                        }
                        let prefix = format!("{}|", row["id"].as_str().unwrap());
                        let lines: Vec<_> = observation
                            .output
                            .split_inclusive(|byte| *byte == b'\n')
                            .filter(|line| line.starts_with(prefix.as_bytes()))
                            .collect();
                        lines.len() == 1 && lines[0] == row["output"].as_str().unwrap().as_bytes()
                    });
                eprintln!(
                    "DECIMAL_CELL {}",
                    serde_json::json!({
                        "id": row["id"], "op": row["op"], "engine": cell.engine,
                        "input_sha256": row["input_sha256"], "expected_sha256": row["expected_sha256"],
                        "started": true, "completed": cell.category == history::Category::Observation,
                        "passed": row_passed, "category": format!("{:?}", cell.category),
                    })
                );
            }
            if !passed
                || cell
                    .observation
                    .as_ref()
                    .is_none_or(|observation| observation.output != expected.as_bytes())
            {
                failures.push(format!("{cell:?}"));
            }
        }
        assert_eq!(report.cells.len(), 4, "missing required engine: {report:?}");
        assert!(
            report.validated && failures.is_empty(),
            "{name}: {}\nexpected={expected:?}\nsource={source}",
            failures.join("\n")
        );
    });
}

macro_rules! part {
    ($name:ident, $file:literal) => {
        #[test]
        fn $name() {
            partition(
                stringify!($name),
                $file,
                include_str!(concat!("../fixtures/decimal-upstream-25/", $file)),
            );
        }
    };
}
part!(partition_00, "part-00.json");
part!(partition_01, "part-01.json");
part!(partition_02, "part-02.json");
part!(partition_03, "part-03.json");
part!(partition_04, "part-04.json");
part!(partition_05, "part-05.json");
part!(partition_06, "part-06.json");
part!(partition_07, "part-07.json");
part!(partition_08, "part-08.json");
part!(partition_09, "part-09.json");
part!(partition_10, "part-10.json");
part!(partition_11, "part-11.json");
part!(partition_12, "part-12.json");
part!(partition_13, "part-13.json");
part!(partition_14, "part-14.json");
part!(partition_15, "part-15.json");
part!(partition_16, "part-16.json");
part!(partition_17, "part-17.json");
part!(partition_18, "part-18.json");
part!(partition_19, "part-19.json");
part!(partition_20, "part-20.json");
part!(partition_21, "part-21.json");
part!(partition_22, "part-22.json");
part!(partition_23, "part-23.json");
part!(normalize_negative_zero_minimal, "minimal.json");
