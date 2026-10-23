//! Independent expected literals exercise the public regex operations on real std sources.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const VECTORS: &str = include_str!("../fixtures/regex-upstream-25/vectors.json");
const CHECK: &str = include_str!("../fixtures/regex-upstream-25/check.hir");
const WIDTH: usize = 4;

fn literal(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
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

fn span(value: &Value) -> String {
    format!("regex.Span(start: {}, end: {})", value[0], value[1])
}
fn optional_span(value: &Value) -> String {
    if value.is_null() {
        "None()".into()
    } else {
        format!("Some({})", span(value))
    }
}
fn list(value: &Value, item: fn(&Value) -> String) -> String {
    format!(
        "[{}]",
        value
            .as_array()
            .unwrap()
            .iter()
            .map(item)
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn captures(value: &Value) -> String {
    format!("regex.Captures(spans: {})", list(value, optional_span))
}
fn expected(value: &Value) -> String {
    let first_captures = if value["captures"].is_null() {
        "None()".into()
    } else {
        format!("Some({})", captures(&value["captures"]))
    };
    format!(
        "Expected(first: {}, matched: {}, all: {}, captures: {}, all_captures: {}, replacement: {})",
        optional_span(&value["find"]),
        value["is_match"],
        list(&value["all"], span),
        first_captures,
        list(&value["all_captures"], captures),
        literal(value["replacement"].as_str().unwrap())
    )
}

fn fixture(directory: &Path, rows: &[Value], policy: bool, operation: Option<usize>) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let std_root = root.join("stdlib/std@1").canonicalize().unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(std_root.join("hirsel.json")).unwrap()).unwrap();
    // Same bounded source selection as regex_capture_span_17. No implementation
    // is copied or rewritten; calls use only regex's public API in its real module.
    let sources = manifest["sources"].as_array_mut().unwrap();
    sources.retain(|s| {
        s["module"] == json!(["regex"])
            || s["module"] == json!(["ascii"])
            || s["module"] == json!(["internal", "substring"])
    });
    for source in sources.iter_mut() {
        source["path"] = std_root
            .join(source["path"].as_str().unwrap())
            .to_str()
            .unwrap()
            .into();
    }
    let text = std::fs::read_to_string(std_root.join("text.hir")).unwrap();
    std::fs::write(
        directory.join("text.hir"),
        text.split("// Pads the text").next().unwrap(),
    )
    .unwrap();
    sources.push(json!({"path": directory.join("text.hir"), "module": ["text"]}));
    let mut source = CHECK.to_string();
    if let Some(operation) = operation {
        let (name, call, result) = OPERATIONS[operation % 6];
        let work = WORK[operation / 6];
        let call = call.replace("WORK", &work.to_string());
        source.push_str(&format!(
            "\nfn check_one(id: String, pattern: String, src: String, expected: Expected) -> Result[(), Mismatch] uses {{}} {{\n  let Ok(re) = regex.compile(pattern, 10000) else {{ return Error(Mismatch(id: unique id, operation: \"compile\", work: 0)) }}\n  if regex.{call} != Ok(expected.{result}) {{ return Error(Mismatch(id: unique id, operation: \"{name}\", work: {work})) }}\n  Ok(())\n}}\n"
        ));
    }
    if !policy {
        source.push_str("\npub fn main() -> Result[(), Mismatch] uses {} {\n");
        for row in rows {
            let id = literal(row["id"].as_str().unwrap());
            let pattern = literal(row["pattern"].as_str().unwrap());
            let subject = row["subject"].as_str().unwrap();
            if let Some(offset) = row["expected"]["syntax"].as_u64() {
                source.push_str(&format!("  syntax_case({id}, {pattern}, {offset})?\n"));
                continue;
            }
            source.push_str("  {\n");
            if row["sliced"].as_bool().unwrap() {
                source.push_str(&format!("    let backing = {}\n    let Some(start) = core@1/string.position(backing, 2) else {{ return Error(Mismatch(id: {id}, operation: \"slice_start\", work: 0)) }}\n    let Some(end) = core@1/string.position(backing, {}) else {{ return Error(Mismatch(id: {id}, operation: \"slice_end\", work: 0)) }}\n    let subject = core@1/string.slice(backing, start, end)\n", literal(&format!("zz{subject}zz")), subject.len() + 2));
            } else {
                source.push_str(&format!("    let subject = {}\n", literal(subject)));
            }
            let check = if operation.is_some() {
                "check_one"
            } else {
                "check_case"
            };
            source.push_str(&format!(
                "    {check}({id}, {pattern}, subject, {})?\n  }}\n",
                expected(&row["expected"])
            ));
        }
        source.push_str("  Ok(())\n}\n");
    }
    std::fs::write(directory.join("vectors.hir"), source).unwrap();
    sources.push(json!({"path": directory.join("vectors.hir"), "module": ["oracle"]}));
    manifest["entry"] = json!({"module": ["oracle"], "name": if policy {"policy"} else {"main"}});
    let path = directory.join("hirsel.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

fn run(partition: usize, engine: &str) {
    run_selection(partition, engine, None, None);
}

fn run_selection(
    partition: usize,
    engine: &str,
    selected: Option<std::ops::Range<usize>>,
    operation: Option<usize>,
) {
    with_compiler_stack(|| {
        let data: Value = serde_json::from_str(VECTORS).unwrap();
        let all = data["rows"].as_array().unwrap();
        assert_eq!(all.len(), 53, "retain the frozen selected union");
        let policy = partition == 14;
        let rows = if let Some(selected) = selected {
            &all[selected]
        } else if policy {
            &all[0..0]
        } else {
            &all[partition * WIDTH..all.len().min((partition + 1) * WIDTH)]
        };
        for row in rows {
            let input = json!({"pattern": row["pattern"], "subject": row["subject"], "sliced": row["sliced"]});
            for (value, key) in [
                (input, "input_sha256"),
                (row["expected"].clone(), "expected_sha256"),
            ] {
                let bytes = serde_json::to_vec(&value).unwrap();
                assert_eq!(
                    format!("{:x}", Sha256::digest(bytes)),
                    row[key].as_str().unwrap(),
                    "fixture integrity {}",
                    row["id"]
                );
            }
        }
        let ids: Vec<_> = rows.iter().map(|r| json!({"id":r["id"],"input_sha256":r["input_sha256"],"expected_sha256":r["expected_sha256"]})).collect();
        eprintln!(
            "REGEX_UPSTREAM_START engine={engine} partition={partition} ids={ids:?} policy={policy}"
        );
        let temp = tempfile::tempdir().unwrap();
        let path = fixture(temp.path(), rows, policy, operation);
        if let Some(operation) = operation {
            eprintln!(
                "REGEX_UPSTREAM_OPERATION engine={engine} partition={partition} operation={} work={}",
                OPERATIONS[operation % 6].0,
                WORK[operation / 6]
            );
        }
        let source = std::fs::read_to_string(temp.path().join("vectors.hir")).unwrap();
        eprintln!(
            "REGEX_UPSTREAM_FIXTURE {}",
            json!({
                "partition": partition, "engine": engine,
                "source_sha256": format!("{:x}", Sha256::digest(source.as_bytes())),
                "source": source,
            })
        );
        let mut loaded =
            load_program(&path).unwrap_or_else(|e| panic!("source load {engine}: {e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("source rejection {engine}: {e}"));
        let success = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        if engine == "interpreter" || engine == "checked-H" {
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(100_000_000),
                    ..Default::default()
                },
                ..Default::default()
            }));
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let result = if engine == "interpreter" {
                run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
            } else {
                run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
            }
            .unwrap_or_else(|e| panic!("{engine} runtime tool error: {e:?}"));
            assert!(
                matches!(
                    result.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{engine} partition {partition}: {result:?}"
            );
            assert_eq!(
                native::Observation::of_reference(result.observation.as_ref().unwrap()),
                success,
                "{engine} partition {partition}"
            );
        } else {
            let input = loaded
                .engine
                .export_native_input()
                .unwrap_or_else(|e| panic!("V export rejection {engine}: {e:?}"));
            let validated = loaded
                .engine
                .import_native_input(input)
                .unwrap_or_else(|e| panic!("V import rejection {engine}: {e:?}"));
            let configuration = native::Configuration {
                options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                engine: if engine == "LLVM" {
                    native::Engine::Native {
                        opt: native::OptLevel::None,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    }
                } else {
                    native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    }
                },
            };
            let matrix = [configuration];
            let mut toolchain =
                native::Toolchain::for_matrix(&temp.path().join("runtime"), &matrix).unwrap();
            toolchain.launch_timeout = std::time::Duration::from_secs(60);
            std::fs::create_dir_all(temp.path().join("launch")).unwrap();
            let cells = native::differential(
                &validated,
                &loaded.entry,
                &success,
                &matrix,
                &toolchain,
                &temp.path().join("launch"),
            );
            assert_eq!(cells.len(), 1);
            for (name, outcome) in cells {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{engine} {name} partition {partition}: {outcome:?}"
                );
            }
        }
        eprintln!(
            "REGEX_UPSTREAM_PASS engine={engine} partition={partition} pairs={} policy={policy}",
            rows.len()
        );
    });
}

macro_rules! partition {
    ($i:ident, $h:ident, $l:ident, $c:ident, $n:literal) => {
        #[test]
        fn $i() {
            run($n, "interpreter");
        }
        #[test]
        fn $h() {
            run($n, "checked-H");
        }
        #[test]
        fn $l() {
            run($n, "LLVM");
        }
        #[test]
        fn $c() {
            run($n, "Cranelift");
        }
    };
}
partition!(interpreter_00, checked_h_00, llvm_00, cranelift_00, 0);
partition!(interpreter_01, checked_h_01, llvm_01, cranelift_01, 1);
partition!(interpreter_02, checked_h_02, llvm_02, cranelift_02, 2);
#[test]
fn interpreter_03() {
    run(3, "interpreter");
}
#[test]
fn llvm_03() {
    run(3, "LLVM");
}
#[test]
fn cranelift_03() {
    run(3, "Cranelift");
}
partition!(interpreter_04, checked_h_04, llvm_04, cranelift_04, 4);
partition!(interpreter_05, checked_h_05, llvm_05, cranelift_05, 5);
partition!(interpreter_06, checked_h_06, llvm_06, cranelift_06, 6);
partition!(interpreter_07, checked_h_07, llvm_07, cranelift_07, 7);
partition!(interpreter_08, checked_h_08, llvm_08, cranelift_08, 8);
partition!(interpreter_09, checked_h_09, llvm_09, cranelift_09, 9);
partition!(interpreter_10, checked_h_10, llvm_10, cranelift_10, 10);
#[test]
fn interpreter_11() {
    run(11, "interpreter");
}
#[test]
fn llvm_11() {
    run(11, "LLVM");
}
#[test]
fn cranelift_11() {
    run(11, "Cranelift");
}
partition!(interpreter_12, checked_h_12, llvm_12, cranelift_12, 12);
partition!(interpreter_13, checked_h_13, llvm_13, cranelift_13, 13);
partition!(interpreter_14, checked_h_14, llvm_14, cranelift_14, 14);

// Only the two checked-H partitions which exhausted the 60-second process cap
// are split. Their exact ID/operation union and work budgets remain unchanged.
macro_rules! checked_slice {
    ($test:ident, $partition:literal, $start:literal) => {
        #[test]
        fn $test() {
            run_selection($partition, "checked-H", Some($start..$start + 1), None);
        }
    };
}
checked_slice!(checked_h_03_0, 3, 12);

checked_slice!(checked_h_03_2, 3, 14);
checked_slice!(checked_h_03_3, 3, 15);
checked_slice!(checked_h_11_0, 11, 44);
checked_slice!(checked_h_11_1, 11, 45);
checked_slice!(checked_h_11_2, 11, 46);
checked_slice!(checked_h_11_3, 11, 47);

// The documentation word golden alone needs one fixed operation per checked-H
// process. Other checked-H pairs and all other engine cells are retained.
const WORK: [u64; 2] = [1_000_000, u64::MAX];
const OPERATIONS: [(&str, &str, &str); 6] = [
    ("find", "find(re, src, WORK)", "first"),
    ("is_match", "is_match(re, src, WORK)", "matched"),
    ("find_all", "find_all(re, src, 256, WORK)", "all"),
    ("find_captures", "find_captures(re, src, WORK)", "captures"),
    (
        "find_all_captures",
        "find_all_captures(re, src, 256, WORK)",
        "all_captures",
    ),
    (
        "replace_all",
        r#"replace_all(re, src, "<$0>", WORK)"#,
        "replacement",
    ),
];
macro_rules! checked_operation {
    ($test:ident, $operation:literal) => {
        #[test]
        fn $test() {
            run_selection(3, "checked-H", Some(13..14), Some($operation));
        }
    };
}
checked_operation!(checked_h_word_find_finite, 0);
checked_operation!(checked_h_word_find_maximum, 6);
checked_operation!(checked_h_word_is_match_finite, 1);
checked_operation!(checked_h_word_is_match_maximum, 7);
checked_operation!(checked_h_word_find_all_finite, 2);
checked_operation!(checked_h_word_find_all_maximum, 8);
checked_operation!(checked_h_word_find_captures_finite, 3);
checked_operation!(checked_h_word_find_captures_maximum, 9);
checked_operation!(checked_h_word_find_all_captures_finite, 4);
checked_operation!(checked_h_word_find_all_captures_maximum, 10);
checked_operation!(checked_h_word_replace_all_finite, 5);
checked_operation!(checked_h_word_replace_all_maximum, 11);
