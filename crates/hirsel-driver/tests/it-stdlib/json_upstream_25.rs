//! Frozen serde_json expectations, with direct Value literals and four real engines.
use crate::source_std::load_program_source;
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

// Expectations enter as constructors, never through json.parse.
fn tree(value: &Value) -> String {
    match value[0].as_str().unwrap() {
        "chain" => format!("chain({})", value[1]),
        "null" => "Null()".into(),
        "bool" => format!("Bool({})", value[1]),
        "number" => format!("Number({})", literal(value[1].as_str().unwrap())),
        "text" => format!("Text({})", literal(value[1].as_str().unwrap())),
        "array" => format!(
            "Array([{}])",
            value[1]
                .as_array()
                .unwrap()
                .iter()
                .map(tree)
                .collect::<Vec<_>>()
                .join(",")
        ),
        "object" => format!(
            "object([{}])",
            value[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| format!(
                    "({}, {})",
                    literal(pair[0].as_str().unwrap()),
                    tree(&pair[1])
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        kind => panic!("unknown frozen tree kind {kind}"),
    }
}

const PRELUDE: &str = r#"
fn chain(depth: u64) -> std@1/json.Value uses {} {
  let mut value: std@1/json.Value = Number("0")
  for _ in 0u64..depth { value = Array([take value]) }
  take value
}
fn object(take entries: List[(String, std@1/json.Value)]) -> std@1/json.Value uses {} {
  let mut values: Map[String, std@1/json.Value] = map.empty()
  for (key, value) in entries { map.insert(mut values, copy key, copy value) }
  Object(take values)
}
fn valid(take input: String, expected: std@1/json.Value, compact: String) -> () uses {} {
  let Ok(parsed) = std@1/json.parse(copy input) else { failure.check(false, TextInvariant()); return () }
  failure.check(parsed == expected, TextInvariant())
  let Ok(written) = std@1/json.stringify(expected) else { failure.check(false, TextInvariant()); return () }
  failure.check(written == compact, TextInvariant())
  let Ok(again) = std@1/json.parse(copy written) else { failure.check(false, TextInvariant()); return () }
  failure.check(again == expected, TextInvariant())
  let Ok(parsed_written) = std@1/json.stringify(parsed) else { failure.check(false, TextInvariant()); return () }
  failure.check(parsed_written == compact, TextInvariant())
  let mut builder: List[u8] = [112,114,101,58]
  let alias = copy builder
  failure.check(std@1/json.stringify_into(expected, mut builder) == Ok(()), TextInvariant())
  failure.check(alias == [112,114,101,58], TextInvariant())
  failure.check(text.finish(take builder) == Ok(string.concat("pre:", compact)), TextInvariant())
}
pub fn main(rt: Io, args: Args) -> Result[(), OutputError] {
  let Ok(sink) = io.stdout(rt) else { failure.check(false, TextInvariant()); return Ok(()) }
"#;

fn program(rows: &[&Value]) -> String {
    let mut source = PRELUDE.to_string();
    for row in rows {
        let input = literal(row["input"].as_str().unwrap());
        let expected = &row["expected"];
        if row["operation"] == "write" {
            let result = if expected["error"].is_null() {
                "Ok(())"
            } else {
                "Error(InvalidNumber())"
            };
            writeln!(source, "  {{ let value: std@1/json.Value = Array([Text(\"owned\"), Number({input})]); let mut output: List[u8] = [112,114,101,58]; let alias = copy output; failure.check(std@1/json.stringify_into(value, mut output) == {result}, TextInvariant()); failure.check(alias == [112,114,101,58], TextInvariant()); failure.check(text.finish(take output) == Ok({}), TextInvariant());", literal(expected["prefix"].as_str().unwrap())).unwrap();
            if expected["error"].is_null() {
                writeln!(
                    source,
                    "    failure.check(std@1/json.stringify(value) == Ok({}), TextInvariant()) }}",
                    literal(&expected["prefix"].as_str().unwrap()[4..])
                )
                .unwrap();
            } else {
                source.push_str("    failure.check(std@1/json.stringify(value) == Error(InvalidNumber()), TextInvariant()) }\n");
            }
        } else if let Some(error) = expected.get("error") {
            writeln!(
                source,
                "  failure.check(std@1/json.parse({input}) == Error({}({})), TextInvariant())",
                error[0].as_str().unwrap(),
                error[1]
            )
            .unwrap();
        } else {
            writeln!(
                source,
                "  valid({input}, {}, {})",
                tree(&expected["tree"]),
                literal(expected["compact"].as_str().unwrap())
            )
            .unwrap();
        }
        if let Some(depth) = row["writer_depth"].as_u64() {
            // Construct the writer boundary independently, including rejected parse 129.
            writeln!(source, "  {{ let mut value: std@1/json.Value = Number(\"0\"); for _ in 0u64..{depth} {{ value = Array([take value]) }}").unwrap();
            if depth > 128 {
                writeln!(source, "    failure.check(std@1/json.stringify(value) == Error(Nesting(128)), TextInvariant()); let mut output: List[u8] = [112,114,101,58]; failure.check(std@1/json.stringify_into(value, mut output) == Error(Nesting(128)), TextInvariant()); failure.check(text.finish(take output) == Ok({}), TextInvariant()) }}", literal(&format!("pre:{}", "[".repeat(128)))).unwrap();
            } else {
                writeln!(source, "    failure.check(std@1/json.stringify(value) == Ok({input}), TextInvariant()) }}").unwrap();
            }
        }
        writeln!(
            source,
            "  out.print(sink, {})?",
            literal(&format!("{}\n", row["id"].as_str().unwrap()))
        )
        .unwrap();
    }
    source.push_str("  Ok(())\n}\n");
    source
}

fn run_partition(prefix: &str, start: usize, end: usize) {
    with_compiler_stack(|| {
        let vectors: Vec<Value> =
            serde_json::from_str(include_str!("../fixtures/json-upstream-25/vectors.json"))
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
                "JSON25 selected {} input={} expected={}",
                row["id"], row["input_sha256"], row["expected_sha256"]
            );
        }
        let source = program(&selected);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-corpus-run.hir");
        let mut loaded =
            load_program_source(&path, Some(&source)).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| source_error(e));
        let snapshot = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        let expected = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: selected
                .iter()
                .flat_map(|r| format!("{}\n", r["id"].as_str().unwrap()).into_bytes())
                .collect(),
            failures: vec![],
            roots: vec![],
        };
        eprintln!("JSON25 started reference count={}", selected.len());
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
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "reference: {:?}",
            run.outcome
        );
        assert_eq!(
            native::Observation::of_reference(run.observation.as_ref().unwrap()),
            expected,
            "reference independent oracle"
        );
        eprintln!("JSON25 passed reference completed={}", selected.len());
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
            .prefix("json-upstream-25-")
            .tempdir()
            .unwrap();
        let mut toolchain =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations)
                .unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        for config in configurations {
            eprintln!("JSON25 started {} count={}", config.name(), selected.len());
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
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{engine}: {outcome:?}"
                );
                eprintln!("JSON25 passed {engine} completed={}", selected.len());
            }
        }
    });
}

macro_rules! partition {
    ($name:ident, $prefix:literal, $start:literal, $end:literal) => {
        #[test]
        fn $name() {
            run_partition($prefix, $start, $end);
        }
    };
}
partition!(trees_000, "tree-", 0, 8);
partition!(trees_008, "tree-", 8, 16);
partition!(mutations_000, "mutation-", 0, 16);
partition!(mutations_016, "mutation-", 16, 32);
partition!(mutations_032, "mutation-", 32, 48);
partition!(mutations_048, "mutation-", 48, 64);
partition!(upstream_numbers, "upstream-number-", 0, 16);
partition!(upstream_strings, "upstream-string-", 0, 11);
partition!(number_boundaries, "number-boundary-", 0, 13);
partition!(all_escapes, "all-escapes", 0, 1);
partition!(all_controls, "all-controls", 0, 1);
partition!(object_order, "order-multibyte", 0, 1);
partition!(nested_order, "nested-order", 0, 1);
partition!(scalar_extremes, "scalar-extremes", 0, 1);
partition!(surrogate_errors, "surrogate-", 0, 3);
partition!(invalid_escape, "invalid-escape", 0, 1);
partition!(incomplete_escape, "incomplete-escape", 0, 1);
partition!(raw_control, "raw-control", 0, 1);
partition!(trailing_input, "trailing-multibyte", 0, 1);
partition!(leading_zero, "leading-zero", 0, 1);
partition!(duplicate_keys, "duplicate-", 0, 2);
partition!(depth_127, "depth-127", 0, 1);
partition!(depth_128, "depth-128", 0, 1);
partition!(depth_129, "depth-129", 0, 1);
partition!(writer_numbers, "writer-number-", 0, 11);
