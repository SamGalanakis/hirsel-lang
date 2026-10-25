//! Saphyr event expectations, adapted only by the documented YAML value policies.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::Value;
use std::{fmt::Write, path::PathBuf};

fn source_error(error: ToolError) -> ! {
    if let ToolError::Query {
        error: QueryError::Diagnostics(diagnostics),
        sources,
        ..
    } = &error
    {
        for diagnostic in diagnostics {
            eprintln!("{}", diagnostic.render_human(sources));
        }
    }
    panic!("source rejection: {error}")
}

fn literal(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control()
                || matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                write!(out, "\\u{{{:x}}}", u32::from(c)).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// Host-built Value constructors: expected data never enters yaml.parse.
fn tree(value: &Value) -> String {
    match value[0].as_str().unwrap() {
        "Null" => "Null()".into(),
        "Bool" => format!("Bool({})", value[1]),
        "Integer" => format!("Integer({}i64)", value[1]),
        "Float" => format!("Float({}f64)", value[1]),
        "Text" => format!("Text({})", literal(value[1].as_str().unwrap())),
        "Tagged" => format!(
            "Tagged(tag: {}, value: box {})",
            literal(value[1].as_str().unwrap()),
            tree(&value[2])
        ),
        "Array" => format!(
            "Array([{}])",
            value[1]
                .as_array()
                .unwrap()
                .iter()
                .map(tree)
                .collect::<Vec<_>>()
                .join(",")
        ),
        "Object" => format!(
            "object([{}])",
            value[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| format!("({}, {})", literal(p[0].as_str().unwrap()), tree(&p[1])))
                .collect::<Vec<_>>()
                .join(",")
        ),
        kind => panic!("unknown frozen tree kind {kind}"),
    }
}

const PRELUDE: &str = r#"
fn chain(depth: u64) -> std@1/yaml.Value uses {} {
  let mut value: std@1/yaml.Value = Integer(0)
  for _ in 0u64..depth { value = Array([take value]) }
  take value
}
fn object(take entries: List[(String, std@1/yaml.Value)]) -> std@1/yaml.Value uses {} {
  let mut values: Map[String, std@1/yaml.Value] = map.empty()
  for (key, value) in entries { map.insert(mut values, copy key, copy value) }
  Object(take values)
}
pub fn main(rt: Io) -> Result[(), OutputError] {
  let Ok(sink) = io.stdout(rt) else { failure.check(false, TextInvariant()); return Ok(()) }
"#;

fn program(rows: &[&Value], modes: &[bool]) -> String {
    let mut source = PRELUDE.to_string();
    for row in rows {
        for merge in modes {
            let mode = merge.to_string();
            let expected = &row["expected"][&mode];
            let value = if let Some(error) = expected.get("error") {
                format!(
                    "Error({}(std@1/yaml.Position(line: {}, column: {})))",
                    error.as_str().unwrap(),
                    expected["position"][0],
                    expected["position"][1]
                )
            } else if row["id"] == "c-depth128" {
                "Ok(chain(128))".into()
            } else {
                format!("Ok({})", tree(&expected["value"]))
            };
            // A failed comparison reports its ID and lets all other selected IDs run.
            writeln!(source, "  if std@1/yaml.parse_with({}, std@1/yaml.Config(merge_keys: {merge})) == {value} {{ out.print(sink, {})? }} else {{ out.print(sink, {})? }}", literal(row["input"].as_str().unwrap()), literal(&format!("{}/{}\n",row["id"].as_str().unwrap(),mode)), literal(&format!("FAIL {}/{}\n",row["id"].as_str().unwrap(),mode))).unwrap();
            if let Some(writer) = row.get("writer") {
                let marker = format!("{}/{mode}/stringify\n", row["id"].as_str().unwrap());
                writeln!(source, "  if std@1/yaml.stringify({}) == Ok({}) {{ out.print(sink, {})? }} else {{ out.print(sink, {})? }}", tree(&expected["value"]), literal(writer["text"].as_str().unwrap()), literal(&marker), literal(&format!("FAIL {marker}"))).unwrap();
            }
        }
    }
    source.push_str("  Ok(())\n}\n");
    source
}
fn run_partition(prefix: &str, start: usize, end: usize, modes: &[bool]) {
    with_compiler_stack(|| {
        let vectors: Vec<Value> =
            serde_json::from_str(include_str!("../fixtures/yaml-upstream-25/vectors.json"))
                .unwrap();
        let selected: Vec<_> = vectors
            .iter()
            .filter(|row| row["id"].as_str().unwrap().starts_with(prefix))
            .skip(start)
            .take(end - start)
            .collect();
        assert_eq!(
            selected.len(),
            end - start,
            "partition must retain all selected IDs"
        );
        for row in &selected {
            eprintln!(
                "YAML25 selected {} input={} expected={}",
                row["id"], row["input_sha256"], row["expected_sha256"]
            );
        }
        let source = program(&selected, modes);
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let mut loaded =
            load_program_source(&path, Some(&source)).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| source_error(e));
        eprintln!(
            "YAML25 source={:?} std={:?}",
            hirsel_syntax::ContentHash::of_bytes(source.as_bytes()),
            hirsel_syntax::ContentHash::of_bytes(include_bytes!(
                "../../../../stdlib/std@1/yaml.hir"
            ))
        );
        let snapshot = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        let expected = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: selected
                .iter()
                .flat_map(|r| {
                    modes
                        .iter()
                        .flat_map(|mode| {
                            let id = r["id"].as_str().unwrap();
                            let mut marker = format!("{id}/{mode}\n");
                            if r.get("writer").is_some() {
                                writeln!(marker, "{id}/{mode}/stringify").unwrap();
                            }
                            marker.into_bytes()
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            failures: vec![],
            roots: vec![],
        };
        let engine_filter = std::env::var("YAML_UPSTREAM_25_ENGINE").ok();
        assert!(matches!(
            engine_filter.as_deref(),
            None | Some("reference") | Some("checked-H") | Some("LLVM") | Some("Cranelift")
        ));
        let mut failures = Vec::new();
        if engine_filter.as_deref().is_none_or(|e| e == "reference") {
            eprintln!(
                "YAML25 started reference count={}",
                selected.len() * modes.len()
            );
            let run = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observed = run
                .observation
                .as_ref()
                .map(native::Observation::of_reference);
            if !matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ) || observed.as_ref() != Some(&expected)
            {
                failures.push(format!(
                    "reference outcome={:?} observed={observed:?}",
                    run.outcome
                ));
            } else {
                eprintln!(
                    "YAML25 passed reference completed={}",
                    selected.len() * modes.len()
                );
            }
        }
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = [
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
            options: options.clone(),
            engine,
        })
        .collect::<Vec<_>>();
        let scratch = tempfile::Builder::new()
            .prefix("yaml-upstream-25-")
            .tempdir()
            .unwrap();
        let mut toolchain =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations)
                .unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        for config in configurations {
            let label = match config.engine {
                native::Engine::Heap { .. } => "checked-H",
                native::Engine::Native { .. } => "LLVM",
                native::Engine::Cranelift { .. } => "Cranelift",
            };
            if engine_filter.as_deref().is_some_and(|e| e != label) {
                continue;
            }
            eprintln!(
                "YAML25 started {} count={}",
                config.name(),
                selected.len() * modes.len()
            );
            let outcomes = native::differential(
                &validated,
                &loaded.entry,
                &expected,
                &[config],
                &toolchain,
                scratch.path(),
            );
            assert_eq!(outcomes.len(), 1);
            for (engine, outcome) in outcomes {
                if matches!(outcome, native::Outcome::Agree) {
                    eprintln!(
                        "YAML25 passed {engine} completed={}",
                        selected.len() * modes.len()
                    );
                } else {
                    failures.push(format!("{engine}: {outcome:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

macro_rules! partition {
    ($name:ident, $prefix:literal, $start:literal, $end:literal) => {
        #[test]
        fn $name() {
            run_partition($prefix, $start, $end, &[false, true]);
        }
    };
}
partition!(generated_000, "g", 0, 4);
partition!(generated_004, "g", 4, 8);
partition!(mutations_000, "m", 0, 4);
partition!(mutations_004, "m", 4, 8);
partition!(mutations_008, "m", 8, 12);
partition!(mutations_012, "m", 12, 16);
partition!(mutations_016, "m", 16, 20);
partition!(mutations_020, "m", 20, 24);
partition!(mutations_024, "m", 24, 28);
partition!(mutations_028, "m", 28, 32);
partition!(mutations_032, "m", 32, 36);
partition!(mutations_036, "m", 36, 40);
partition!(mutations_040, "m", 40, 44);
partition!(mutations_044, "m", 44, 48);
partition!(upstream_000, "u-", 0, 4);
partition!(upstream_004, "u-", 4, 7);
partition!(controls_000, "c-", 0, 4);
partition!(controls_004, "c-", 4, 8);
partition!(controls_008, "c-", 8, 11);
partition!(controls_012, "c-", 12, 16);
#[test]
fn depth_128_false() {
    run_partition("c-depth128", 0, 1, &[false]);
}
#[test]
fn depth_128_true() {
    run_partition("c-depth128", 0, 1, &[true]);
}
#[test]
fn depth_129_false() {
    run_partition("c-depth129", 0, 1, &[false]);
}
#[test]
fn depth_129_true() {
    run_partition("c-depth129", 0, 1, &[true]);
}
partition!(alias_minimal, "r-alias-minimal", 0, 1);
partition!(alias_controls, "r-alias-controls-", 0, 4);
partition!(alias_original, "c-anchor-key", 0, 1);
