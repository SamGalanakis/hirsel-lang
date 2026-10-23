//! Pinned Unicode 16 casing fixtures, projected independently onto all four engines.
use hirsel_driver::{native, *};
use hirsel_engine::*;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/unicode-case-upstream-25")
        .join(name)
}

// Hirsel string syntax differs from JSON for Unicode escapes. Byte-for-byte
// source literals are emitted from scalars; expected data never enters casing.
fn literal(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn partition(start: usize, end: usize) {
    with_compiler_stack(|| {
        let rows: Vec<Value> = serde_json::from_str(include_str!(
            "../fixtures/unicode-case-upstream-25/vectors.json"
        ))
        .unwrap();
        assert_eq!(rows.len(), 160);
        assert!(start < end && end <= rows.len());
        let mut source = include_str!("../fixtures/unicode-case-upstream-25/probe.hir").to_owned();
        source.push_str(
            "\npub fn main(rt: Io) -> Result[(), OutputError] {\n let mut out: List[u8] = []\n",
        );
        let mut expected = String::new();
        let mut individual = Vec::new();
        let mut entries = String::new();
        for row in &rows[start..end] {
            let id = row["id"].as_str().unwrap();
            let call_start = source.len();
            let expected_start = expected.len();
            if row["kind"] == "equality" {
                source.push_str(&format!(
                    "equality({}, {}, {}, mut out)\n",
                    literal(id),
                    literal(row["a"].as_str().unwrap()),
                    literal(row["b"].as_str().unwrap())
                ));
            } else if let Some(slice) = row.get("slice") {
                source.push_str(&format!(
                    "observe_slice({}, {}, {}, {}, mut out)\n",
                    literal(id),
                    literal(slice["container"].as_str().unwrap()),
                    slice["start"],
                    slice["end"]
                ));
            } else {
                source.push_str(&format!(
                    "observe({}, {}, mut out)\n",
                    literal(id),
                    literal(row["text"].as_str().unwrap())
                ));
            }
            let call = source[call_start..].to_owned();
            entries.push_str(&format!("\npub fn case_{}(rt: Io) -> Result[(), OutputError] {{\nlet mut out: List[u8] = []\n{call}out.write(io.stdout(rt) ? output_grant, bytes.from_list(take out))?\nOk(())\n}}\n", row["id"].as_str().unwrap().replace('-', "_")));
            let operations: &[&str] = if row["kind"] == "equality" {
                &["eq"]
            } else {
                &["upper", "lower", "casefold"]
            };
            for op in operations {
                let value = row["expected"][op].as_str().unwrap();
                expected.push_str(&format!("{id}|{op}|{}:{value}\n", value.len()));
            }
            individual.push(expected.as_bytes()[expected_start..].to_vec());
        }
        source.push_str("out.write(io.stdout(rt) ? output_grant, bytes.from_list(take out))?\nOk(())\n}\nfn output_grant(e: GrantError) -> OutputError { OutputFault(1) }\n");
        source.push_str(&entries);
        eprintln!("CASING_SELECTED {}", serde_json::to_string(&rows[start..end].iter().map(|r| serde_json::json!({"id":r["id"],"input":r["input_sha256"],"expected":r["expected_sha256"],"operations":r["expected"]})).collect::<Vec<_>>()).unwrap());
        let mut loaded = load_program_source(&fixture("probe.hir"), Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("source-check: {e}"));
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        eprintln!(
            "CASING_IDENTITY compiler={:?} llvm={} source={:?} input={:?}",
            COMPILER_IDENTITY,
            native::llvm_version(),
            hirsel_syntax::ContentHash::of_bytes(source.as_bytes()),
            native::input_identity(validated.snapshot())
        );
        let oracle = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: expected.as_bytes().to_vec(),
            failures: vec![],
            roots: vec![],
        };
        let mut failures = vec![];
        // Each reference invocation keeps the ordinary step budget. The same
        // observe calls run as a four-ID union in the other engines.
        for (row, output) in rows[start..end].iter().zip(individual) {
            let mut entry = loaded.entry.clone();
            entry.0.name = format!("case_{}", row["id"].as_str().unwrap().replace('-', "_"));
            let reference = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry,
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            );
            let mut expected = oracle.clone();
            expected.output = output;
            let observed = reference
                .as_ref()
                .ok()
                .and_then(|run| run.observation.as_ref())
                .map(native::Observation::of_reference);
            let passed = observed.as_ref() == Some(&expected);
            eprintln!("CASING_CELL reference {} result={reference:?}", row["id"]);
            eprintln!(
                "CASING_RECEIPT {}",
                serde_json::json!({"engine":"reference","id":row["id"],"input":row["input_sha256"],"expected":row["expected_sha256"],"passed":passed})
            );
            if !passed {
                failures.push(format!(
                    "reference {}: expected {expected:?}, got {reference:?}",
                    row["id"]
                ));
            }
        }
        let configurations: Vec<_> = [
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
        let scratch = tempfile::tempdir().unwrap();
        let mut toolchain = native::Toolchain::for_matrix(scratch.path(), &configurations).unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        for (engine, result) in native::differential(
            &validated,
            &loaded.entry,
            &oracle,
            &configurations,
            &toolchain,
            scratch.path(),
        ) {
            eprintln!("CASING_CELL {engine} {start}..{end} {result:?}");
            for row in &rows[start..end] {
                eprintln!(
                    "CASING_RECEIPT {}",
                    serde_json::json!({"engine":engine,"id":row["id"],"input":row["input_sha256"],"expected":row["expected_sha256"],"passed": matches!(result, native::Outcome::Agree)})
                );
            }
            if !matches!(result, native::Outcome::Agree) {
                failures.push(format!("{engine}: {result:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        eprintln!("CASING_COMPLETED ids={} engines=4", end - start);
    });
}

macro_rules! batch {
    ($name:ident, $start:literal, $end:literal) => {
        #[test]
        fn $name() {
            partition($start, $end);
        }
    };
}
batch!(casing_000, 0, 4);
batch!(casing_004, 4, 8);
batch!(casing_008, 8, 12);
batch!(casing_012, 12, 16);
batch!(casing_016, 16, 20);
batch!(casing_020, 20, 24);
batch!(casing_024, 24, 28);
batch!(casing_028, 28, 32);
batch!(casing_032, 32, 36);
batch!(casing_036, 36, 40);
batch!(casing_040, 40, 44);
batch!(casing_044, 44, 48);
batch!(casing_048, 48, 52);
batch!(casing_052, 52, 56);
batch!(casing_056, 56, 60);
batch!(casing_060, 60, 64);
batch!(casing_064, 64, 68);
batch!(casing_068, 68, 72);
batch!(casing_072, 72, 76);
batch!(casing_076, 76, 80);
batch!(casing_080, 80, 84);
batch!(casing_084, 84, 88);
batch!(casing_088, 88, 92);
batch!(casing_092, 92, 96);
batch!(casing_096, 96, 100);
batch!(casing_100, 100, 104);
batch!(casing_104, 104, 108);
batch!(casing_108, 108, 112);
batch!(casing_112, 112, 116);
batch!(casing_116, 116, 120);
batch!(casing_120, 120, 124);
batch!(casing_124, 124, 128);
batch!(equality_000, 128, 132);
batch!(equality_004, 132, 136);
batch!(equality_008, 136, 140);
batch!(equality_012, 140, 144);
batch!(equality_016, 144, 148);
batch!(equality_020, 148, 152);
batch!(equality_024, 152, 156);
batch!(equality_028, 156, 160);
