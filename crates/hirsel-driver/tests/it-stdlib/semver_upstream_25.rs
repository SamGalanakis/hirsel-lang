//! Pinned semver oracle fixtures, projected independently onto all four engines.
use hirsel_driver::{native, *};
use hirsel_engine::*;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/semver-upstream-25")
        .join(name)
}

// Hirsel string syntax differs from JSON for Unicode escapes. Byte-for-byte
// source literals are emitted from scalars; expected data never enters semver.
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
            serde_json::from_str(include_str!("../fixtures/semver-upstream-25/vectors.json"))
                .unwrap();
        assert_eq!(rows.len(), 224);
        assert!(start < end && end <= rows.len());
        let mut source = include_str!("../fixtures/semver-upstream-25/probe.hir").to_owned();
        source.push_str(
            "\npub fn main(rt: Io) -> Result[(), OutputError] {\n let mut out: List[u8] = []\n",
        );
        let mut expected = String::new();
        for row in &rows[start..end] {
            let id = row["id"].as_str().unwrap();
            let dialect = match row["dialect"].as_str().unwrap() {
                "cargo" => 0,
                "npm" => 1,
                "order" => 2,
                _ => unreachable!(),
            };
            source.push_str(&format!(
                "observe({}, {}, {}, {dialect}, mut out)\n",
                literal(id),
                literal(row["a"].as_str().unwrap()),
                literal(row["b"].as_str().unwrap())
            ));
            expected.push_str(&format!("{id}|{}\n", row["expected"].as_str().unwrap()));
        }
        source.push_str("out.write(io.stdout(rt) ? output_grant, bytes.from_list(take out))?\nOk(())\n}\nfn output_grant(e: GrantError) -> OutputError { OutputFault(1) }\n");
        eprintln!("SEMVER_SELECTED {}", serde_json::to_string(&rows[start..end].iter().map(|r| serde_json::json!({"id":r["id"],"input":r["input_sha256"],"expected":r["expected_sha256"],"operation":r["operation"]})).collect::<Vec<_>>()).unwrap());
        let mut loaded = load_program_source(&fixture("probe.hir"), Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("source-check: {e}"));
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        eprintln!(
            "SEMVER_IDENTITY compiler={:?} llvm={} source={:?} input={:?}",
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
            "SEMVER_CELL reference {start}..{end} exit={:?} output={}",
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
            eprintln!("SEMVER_CELL {engine} {start}..{end} {result:?}");
            if !matches!(result, native::Outcome::Agree) {
                failures.push(format!("{engine}: {result:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        eprintln!("SEMVER_COMPLETED ids={} engines=4", end - start);
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
batch!(cargo_00, 0, 8);
batch!(cargo_08, 8, 16);
batch!(cargo_16, 16, 24);
batch!(cargo_24, 24, 32);
batch!(cargo_32, 32, 40);
batch!(cargo_40, 40, 48);
batch!(cargo_48, 48, 56);
batch!(cargo_56, 56, 64);
batch!(cargo_64, 64, 72);
batch!(cargo_72, 72, 80);
batch!(cargo_80, 80, 88);
batch!(cargo_88, 88, 96);
batch!(npm_00, 96, 104);
batch!(npm_08, 104, 112);
batch!(npm_16, 112, 120);
batch!(npm_24, 120, 128);
batch!(npm_32, 128, 136);
batch!(npm_40, 136, 144);
batch!(npm_48, 144, 152);
batch!(npm_56, 152, 160);
batch!(npm_64, 160, 168);
batch!(npm_72, 168, 176);
batch!(npm_80, 176, 184);
batch!(npm_88, 184, 192);
batch!(order_00, 192, 200);
batch!(order_08, 200, 208);
batch!(order_16, 208, 216);
batch!(order_24, 216, 224);
