//! Frozen Ciborium Value expectations for the JSON-domain CBOR policy.
//! Each bounded partition runs the same IDs through all four public engines.
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
fn error(e: std@1/binary.DecodeError) -> String {
  match e {
    Truncated(p) => f"Truncated:{p}",
    Syntax(p) => f"Syntax:{p}",
    Unsupported(p) => f"Unsupported:{p}",
    Utf8(p) => f"Utf8:{p}",
    DuplicateKey(p) => f"DuplicateKey:{p}",
    Nesting(p) => f"Nesting:{p}",
    Trailing(p) => f"Trailing:{p}",
    InvalidNumber() => "InvalidNumber",
  }
}
fn view(v: std@1/json.Value) -> String {
  match v {
    Null() => "N;",
    Bool(b) => if b { "B1;" } else { "B0;" },
    Text(s) => {
      let text = std@1/hex.encode(string.as_bytes(s))
      f"T{text};"
    },
    Number(s) => {
      let mut floating = false
      for b in string.as_bytes(s) {
        if b == 46 || b == 101 || b == 69 { floating = true }
      }
      if floating {
        match std@1/f64.parse(s) {
          Ok(n) => { let bits = f64.bits(n)
 f"F{bits};" },
          Error(_) => "invalid float projection",
        }
      } else { f"I{s};" }
    },
    Array(items) => {
      let n = list.len(items)
      let mut out = f"A{n}["
      for child in items { out = std@1/string.concat(out, view(child)) }
      std@1/string.concat(out, "];" )
    },
    Object(entries) => {
      let n = map.len(entries)
      let mut out = f"O{n}["
      for (key, child) in entries {
        out = std@1/string.concat(out, view(Text(key)))
        out = std@1/string.concat(out, view(child))
      }
      std@1/string.concat(out, "];" )
    },
  }
}
fn decode(output: Stdout, id: String, data: Bytes) -> Result[(), AppError] {
  let observed = match std@1/cbor.decode(data) {
    Ok(v) => view(v),
    Error(e) => error(e),
  }
  out.print(output, f"{id}|decode|{observed}\n")?
  Ok(())
}
fn encode(output: Stdout, id: String, value: std@1/json.Value) -> Result[(), AppError] {
  let deterministic = match std@1/cbor.encode(value, true) {
    Ok(data) => std@1/hex.encode(data),
    Error(e) => error(e),
  }
  let ordinary = match std@1/cbor.encode(value, false) {
    Ok(data) => std@1/hex.encode(data),
    Error(e) => error(e),
  }
  out.print(output, f"{id}|deterministic|{deterministic}\n{id}|ordinary|{ordinary}\n")?
  Ok(())
}
fn nested(depth: u64) -> std@1/json.Value {
  let mut v: std@1/json.Value = Number("0")
  let mut i: u64 = 0
  while i < depth { v = Array([take v])
 i += 1 }
  take v
}
pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt)?
"#;

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for scalar in text.chars() {
        match scalar {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => write!(out, "\\u{{{:x}}}", u32::from(c)).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn literal(v: &Value) -> String {
    literal_nested(v, 0)
}

fn literal_nested(v: &Value, map_depth: u32) -> String {
    match v["t"].as_str().unwrap() {
        "null" => "Null()".into(),
        "bool" => format!("Bool({})", v["v"]),
        "int" | "float" => format!("Number({})", quoted(v["v"].as_str().unwrap())),
        "text" => format!("Text({})", quoted(v["v"].as_str().unwrap())),
        "array" => format!(
            "Array([{}])",
            v["v"]
                .as_array()
                .unwrap()
                .iter()
                .map(|child| literal_nested(child, map_depth))
                .collect::<Vec<_>>()
                .join(",")
        ),
        "map" => {
            let name = if map_depth == 0 {
                "entries".into()
            } else {
                format!("entries_{map_depth}")
            };
            let mut text =
                format!("{{ let mut {name}: Map[String, std@1/json.Value] = map.empty()\n");
            for pair in v["v"].as_array().unwrap() {
                writeln!(
                    text,
                    "map.insert(mut {name}, {}, {})",
                    quoted(pair[0].as_str().unwrap()),
                    literal_nested(&pair[1], map_depth + 1)
                )
                .unwrap();
            }
            write!(text, "Object(take {name}) }}").unwrap();
            text
        }
        t => panic!("unknown fixture type {t}"),
    }
}

fn source(rows: &[Value]) -> String {
    let mut source = PRELUDE.to_owned();
    for row in rows {
        let id = row["id"].as_str().unwrap();
        let bytes = row["input"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| format!("\\x{}", std::str::from_utf8(pair).unwrap()))
            .collect::<String>();
        writeln!(source, "decode(output, {id:?}, b\"{bytes}\")?").unwrap();
        if let Some(value) = row.get("value") {
            // Host-built public Value constructors: no CBOR/JSON parser prepares
            // the encoder's input or the external expected results.
            let value = row
                .get("encode_depth")
                .map_or_else(|| literal(value), |depth| format!("nested({depth})"));
            writeln!(source, "encode(output, {id:?}, {value})?").unwrap();
        }
    }
    source.push_str("Ok(())\n}\n");
    source
}

fn describe(error: ToolError) -> String {
    match error {
        ToolError::Query { error, .. } => format!("{error:?}"),
        error => error.to_string(),
    }
}

fn partition(name: &str, file: &str, fixture: &str) {
    with_compiler_stack(|| {
        let manifest: Value =
            serde_json::from_str(include_str!("../fixtures/cbor-upstream-25/manifest.json"))
                .unwrap();
        assert_eq!(
            manifest["files"][file],
            format!("{:x}", Sha256::digest(fixture.as_bytes()))
        );
        let rows: Vec<Value> = serde_json::from_str(fixture).unwrap();
        assert!(!rows.is_empty());
        for row in &rows {
            if let Some(value) = row.get("value") {
                assert_eq!(
                    row["encode_sha256"],
                    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
                );
            }
            let input: Vec<_> = row["input"]
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect();
            assert_eq!(row["input_sha256"], format!("{:x}", Sha256::digest(&input)));
            assert_eq!(
                row["expected_sha256"],
                format!(
                    "{:x}",
                    Sha256::digest(row["output"].as_str().unwrap().as_bytes())
                )
            );
        }
        let expected: String = rows.iter().map(|r| r["output"].as_str().unwrap()).collect();
        let source = source(&rows);
        let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        let scratch = tempfile::tempdir().unwrap();
        if let Some(directory) = std::env::var_os("CBOR_UPSTREAM_25_EVIDENCE") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(Path::new(&directory).join(format!("{name}.hir")), &source).unwrap();
        }
        eprintln!("CBOR_SOURCE {name} {source_hash}");
        for row in &rows {
            eprintln!("CBOR_SELECTED {}", row["id"]);
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let mut loaded = load_program_source(&path, Some(&source))
            .unwrap_or_else(|e| panic!("{name}: source load {}\n{source}", describe(e)));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{name}: source check {}\n{source}", describe(e)));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let hash_seed = if name.ends_with('0') || name == "partition_12" {
            0
        } else {
            2026102514
        };
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed,
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
        .map(|engine| {
            let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            options.launch.hash_seed = hash_seed;
            native::Configuration { options, engine }
        })
        .collect();
        let runner = history::Runner::with_matrix(scratch.path(), matrix).unwrap();
        let report = runner.compare(&validated, &loaded.entry, &reference, name);
        let mut failures = Vec::new();
        for cell in &report.cells {
            let passed = cell.category == history::Category::Observation
                && cell.observation.as_ref().is_some_and(|o| {
                    o.exit_code == Some(0)
                        && o.failures.is_empty()
                        && !o.blocked
                        && o.budget.is_none()
                        && o.signaled.is_none()
                        && o.roots.is_empty()
                });
            eprintln!(
                "CBOR_OBSERVATION {name} {} {:?}",
                cell.engine, cell.observation
            );
            for row in &rows {
                for operation in ["decode", "deterministic", "ordinary"] {
                    if operation != "decode" && row.get("value").is_none() {
                        continue;
                    }
                    let prefix = format!("{}|{operation}|", row["id"].as_str().unwrap());
                    let line = row["output"]
                        .as_str()
                        .unwrap()
                        .split_inclusive('\n')
                        .find(|l| l.starts_with(&prefix))
                        .unwrap();
                    let row_passed = passed
                        && cell.observation.as_ref().is_some_and(|o| {
                            let actual: Vec<_> = o
                                .output
                                .split_inclusive(|b| *b == b'\n')
                                .filter(|l| l.starts_with(prefix.as_bytes()))
                                .collect();
                            actual.len() == 1 && actual[0] == line.as_bytes()
                        });
                    eprintln!(
                        "CBOR_CELL {}",
                        serde_json::json!({"id":row["id"],"op":operation,"engine":cell.engine,"source_sha256":source_hash,"input_sha256":if operation == "decode" {&row["input_sha256"]} else {&row["encode_sha256"]},"expected_sha256":row["expected_sha256"],"hash_seed":hash_seed,"started":true,"completed":cell.category == history::Category::Observation,"passed":row_passed,"category":format!("{:?}",cell.category)})
                    );
                }
            }
            if !passed
                || cell
                    .observation
                    .as_ref()
                    .is_none_or(|o| o.output != expected.as_bytes())
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
                include_str!(concat!("../fixtures/cbor-upstream-25/", $file)),
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
part!(partition_24, "part-24.json");
part!(partition_25, "part-25.json");
part!(partition_26, "part-26.json");
part!(partition_27, "part-27.json");
part!(partition_28, "part-28.json");
part!(partition_29, "part-29.json");
part!(partition_30, "part-30.json");
part!(partition_31, "part-31.json");
part!(partition_32, "part-32.json");
part!(partition_33, "part-33.json");
