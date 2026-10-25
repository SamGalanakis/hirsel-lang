//! Pinned TOML 1.1 oracle fixtures, projected independently onto all four engines.
use hirsel_driver::{native, *};
use hirsel_engine::*;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/toml-upstream-25")
        .join(name)
}

// Hirsel string syntax differs from JSON for Unicode escapes. Byte-for-byte
// source literals are emitted from scalars; expected data never enters TOML.
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
        let rows: Vec<Value> =
            serde_json::from_str(include_str!("../fixtures/toml-upstream-25/vectors.json"))
                .unwrap();
        assert_eq!(rows.len(), 176);
        assert!(start < end && end <= rows.len());
        let mut source = include_str!("../fixtures/toml-upstream-25/probe.hir").to_owned();
        source.push_str(
            "\npub fn main(rt: Io) -> Result[(), OutputError] {\n let mut out: List[u8] = []\n",
        );
        let mut expected = String::new();
        for row in &rows[start..end] {
            let id = row["id"].as_str().unwrap();
            let text = row["input"].as_str().unwrap();
            let moment = match row["moment"].as_str() {
                Some("fraction") => 1,
                Some("leap") => 2,
                None => 0,
                _ => unreachable!(),
            };
            source.push_str(&format!(
                "observe({}, {}, {moment}, mut out)\n",
                literal(id),
                literal(text)
            ));
            expected.push_str(row["expected_output"].as_str().unwrap());
        }
        source.push_str("out.write(io.stdout(rt) ? output_grant, bytes.from_list(take out))?\nOk(())\n}\nfn output_grant(e: GrantError) -> OutputError { OutputFault(1) }\n");
        eprintln!("TOML_SELECTED {}", serde_json::to_string(&rows[start..end].iter().map(|r| serde_json::json!({"id":r["id"],"input":r["input_sha256"],"expected":r["expected_sha256"],"expected_output":r["expected_output_sha256"],"operations":r["operations"]})).collect::<Vec<_>>()).unwrap());
        let mut loaded = load_program_source(&fixture("probe.hir"), Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("source-check: {e}"));
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        eprintln!(
            "TOML_IDENTITY compiler={:?} llvm={} source={:?} input={:?}",
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
        let observed = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let mut failures = vec![];
        eprintln!(
            "TOML_CELL reference {start}..{end} exit={:?} output={}",
            observed.exit_code,
            String::from_utf8_lossy(&observed.output)
        );
        if observed != oracle {
            failures.push(format!("reference: expected {oracle:?}, got {observed:?}"));
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
            eprintln!("TOML_CELL {engine} {start}..{end} {result:?}");
            if !matches!(result, native::Outcome::Agree) {
                failures.push(format!("{engine}: {result:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        eprintln!(
            "TOML_COMPLETED ids={} engines=4 operations={}",
            end - start,
            rows[start..end]
                .iter()
                .map(|r| r["operations"].as_array().unwrap().len())
                .sum::<usize>()
        );
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
batch!(generated_00, 0, 8);
batch!(generated_08, 8, 16);
batch!(mutations_00, 96, 112);
batch!(mutations_16, 112, 128);
batch!(mutations_32, 128, 144);
batch!(mutations_48, 144, 160);
batch!(upstream, 160, 162);
batch!(scalars, 162, 168);
batch!(ownership, 168, 174);
batch!(depth128, 174, 175);
batch!(depth129, 175, 176);
