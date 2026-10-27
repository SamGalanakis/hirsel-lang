//! Relative intervals through helper returns, independent of UTF-8 construction.
//! oracle.tsv: pinned Rust/Go boundary vector shapes; see NOTICES.md.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::path::PathBuf;

const HELPERS: &str = include_str!("../fixtures/text-view-bounds-26/helpers.hir");
const ORACLE: &str = include_str!("../fixtures/text-view-bounds-26/oracle.tsv");

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/text-view-bounds-26/helpers.hir")
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unhex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}

fn literal(bytes: &[u8], text: bool) -> String {
    if text {
        let escaped: String = std::str::from_utf8(bytes)
            .unwrap()
            .chars()
            .map(|c| format!("\\u{{{:x}}}", u32::from(c)))
            .collect();
        format!("\"{escaped}\"")
    } else {
        let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
        format!("b\"{escaped}\"")
    }
}

fn context(backing: &[u8], depth: usize) -> String {
    let mut source = format!(
        "  let s0 = {}\n  let b0 = string.as_bytes(s0)\n",
        literal(backing, true)
    );
    let mut len = backing.len();
    for d in 1..=depth {
        writeln!(
            source,
            "  let s{d} = text_window(s{}, 1, {})",
            d - 1,
            len - 1
        )
        .unwrap();
        writeln!(
            source,
            "  let b{d} = byte_window(b{}, 1, {})",
            d - 1,
            len - 1
        )
        .unwrap();
        len -= 2;
    }
    let read = if depth == 0 { "copy " } else { "" };
    writeln!(source, "  let s = {read}s{depth}\n  let b = b{depth}").unwrap();
    source
}

fn valid(expected: &str) -> String {
    let mut source = String::new();
    let mut scalars = Vec::new();
    for token in expected.split(';').filter(|t| !t.is_empty()) {
        let (operation, value) = token.split_once('=').unwrap();
        if operation == "c" {
            scalars.push(format!("'\\u{{{:x}}}'", value.parse::<u32>().unwrap()));
        } else if let Some(offset) = operation.strip_prefix('p') {
            writeln!(source, "  check_position(s, {offset}u64, {value})").unwrap();
        } else {
            let (a, b) = operation[1..].split_once(':').unwrap();
            let text = operation.starts_with('s');
            let (function, input) = if text {
                ("text_window", "s")
            } else {
                ("byte_window", "b")
            };
            writeln!(
                source,
                "  failure.check({function}({input}, {a}, {b}) == {}, TextInvariant())",
                literal(&unhex(value), text)
            )
            .unwrap();
            if text {
                let length = unhex(value).len();
                writeln!(source, "  check_position(text_window(s, {a}, {b}), 0, true)\n  check_position(text_window(s, {a}, {b}), {length}, true)\n  check_position(text_window(s, {a}, {b}), {}, false)", length + 1).unwrap();
            } else {
                let bytes = unhex(value);
                for (i, byte) in bytes.iter().enumerate() {
                    writeln!(source, "  failure.check(byte_at(byte_window(b, {a}, {b}), {i}) == {byte}u8, TextInvariant())").unwrap();
                }
                writeln!(
                    source,
                    "  failure.check(bytes.len(byte_window(b, {a}, {b})) == {}, TextInvariant())",
                    bytes.len()
                )
                .unwrap();
            }
        }
    }
    writeln!(source, "  let expected: List[char] = [{}]\n  let mut index: u64 = 0\n  for ch in string.scalars(s) {{\n    failure.check(ch == expected[index], TextInvariant())\n    index += 1\n  }}\n  failure.check(index == list.len(expected), TextInvariant())\n  failure.check(string.as_bytes(s) == b, TextInvariant())", scalars.join(", ")).unwrap();
    source
}

fn observation(source: &str, entry: &DefPath, kind: &str, expected: &str) -> native::Observation {
    let mut oracle = native::Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    if kind != "valid" {
        let (rule, _) = expected.split_once(':').unwrap();
        let (name, operation) = match kind {
            "str_reverse" => ("text_window", "string.slice(s, a, b)"),
            "index" => ("byte_at", "s[i]"),
            _ => ("byte_window", "bytes.slice(s, start, end)"),
        };
        let offset = source.find(operation).unwrap();
        let mut definition = entry.clone();
        definition.0.name = name.into();
        oracle.exit_code = None;
        oracle.failures.push(native::Failure {
            rule: rule.into(),
            definition,
            file: fixture()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            line: source[..offset].bytes().filter(|b| *b == b'\n').count() as u32 + 1,
            column: source[..offset].rsplit('\n').next().unwrap().len() as u32 + 1,
            operation: None,
        });
    }
    oracle
}

fn partition(text: usize, depth: usize) {
    with_compiler_stack(|| {
        let rows: Vec<_> = ORACLE
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect();
        assert_eq!(rows.len(), 96);
        let rows = &rows[(text * 4 + depth) * 6..(text * 4 + depth + 1) * 6];
        let mut source = HELPERS.to_owned();
        for row in rows {
            let fields: Vec<_> = row.split('\t').collect();
            let [id, backing, _, kind, expected] = fields[..] else {
                panic!("{row}")
            };
            writeln!(source, "\npub fn {id}() -> Result[(), ()] {{").unwrap();
            source += &context(&unhex(backing), depth);
            if kind == "valid" {
                source += &valid(expected);
            } else {
                let args: Vec<_> = expected.split(':').collect();
                let (a, b) = (args[1], args[2]);
                let call = match kind {
                    "index" => format!("byte_at(b, {a}u64)"),
                    "str_reverse" => format!("text_window(s, {a}u64, {b}u64)"),
                    _ => format!("byte_window(b, {a}u64, {b}u64)"),
                };
                writeln!(source, "  let _ = {call}").unwrap();
            }
            source += "  Ok(())\n}\n";
        }
        source += "\npub fn main() -> Result[(), ()] { Ok(()) }\n";
        let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("source check: {error:?}"),
            e => panic!("source check: {e}"),
        });
        let input = loaded.engine.export_native_input().expect("export");
        let validated = loaded
            .engine
            .import_native_input(input)
            .expect("independent V");
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
        .map(|engine| native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        });
        let scratch = tempfile::tempdir().unwrap();
        let mut tools = native::Toolchain::for_matrix(scratch.path(), &configurations).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        let mut failures = Vec::new();
        let only = std::env::var("HIRSEL_TEXT_VIEW_ONLY").ok();
        let reference_only = std::env::var_os("HIRSEL_TEXT_VIEW_REFERENCE_ONLY").is_some();
        let mut executed = 0;
        for row in rows {
            let fields: Vec<_> = row.split('\t').collect();
            let [id, _, _, kind, expected] = fields[..] else {
                unreachable!()
            };
            if only.as_ref().is_some_and(|selected| selected != id) {
                continue;
            }
            executed += 1;
            let mut entry = loaded.entry.clone();
            entry.0.name = id.into();
            let oracle = observation(&source, &entry, kind, expected);
            eprintln!(
                "TEXT_SELECTED {}",
                serde_json::json!({"id":id,"source":hash(source.as_bytes()),"expected":hash(expected.as_bytes()),"observation":oracle.render()})
            );
            eprintln!("TEXT_STARTED {id} reference");
            let result = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            );
            let observed = result
                .as_ref()
                .ok()
                .and_then(|run| run.observation.as_ref())
                .map(native::Observation::of_reference);
            let passed = observed.as_ref() == Some(&oracle);
            eprintln!(
                "TEXT_RECEIPT {}",
                serde_json::json!({"id":id,"engine":"reference","source":hash(source.as_bytes()),"expected":hash(expected.as_bytes()),"passed":passed,"observed":observed.as_ref().map(native::Observation::render)})
            );
            if !passed {
                failures.push(format!(
                    "{id} reference: expected {oracle:?}, got {result:?}"
                ));
            }
            if reference_only {
                continue;
            }
            eprintln!("TEXT_STARTED {id} H/LLVM/Cranelift");
            let results = native::differential_in(
                &validated,
                &entry,
                &oracle,
                &configurations,
                &tools,
                scratch.path(),
                &native::Fixture {
                    steps: Some(1_000_000),
                    ..Default::default()
                },
            );
            assert_eq!(results.len(), 3);
            for (engine, result) in results {
                let passed = matches!(result, native::Outcome::Agree);
                eprintln!(
                    "TEXT_RECEIPT {}",
                    serde_json::json!({"id":id,"engine":engine,"source":hash(source.as_bytes()),"expected":hash(expected.as_bytes()),"passed":passed,"observed":format!("{result:?}")})
                );
                if !passed {
                    failures.push(format!("{id} {engine}: {result:?}"));
                }
            }
        }
        assert!(executed > 0, "zero selected text-view histories");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

macro_rules! partitions {
    ($($name:ident: ($text:expr, $depth:expr)),* $(,)?) => {$(
        #[test]
        fn $name() { partition($text, $depth); }
    )*};
}
partitions! {
    ascii_d0: (0,0), ascii_d1: (0,1), ascii_d2: (0,2), ascii_d3: (0,3),
    two_d0: (1,0), two_d1: (1,1), two_d2: (1,2), two_d3: (1,3),
    three_d0: (2,0), three_d1: (2,1), three_d2: (2,2), three_d3: (2,3),
    four_d0: (3,0), four_d1: (3,1), four_d2: (3,2), four_d3: (3,3),
}

#[test]
fn helper_positions_reject_wrong_slice_and_numeric_brands() {
    with_compiler_stack(|| {
        for (id, body) in [
            (
                "wrong-slice",
                "fn bad(s: String, p: StrPos of s) -> () {\nlet view = text_window(s, 0, 0)\nlet _ = string.slice(view, p, p)\n}\n",
            ),
            (
                "numeric-offset",
                "fn bad(s: String, p: StrPos of s) -> () {\nlet offset = string.byte_offset(s, p)\nlet _ = string.slice(s, offset, offset)\n}\n",
            ),
            (
                "forged-position",
                "fn bad(s: String) -> () {\nlet p = StrPos(0)\nlet _ = string.slice(s, p, p)\n}\n",
            ),
        ] {
            let source = format!("{HELPERS}\n{body}\npub fn main() -> () {{}}\n");
            let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
            let checked = loaded.check(LoweringPolicy::Reachable).unwrap();
            eprintln!("TEXT_STATIC {id} checker {:?}", checked.diagnostics);
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                "{id}: expected rejection"
            );
            if id == "wrong-slice" {
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "PROV-1")
                );
            }
        }
    });
}

#[test]
fn independent_v_rejects_wrong_helper_receiver() {
    with_compiler_stack(|| {
        let source = "fn use_position(s: String, other: String, p: StrPos of s) -> () {\nlet _ = string.byte_offset(s, p)\n}\npub fn main() -> () {}\n";
        let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut path = loaded.entry.clone();
        path.0.name = "use_position".into();
        let mut definition = input.definitions[&path].as_ref().clone();
        let good = hirsel_validate::validate(&definition, &input.types, &signatures, &mut vec![]);
        assert!(good.violations.is_empty(), "{good:?}");
        let mut changed = 0;
        for body in &mut definition.bodies {
            for block in &body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &block.terminator
                {
                    if path.is_core("string.byte_offset") {
                        let receiver = arguments[0].value.place;
                        body.places[receiver.0 as usize].root = body.parameters[1];
                        changed += 1;
                    }
                }
            }
        }
        assert_eq!(changed, 1);
        let bad = hirsel_validate::validate(&definition, &input.types, &signatures, &mut vec![]);
        eprintln!(
            "TEXT_STATIC wrong-helper-receiver independent-V {:?}",
            bad.violations
        );
        assert!(
            !bad.violations.is_empty(),
            "forged receiver acquired a valid brand"
        );
    });
}

#[test]
fn index_expression_failure_site() {
    with_compiler_stack(|| {
        // Smallest source form retaining the original interpreter disagreement:
        // a failed read used as a function's tail expression.
        let source = "fn at() -> u8 {\n  let b = b\"\"\n  b[0]\n}\npub fn main() -> Result[(), ()] { let _ = at(); Ok(()) }\n";
        let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let result = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = native::Observation::of_reference(result.observation.as_ref().unwrap());
        eprintln!("TEXT_MINIMAL {}", observation.render());
        assert_eq!(observation.failures.len(), 1);
        assert_eq!(observation.failures[0].rule, "MEM-14");
        assert_eq!(
            (observation.failures[0].line, observation.failures[0].column),
            (3, 3)
        );
    });
}
