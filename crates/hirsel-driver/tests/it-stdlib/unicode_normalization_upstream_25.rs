//! Unicode16 independent expectations on reference, checked H, LLVM and Cranelift.
use hirsel_driver::{native::*, *};
use hirsel_engine::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

const VECTORS: &str = include_str!("../fixtures/unicode-normalization-upstream-25/vectors.json");
const PROBE: &str = include_str!("../fixtures/unicode-normalization-upstream-25/probe.hir");
const OPERATIONS: [&str; 12] = [
    "nfc",
    "nfd",
    "nfkc",
    "nfkd",
    "idem-nfc",
    "idem-nfd",
    "idem-nfkc",
    "idem-nfkd",
    "nfc-nfd",
    "nfd-nfc",
    "nfkc-nfkd",
    "nfkd-nfkc",
];

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn literal(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// One retained long ASCII-prefix witness needs separate reference invocations
// per operation under the unchanged step limit. Its native batch is unchanged.
const SPLIT_REFERENCE_ID: &str = "ascii-tail-255";

fn partition(group: &str, offset: usize, reference_only: bool, split_reference: bool) {
    with_compiler_stack(|| {
        let rows: Vec<Value> = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(rows.len(), 219);
        let unique: std::collections::BTreeSet<_> =
            rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(unique.len(), 219);
        let count = match group {
            "ascii" => 32,
            "upstream" => 11,
            "official" => 40,
            "slice" => 8,
            _ => 16,
        };
        assert_eq!(
            rows.iter().filter(|r| r["partition"] == group).count(),
            count
        );
        assert!(offset < count && offset % 8 == 0);
        let selected: Vec<_> = rows
            .iter()
            .filter(|r| r["partition"] == group)
            .skip(offset)
            .take(8)
            .filter(|r| !reference_only || (r["id"] == SPLIT_REFERENCE_ID) == split_reference)
            .collect();
        assert!(!selected.is_empty(), "empty {group}/{offset}");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let mut source = format!(
            "{PROBE}\npub fn main(rt: Io) -> Result[(), OutputError] {{\nlet mut output: List[u8] = []\n"
        );
        let mut output = Vec::new();
        let mut ids = Vec::new();
        let mut cases = Vec::new();
        for row in &selected {
            let source_start = source.len();
            let output_start = output.len();
            let id = row["id"].as_str().unwrap();
            let input = bytes(row["input_hex"].as_str().unwrap());
            assert_eq!(input, row["input"].as_str().unwrap().as_bytes());
            assert_eq!(
                format!("{:x}", Sha256::digest(&input)),
                row["input_sha256"].as_str().unwrap()
            );
            assert_eq!(row["disposition"], "identical");
            assert_eq!(row["expected_hex"], row["raw_output_hex"]);
            if let Some(owner) = row["container"].as_str() {
                source.push_str(&format!(
                    "observe_slice({}, {}, {}, {}, mut output)\n",
                    literal(id),
                    literal(owner),
                    row["start"],
                    row["end"]
                ));
            } else {
                source.push_str(&format!(
                    "observe({}, {}, mut output)\n",
                    literal(id),
                    literal(std::str::from_utf8(&input).unwrap())
                ));
            }
            for (index, op) in OPERATIONS.iter().enumerate() {
                let value = bytes(row["expected_hex"][index % 4].as_str().unwrap());
                assert_eq!(
                    format!("{:x}", Sha256::digest(&value)),
                    row["expected_sha256"][index % 4].as_str().unwrap()
                );
                output.extend_from_slice(format!("{id}/{op} {}:", value.len()).as_bytes());
                output.extend_from_slice(&value);
                output.push(b'\n');
                ids.push(format!("{id}/{op}"));
            }
            if split_reference {
                let input_literal = literal(std::str::from_utf8(&input).unwrap());
                let functions = ["nfc", "nfd", "nfkc", "nfkd"];
                for (index, op) in OPERATIONS.iter().enumerate() {
                    let form = index % 4;
                    let normalize = |f: usize, argument: &str| {
                        format!("std@1/unicode.normalize_{}({argument})", functions[f])
                    };
                    let expression = match index / 4 {
                        0 => normalize(form, &input_literal),
                        1 => normalize(form, &normalize(form, &input_literal)),
                        2 => normalize(form, &normalize(form ^ 1, &input_literal)),
                        _ => unreachable!(),
                    };
                    let value = bytes(row["expected_hex"][form].as_str().unwrap());
                    let mut single_output = format!("{id}/{op} {}:", value.len()).into_bytes();
                    single_output.extend_from_slice(&value);
                    single_output.push(b'\n');
                    cases.push((
                        format!(
                            "emit({}, {}, {expression}, mut output)\n",
                            literal(id),
                            literal(op)
                        ),
                        single_output,
                        format!("{id}/{op}"),
                    ));
                }
            } else {
                cases.push((
                    source[source_start..].to_string(),
                    output[output_start..].to_vec(),
                    id.to_string(),
                ));
            }
        }
        source.push_str("out.write(io.stdout(rt) ? grant_error, bytes.from_list(take output))?\nOk(())\n}\nfn grant_error(e: GrantError) -> OutputError { OutputFault(1) }\n");
        if reference_only {
            for (index, (call, _, _)) in cases.iter().enumerate() {
                source.push_str(&format!("\npub fn case_{index}(rt: Io) -> Result[(), OutputError] {{\nlet mut output: List[u8] = []\n{call}out.write(io.stdout(rt) ? grant_error, bytes.from_list(take output))?\nOk(())\n}}\n"));
            }
        }
        eprintln!("UNICODE_SOURCE {group}/{offset}\n{source}\nUNICODE_SOURCE_END");
        std::fs::write(&path, &source).unwrap();
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(path.with_extension("hir.json"), serde_json::to_vec(&serde_json::json!({
            "schema":1,"package":{"name":"standalone","version":"0"},
            "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":std_manifest,"package":{"name":"std","version":"1"},"expose":[]}]
        })).unwrap()).unwrap();
        eprintln!("UNICODE_SELECTED {group}/{offset} {}", serde_json::to_string(&selected.iter().map(|r| serde_json::json!({
            "id":r["id"],"input":r["input_sha256"],"expected":r["expected_sha256"],"operations":OPERATIONS
        })).collect::<Vec<_>>()).unwrap());
        let mut loaded = load_program(&path).unwrap_or_else(|e| panic!("source load: {e:?}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("source check: {e:?}"));
        // This is a bounded Rust harness, not a corpus budget increase.
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
                ..Default::default()
            },
            ..Default::default()
        }));
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        eprintln!(
            "UNICODE_IDENTITY compiler={COMPILER_IDENTITY:?} llvm={} image={:?} source={:?} input={:?}",
            native::llvm_version(),
            loaded.engine.selected_image(),
            hirsel_syntax::ContentHash::of_bytes(source.as_bytes()),
            native::input_identity(validated.snapshot())
        );
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output,
            failures: vec![],
            roots: vec![],
        };
        let mut errors = Vec::new();
        if reference_only {
            for (index, (_, expected_output, id)) in cases.iter().enumerate() {
                let mut entry = loaded.entry.clone();
                entry.0.name = format!("case_{index}");
                let oracle = Observation {
                    output: expected_output.clone(),
                    ..expected.clone()
                };
                eprintln!("UNICODE_STARTED reference {group}/{offset} id={id}");
                match run_observed_snapshot(
                    &mut loaded.engine,
                    &RunConfiguration {
                        entry,
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    SimulationConfig::default(),
                ) {
                    Ok(run) => match run.observation.as_ref() {
                        Some(observed) => {
                            let actual = Observation::of_reference(observed);
                            eprintln!(
                                "UNICODE_CELL reference {group}/{offset} id={id} outcome={:?} actual={actual:?}",
                                run.outcome
                            );
                            if actual != oracle {
                                errors.push(format!(
                                    "reference {id} expected={oracle:?} actual={actual:?}"
                                ));
                            } else {
                                eprintln!("UNICODE_PASSED reference {group}/{offset} id={id}");
                            }
                        }
                        None => errors.push(format!("reference {id} tooling/budget: {run:?}")),
                    },
                    Err(e) => errors.push(format!(
                        "reference {id} {:?}: {e:?}",
                        history::tooling(&format!("{e:?}"))
                    )),
                }
            }
            assert!(errors.is_empty(), "{}", errors.join("\n"));
            eprintln!(
                "UNICODE_COMPLETED {group}/{offset} ids={} operations={} engines=1",
                selected.len(),
                ids.len()
            );
            return;
        }
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
        let mut toolchain =
            Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        for configuration in &matrix {
            let engine = configuration.name();
            eprintln!("UNICODE_STARTED {engine} {group}/{offset} ids={ids:?}");
            let results = differential_in(
                &validated,
                &loaded.entry,
                &expected,
                std::slice::from_ref(configuration),
                &toolchain,
                directory.path(),
                &Fixture {
                    steps: Some(10_000_000),
                    ..Default::default()
                },
            );
            assert_eq!(results.len(), 1);
            for (engine, result) in results {
                eprintln!("UNICODE_CELL {engine} {group}/{offset} {result:?}");
                match result {
                    Outcome::Agree => {
                        eprintln!("UNICODE_PASSED {engine} {group}/{offset} ids={ids:?}")
                    }
                    Outcome::Mismatch(actual) => {
                        errors.push(format!("{engine} expected={expected:?} actual={actual:?}"))
                    }
                    Outcome::Unavailable(error) => {
                        errors.push(format!("{:?}", history::native_error(&engine, error)))
                    }
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        eprintln!(
            "UNICODE_COMPLETED {group}/{offset} ids={} operations={} engines=3",
            selected.len(),
            ids.len()
        );
    });
}

macro_rules! batch {
    ($name:ident, $reference:ident, $group:literal, $offset:literal) => {
        #[test]
        fn $name() {
            partition($group, $offset, false, false);
        }
        #[test]
        fn $reference() {
            partition($group, $offset, true, false);
        }
    };
}
batch!(canonical_00, canonical_00_reference, "canonical", 0);
batch!(canonical_08, canonical_08_reference, "canonical", 8);
batch!(leading_00, leading_00_reference, "leading", 0);
batch!(leading_08, leading_08_reference, "leading", 8);
batch!(equal_00, equal_00_reference, "equal", 0);
batch!(equal_08, equal_08_reference, "equal", 8);
batch!(decreasing_00, decreasing_00_reference, "decreasing", 0);
batch!(decreasing_08, decreasing_08_reference, "decreasing", 8);
batch!(exclusion_00, exclusion_00_reference, "exclusion", 0);
batch!(exclusion_08, exclusion_08_reference, "exclusion", 8);
batch!(hangul_00, hangul_00_reference, "hangul", 0);
batch!(hangul_08, hangul_08_reference, "hangul", 8);
batch!(
    compatibility_00,
    compatibility_00_reference,
    "compatibility",
    0
);
batch!(
    compatibility_08,
    compatibility_08_reference,
    "compatibility",
    8
);
batch!(mixed_00, mixed_00_reference, "mixed", 0);
batch!(mixed_08, mixed_08_reference, "mixed", 8);
batch!(ascii_00, ascii_00_reference, "ascii", 0);
batch!(ascii_08, ascii_08_reference, "ascii", 8);
batch!(ascii_16, ascii_16_reference, "ascii", 16);
batch!(ascii_24, ascii_24_reference, "ascii", 24);
batch!(upstream_00, upstream_00_reference, "upstream", 0);
batch!(upstream_08, upstream_08_reference, "upstream", 8);
batch!(official_00, official_00_reference, "official", 0);
batch!(official_08, official_08_reference, "official", 8);
batch!(official_16, official_16_reference, "official", 16);
batch!(official_24, official_24_reference, "official", 24);
batch!(official_32, official_32_reference, "official", 32);
batch!(slices, slices_reference, "slice", 0);

#[test]
fn ascii_tail_255_reference() {
    partition("ascii", 24, true, true);
}
