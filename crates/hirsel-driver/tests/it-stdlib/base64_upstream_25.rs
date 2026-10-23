//! Independent strict Go expectations, including documented Hirsel policies.
use hirsel_driver::{native::*, *};
use hirsel_engine::*;
use serde_json::Value;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

const VECTORS: &str = include_str!("../fixtures/base64-upstream-25/vectors.json");
const CHECKS: &str = include_str!("../fixtures/base64-upstream-25/checks.hir");

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn literal(data: &[u8], byte_string: bool) -> String {
    let mut out = if byte_string {
        "b\"".to_string()
    } else {
        "\"".to_string()
    };
    if byte_string {
        for byte in data {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    } else {
        for c in std::str::from_utf8(data).unwrap().chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                c => out.push(c),
            }
        }
    }
    out.push('"');
    out
}

fn partition(group: &str) {
    with_compiler_stack(|| {
        let rows: Vec<Value> = serde_json::from_str(VECTORS).unwrap();
        let selected: Vec<_> = rows.iter().filter(|row| row["group"] == group).collect();
        assert!(!selected.is_empty(), "empty partition {group}");
        let mut source = format!("{CHECKS}\npub fn main(t: Test) -> Result[(), ()] {{\n");
        let mut output = Vec::new();
        let mut ids = Vec::new();
        for row in &selected {
            let id = row["id"].as_str().unwrap();
            let mode = row["mode"].as_u64().unwrap();
            let raw = bytes(row["raw_hex"].as_str().unwrap());
            let input = bytes(row["input_hex"].as_str().unwrap());
            let encode = row["kind"] == "valid";
            let prefix = bytes(row["expected"]["stream_prefix_hex"].as_str().unwrap());
            let fail_at = row["expected"]["stream_fail_at"].as_u64().unwrap();
            let terminal = input.iter().position(|b| *b == b'=').unwrap_or(input.len()) / 4 * 3;
            let short = !encode || raw.len() <= 24;
            let expected = if let Some(hex) = row["expected"]["ok_hex"].as_str() {
                format!("Ok({})", literal(&bytes(hex), true))
            } else {
                format!(
                    "Error({}({}))",
                    row["expected"]["error"].as_str().unwrap(),
                    row["expected"]["offset"].as_u64().unwrap()
                )
            };
            source.push_str(&format!(
                "vector(t, \"{id}\", {}, {}, {}(), {}(), {expected}, {}, {terminal}, {fail_at}, {encode}, {short})\n",
                literal(&raw, true),
                literal(&input, false),
                if mode < 2 { "Standard" } else { "UrlSafe" },
                if mode % 2 == 0 { "Required" } else { "Omitted" },
                literal(&prefix, true)
            ));
            let mut record = |suffix: String| {
                let id = format!("{id}/{suffix}");
                output.extend_from_slice(format!("test log: {id}\n").as_bytes());
                ids.push(id);
            };
            record("whole-decode".into());
            if encode {
                record("whole-encode".into());
            }
            if short {
                for split in 0..=input.len() {
                    record(format!("stream-decode/split-{split}"));
                }
                if encode {
                    for split in 0..=raw.len() {
                        record(format!("stream-encode/split-{split}"));
                    }
                }
            } else {
                for size in [1, 3, 7, 16] {
                    record(format!("stream-decode/chunk-{size}"));
                    if encode {
                        record(format!("stream-encode/chunk-{size}"));
                    }
                }
            }
        }
        source.push_str("Ok(())\n}\n");
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output,
            failures: vec![],
            roots: vec![],
        };
        let directory =
            std::env::temp_dir().join(format!("base64-upstream-25-{}-{group}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("main.hir");
        std::fs::write(&path, &source).unwrap();
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":std_manifest,"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(
            path.with_extension("hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        eprintln!(
            "BASE64_SELECTED {group} rows={} operations={} ids={ids:?} compiler={COMPILER_IDENTITY:?}",
            selected.len(),
            ids.len()
        );
        let mut loaded = load_program(&path).unwrap_or_else(|error| match error {
            ToolError::Query { error, .. } => panic!("source {group}: {error:?}"),
            error => panic!("source {group}: {error}"),
        });
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("source {group}: {error:?}"),
                error => panic!("source {group}: {error}"),
            });
        // This bounded Rust fixture exceeds a corpus body's budget; no corpus limit changes.
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
                ..Default::default()
            },
            ..Default::default()
        }));
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
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "reference {group}: {reference:?}"
        );
        assert_eq!(
            Observation::of_reference(reference.observation.as_ref().unwrap()),
            expected,
            "reference {group}"
        );
        eprintln!(
            "BASE64_COMPLETED {group} reference operations={}",
            ids.len()
        );
        let matrix: Vec<_> = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let mut toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        let outcomes = differential_in(
            &validated,
            &loaded.entry,
            &expected,
            &matrix,
            &toolchain,
            &directory,
            &Fixture {
                steps: Some(10_000_000),
                ..Default::default()
            },
        );
        let mut errors = Vec::new();
        assert_eq!(outcomes.len(), 3);
        for (engine, result) in outcomes {
            match result {
                Outcome::Agree => {
                    eprintln!("BASE64_COMPLETED {group} {engine} operations={}", ids.len())
                }
                Outcome::Mismatch(actual) => {
                    errors.push(format!("{engine} observation mismatch: {actual:?}"))
                }
                Outcome::Unavailable(error) => {
                    errors.push(format!("{:?}", history::native_error(&engine, error)))
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        std::fs::remove_dir_all(directory).unwrap();
    });
}

macro_rules! partitions {
    ($($name:ident),+ $(,)?) => { $(#[test] fn $name() { partition(stringify!($name)); })+ };
}
partitions!(
    m0_golden,
    m0_short0,
    m0_short1,
    m0_short2,
    m0_long,
    m0_invalid0,
    m0_invalid1,
    m0_invalid2,
    m1_golden,
    m1_short0,
    m1_short1,
    m1_short2,
    m1_long,
    m1_invalid0,
    m1_invalid1,
    m1_invalid2,
    m2_golden,
    m2_short0,
    m2_short1,
    m2_short2,
    m2_long,
    m2_invalid0,
    m2_invalid1,
    m2_invalid2,
    m3_golden,
    m3_short0,
    m3_short1,
    m3_short2,
    m3_long,
    m3_invalid0,
    m3_invalid1,
    m3_invalid2,
);
