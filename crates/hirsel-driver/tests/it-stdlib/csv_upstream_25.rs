//! Frozen Go strict-reader expectations; csv-core corroborates common valid chunks.
use crate::source_std::load_program_source;
use hirsel_driver::{native, *};
use hirsel_engine::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

fn literal(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn byte_literal(hex: &str) -> String {
    format!(
        "bytes.from_list([{}])",
        bytes(hex)
            .iter()
            .map(|b| format!("{b}u8"))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn strings(fields: &Value) -> String {
    format!(
        "[{}]",
        fields
            .as_array()
            .unwrap()
            .iter()
            .map(|s| literal(s.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn expected(case: &Value) -> String {
    let value = &case["expected"];
    if let Some(rows) = value["rows"].as_array() {
        return format!(
            "Ok([{}])",
            rows.iter().map(strings).collect::<Vec<_>>().join(",")
        );
    }
    let error = &value["error"];
    let kind = error["kind"].as_str().unwrap();
    if let Some(position) = error["position"].as_array() {
        format!(
            "Error({kind}(std@1/csv.Position(start_line: {}, line: {}, column: {})))",
            position[0], position[1], position[2]
        )
    } else {
        format!("Error({kind}())")
    }
}

fn expected_hash(value: &Value) -> String {
    let json = serde_json::to_string(value).unwrap();
    let mut ascii = String::new();
    for c in json.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                ascii.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    format!("{:x}", Sha256::digest(ascii.as_bytes()))
}

fn program(part: usize, only: Option<usize>) -> (String, native::Observation, usize) {
    let manifest: Value =
        serde_json::from_str(include_str!("../fixtures/csv-upstream-25/vectors.json")).unwrap();
    let mut source = include_str!("../fixtures/csv-upstream-25/helpers.hir").to_string();
    source.push_str("\npub fn main(t: Test) -> Result[(), ()] {\n");
    let mut labels = Vec::new();
    let mut add = |label: String, condition: String, source: &mut String| {
        source.push_str(&format!("check(t, {}, {condition})\n", literal(&label)));
        labels.push(label);
    };
    if part < 48 {
        for (index, case) in manifest["cases"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .skip(part * 4)
            .take(4)
        {
            if only.is_some_and(|wanted| wanted != index) {
                continue;
            }
            let id = case["id"].as_str().unwrap();
            let hex = case["hex"].as_str().unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes(hex))),
                case["input_sha256"].as_str().unwrap()
            );
            assert_eq!(
                expected_hash(&case["expected"]),
                case["expected_sha256"].as_str().unwrap()
            );
            eprintln!(
                "csv-vector id={id} input_sha256={} expected_sha256={}",
                case["input_sha256"], case["expected_sha256"]
            );
            let width = case["width"].as_i64().unwrap();
            source.push_str(&format!("{{\nlet config = std@1/csv.Config(delimiter: {}u8, fields_per_record: {}, max_record_bytes: {})\nlet data = {}\n", case["delimiter"], if width < 0 { "None()".into() } else { format!("Some({width})") }, case["limit"], byte_literal(hex)));
            let answer = expected(case);
            if case["string_input"] == true {
                let text = literal(std::str::from_utf8(&bytes(hex)).unwrap());
                add(
                    format!("{id}/parse"),
                    format!("std@1/csv.parse({text}, config) == {answer}"),
                    &mut source,
                );
                add(
                    format!("{id}/borrowed"),
                    format!("borrowed({text}, config) == {answer}"),
                    &mut source,
                );
            }
            for plan in case["chunks"].as_array().unwrap() {
                let sizes = plan["sizes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| format!("{}u64", n.as_u64().unwrap()))
                    .collect::<Vec<_>>()
                    .join(",");
                for (operation, function) in [("feed", "fed"), ("pull", "pulled")] {
                    add(
                        format!("{id}/{operation}/{}", plan["id"].as_str().unwrap()),
                        format!("{function}(data, [{sizes}], config) == {answer}"),
                        &mut source,
                    );
                }
            }
            source.push_str("}\n");
        }
    } else {
        assert_eq!(part, 48);
        for case in manifest["writers"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            assert_eq!(
                expected_hash(&serde_json::json!([
                    case["expected_hex"],
                    case["preserve_hex"]
                ])),
                case["expected_sha256"].as_str().unwrap()
            );
            eprintln!(
                "csv-vector id={id} expected_sha256={}",
                case["expected_sha256"]
            );
            for (mode, function, hex) in [
                (
                    "legacy",
                    "std@1/csv.write_record(fields, 44u8, mut output)",
                    case["expected_hex"].as_str().unwrap(),
                ),
                (
                    "default",
                    "std@1/csv.write_record_with(fields, std@1/csv.default_write_config(), mut output)",
                    case["expected_hex"].as_str().unwrap(),
                ),
                (
                    "preserve",
                    "std@1/csv.write_record_with(fields, std@1/csv.WriteConfig(delimiter: 44u8, preserve_single_empty: true), mut output)",
                    case["preserve_hex"].as_str().unwrap(),
                ),
            ] {
                source.push_str(&format!("{{\nlet fields: List[String] = {}\nlet mut output: List[u8] = []\nlet outcome = {function}\n", strings(&case["fields"])));
                add(
                    format!("{id}/{mode}"),
                    format!(
                        "outcome == Ok(()) && bytes.from_list(take output) == {}",
                        byte_literal(hex)
                    ),
                    &mut source,
                );
                source.push_str("}\n");
            }
        }
    }
    source.push_str("Ok(())\n}\n");
    let output = labels
        .iter()
        .map(|label| format!("test log: {label}\n"))
        .collect::<String>()
        .into_bytes();
    let count = labels.len();
    (
        source,
        native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output,
            failures: vec![],
            roots: vec![],
        },
        count,
    )
}

fn source_error(error: ToolError, source: &str) -> ! {
    eprintln!("fixture source:\n{source}");
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
    panic!("source-rejection: {error}")
}

fn partition(part: usize) {
    with_compiler_stack(|| {
        let (source, expected, selected) = program(part, None);
        eprintln!("csv-compiler identity={COMPILER_IDENTITY:02x?}");
        eprintln!(
            "csv-seal part={part} source_sha256={:x} transcript_sha256={:x}",
            Sha256::digest(source.as_bytes()),
            Sha256::digest(&expected.output)
        );
        if let Some(products) = std::env::var_os("HIRSEL_PRODUCTS") {
            let products = PathBuf::from(products);
            assert!(
                products.is_dir(),
                "ordinary products must be a directory: {}",
                products.display()
            );
            eprintln!("csv-products directory={}", products.display());
        }
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let mut loaded =
            load_program_source(&path, Some(&source)).unwrap_or_else(|e| source_error(e, &source));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| source_error(e, &source));
        let input = loaded
            .engine
            .export_native_input()
            .unwrap_or_else(|e| panic!("V-export rejection: {e:?}"));
        let validated = loaded
            .engine
            .import_native_input(input)
            .unwrap_or_else(|e| panic!("V-import rejection: {e:?}"));
        let mask = std::env::var("HIRSEL_CSV_RETRY_ENGINES").unwrap_or_else(|_| "all".into());
        assert!(
            mask == "all" || mask == "reference",
            "unknown engine selection {mask}"
        );
        eprintln!("csv-engine-selection={mask}");
        let mut failures = Vec::new();
        let references: Vec<_> = if part < 48 {
            (part * 4..part * 4 + 4).map(Some).collect()
        } else {
            vec![None]
        };
        let mut executed = 0;
        let mut reference_passed = true;
        for only in references {
            let (piece, answer, count) = program(part, only);
            eprintln!(
                "csv-reference-seal part={part} only={only:?} source_sha256={:x} transcript_sha256={:x}",
                Sha256::digest(piece.as_bytes()),
                Sha256::digest(&answer.output)
            );
            let mut reference = load_program_source(&path, Some(&piece))
                .unwrap_or_else(|e| source_error(e, &piece));
            reference
                .check_for_execution()
                .unwrap_or_else(|e| source_error(e, &piece));
            let run = run_observed_snapshot(
                &mut reference.engine,
                &RunConfiguration {
                    entry: reference.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            );
            match run {
                Ok(run) => {
                    match run.observation {
                        Some(observation) => {
                            let observed = native::Observation::of_reference(&observation);
                            executed += observed
                                .output
                                .split(|b| *b == b'\n')
                                .filter(|line| {
                                    line.starts_with(b"test log: r")
                                        || line.starts_with(b"test log: w")
                                })
                                .count();
                            eprintln!(
                                "csv-reference-piece part={part} only={only:?} selected={count} passed={}",
                                observed == answer
                            );
                            if observed != answer {
                                reference_passed = false;
                                failures.push(format!("reference only={only:?} observation: {observed:?}; outcome={:?}", run.outcome));
                            }
                        }
                        None => {
                            reference_passed = false;
                            failures.push(format!(
                                "reference only={only:?} tooling/budget failure: {run:?}"
                            ));
                        }
                    }
                }
                Err(e) => {
                    reference_passed = false;
                    failures.push(format!(
                        "reference only={only:?} {:?}: {e:?}",
                        history::tooling(&format!("{e:?}"))
                    ));
                }
            }
        }
        eprintln!(
            "csv-upstream-25 part={part} engine=reference selected={selected} executed={executed} passed={reference_passed}"
        );
        if mask == "reference" {
            assert!(failures.is_empty(), "part={part}\n{}", failures.join("\n"));
            return;
        }
        // Each engine compares directly with the host-built independent expectation.
        // No reference observation is passed to this adapter.
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
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
            options: options.clone(),
            engine,
        })
        .collect::<Vec<_>>();
        let scratch =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("csv-upstream-25-{part}"));
        let mut toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &matrix).unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        let outcomes = native::differential(
            &validated,
            &loaded.entry,
            &expected,
            &matrix,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3, "all required engines must execute");
        for (engine, outcome) in outcomes {
            match outcome {
                native::Outcome::Agree => eprintln!(
                    "csv-upstream-25 part={part} engine={engine} selected={selected} executed={selected} passed=true"
                ),
                native::Outcome::Mismatch(observation) => {
                    let completed = observation
                        .output
                        .split(|b| *b == b'\n')
                        .filter(|line| {
                            line.starts_with(b"test log: r") || line.starts_with(b"test log: w")
                        })
                        .count();
                    eprintln!(
                        "csv-upstream-25 part={part} engine={engine} selected={selected} executed={completed} passed=false category=Observation"
                    );
                    failures.push(format!("{engine} observation: {observation:?}"))
                }
                native::Outcome::Unavailable(error) => {
                    let cell = history::native_error(&engine, error);
                    eprintln!(
                        "csv-upstream-25 part={part} engine={engine} selected={selected} executed=unknown passed=false category={:?}",
                        cell.category
                    );
                    failures.push(format!("{cell:?}"))
                }
            }
        }
        if !failures.is_empty() {
            eprintln!("fixture source:\n{source}");
        }
        assert!(failures.is_empty(), "part={part}\n{}", failures.join("\n"));
    });
}
macro_rules! partitions {
    ($($name:ident = $part:literal),* $(,)?) => { $(#[test] fn $name() { partition($part); })* };
}
partitions!(
    records_000 = 0,
    records_004 = 1,
    records_008 = 2,
    records_012 = 3,
    records_016 = 4,
    records_020 = 5,
    records_024 = 6,
    records_028 = 7,
    records_032 = 8,
    records_036 = 9,
    records_040 = 10,
    records_044 = 11,
    records_048 = 12,
    records_052 = 13,
    records_056 = 14,
    records_060 = 15,
    records_064 = 16,
    records_068 = 17,
    records_072 = 18,
    records_076 = 19,
    records_080 = 20,
    records_084 = 21,
    records_088 = 22,
    records_092 = 23,
    records_096 = 24,
    records_100 = 25,
    records_104 = 26,
    records_108 = 27,
    records_112 = 28,
    records_116 = 29,
    records_120 = 30,
    records_124 = 31,
    mutations_128 = 32,
    mutations_132 = 33,
    mutations_136 = 34,
    mutations_140 = 35,
    mutations_144 = 36,
    mutations_148 = 37,
    mutations_152 = 38,
    mutations_156 = 39,
    mutations_160 = 40,
    mutations_164 = 41,
    mutations_168 = 42,
    mutations_172 = 43,
    mutations_176 = 44,
    mutations_180 = 45,
    mutations_184 = 46,
    mutations_188 = 47,
    writer_policies = 48,
);
