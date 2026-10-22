//! Finite construction/composition model for SEM-3, separate from decoder streams.
//! Rust's UTF-8 validator is the external oracle; Go's pinned scalar table is
//! independently checked before any Hirsel source is generated (NOTICES.md).
use hirsel_driver::*;
use hirsel_engine::*;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "tests/fixtures/utf8-construction-model-16/{name}.hir"
    ))
}

#[test]
fn host_exhaustive_one_and_two_bytes() {
    let mut single = 0;
    let mut pair = 0;
    for a in 0..=255u8 {
        let bytes = [a];
        assert_eq!(std::str::from_utf8(&bytes).is_ok(), a < 128);
        single += usize::from(std::str::from_utf8(&bytes).is_ok());
        for b in 0..=255u8 {
            let bytes = [a, b];
            let expected =
                (a < 128 && b < 128) || ((0xc2..=0xdf).contains(&a) && (0x80..=0xbf).contains(&b));
            assert_eq!(std::str::from_utf8(&bytes).is_ok(), expected, "{bytes:x?}");
            pair += usize::from(expected);
        }
    }
    assert_eq!((single, pair), (128, 18_304));
    eprintln!(
        "host oracle: 256 single bytes (128 valid), 65536 pairs (18304 valid); pairs never compiled"
    );
}

struct Case {
    class: &'static str,
    bytes: Vec<u8>,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for byte in 0..=255u8 {
        cases.push(Case {
            class: "single-byte",
            bytes: vec![byte],
        });
    }
    cases.push(Case {
        class: "empty",
        bytes: vec![],
    });
    let table = std::fs::read_to_string(fixture("go-scalars").with_extension("tsv")).unwrap();
    let mut rows = 0;
    for line in table.lines().filter(|line| !line.starts_with('#')) {
        let (scalar, encoded) = line.split_once('\t').unwrap();
        let scalar = u32::from_str_radix(scalar.trim_start_matches("0x"), 16).unwrap();
        let bytes: Vec<_> = encoded
            .split("\\x")
            .skip(1)
            .map(|hex| u8::from_str_radix(hex, 16).unwrap())
            .collect();
        if let Some(ch) = char::from_u32(scalar) {
            assert_eq!(bytes, ch.to_string().as_bytes());
            cases.push(Case {
                class: "pinned-scalar",
                bytes: bytes.clone(),
            });
            for end in 1..bytes.len() {
                assert!(std::str::from_utf8(&bytes[..end]).is_err());
                cases.push(Case {
                    class: "truncated",
                    bytes: bytes[..end].to_vec(),
                });
            }
            let mut surrounding = vec![0, b'A'];
            surrounding.extend(&bytes);
            surrounding.extend([0xef, 0xbf, 0xbd, b'Z']);
            cases.push(Case {
                class: "valid-context",
                bytes: surrounding,
            });
        } else {
            assert!(std::str::from_utf8(&bytes).is_err());
            cases.push(Case {
                class: "surrogate",
                bytes,
            });
        }
        rows += 1;
    }
    assert_eq!(rows, 34);
    // RFC 3629 rejection classes described in Rust validations.rs:128-188 at
    // f6c1107f8be13e48465e12c0fd26e6a39fe18e4f. No validator is ported here.
    let malformed: &[(&str, &[u8])] = &[
        ("overlong", &[0xc0, 0x80]),
        ("overlong", &[0xc1, 0xbf]),
        ("overlong", &[0xe0, 0x80, 0x80]),
        ("overlong", &[0xe0, 0x9f, 0xbf]),
        ("overlong", &[0xf0, 0x80, 0x80, 0x80]),
        ("overlong", &[0xf0, 0x8f, 0xbf, 0xbf]),
        ("above-max", &[0xf4, 0x90, 0x80, 0x80]),
        ("above-max", &[0xf4, 0xbf, 0xbf, 0xbf]),
        ("above-max", &[0xf5, 0x80, 0x80, 0x80]),
        ("above-max", &[0xf7, 0xbf, 0xbf, 0xbf]),
        ("illegal-lead", &[0xf8, 0x88, 0x80, 0x80, 0x80]),
        ("illegal-lead", &[0xfc, 0x84, 0x80, 0x80, 0x80, 0x80]),
        ("continuation", &[0x80, 0xbf]),
        ("continuation", &[b'A', 0x80]),
        ("bad-tail", &[0xc2, 0x7f]),
        ("bad-tail", &[0xdf, 0xc0]),
        ("bad-tail", &[0xe0, 0xa0, 0x7f]),
        ("bad-tail", &[0xe1, 0xc0, 0x80]),
        ("bad-tail", &[0xed, 0x9f, 0xc0]),
        ("bad-tail", &[0xf0, 0x90, 0x80, 0x7f]),
        ("bad-tail", &[0xf1, 0x80, 0xc0, 0x80]),
        ("bad-tail", &[0xf4, 0x8f, 0xbf, 0xff]),
    ];
    for &(class, bytes) in malformed {
        assert!(std::str::from_utf8(bytes).is_err(), "{class}");
        cases.push(Case {
            class,
            bytes: bytes.to_vec(),
        });
        let mut context = vec![0xc2, 0x80, b'A'];
        context.extend(bytes);
        context.extend([b'Z', 0xef, 0xbf, 0xbd]);
        cases.push(Case {
            class: "invalid-context",
            bytes: context,
        });
    }
    assert_eq!(cases.len(), 414);
    assert!(cases.len() <= 512);
    cases
}

fn bytes_literal(bytes: &[u8]) -> String {
    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
    format!("b\"{escaped}\"")
}

fn text_literal(text: &str) -> String {
    let escaped: String = text
        .chars()
        .map(|c| format!("\\u{{{:x}}}", u32::from(c)))
        .collect();
    format!("\"{escaped}\"")
}

fn expected(text: &str) -> String {
    let scalars: Vec<_> = text
        .chars()
        .map(|c| format!("'\\u{{{:x}}}'", u32::from(c)))
        .collect();
    let offsets: Vec<_> = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain([text.len()])
        .collect();
    let mut slices = Vec::new();
    for (i, &start) in offsets.iter().enumerate() {
        for &end in &offsets[i..] {
            slices.push(text_literal(&text[start..end]));
        }
    }
    format!(
        "Expected(data: {}, text: {}, scalars: [{}], offsets: [{}], slices: [{}])",
        bytes_literal(text.as_bytes()),
        text_literal(text),
        scalars.join(", "),
        offsets
            .iter()
            .map(|n| format!("{n}u64"))
            .collect::<Vec<_>>()
            .join(", "),
        slices.join(", ")
    )
}

fn all_engines(name: &str, source: Option<&str>, failure: bool, campaign: bool) {
    let mut loaded = load_program_source(&fixture(name), source).unwrap_or_else(|e| match e {
        ToolError::Query { error, .. } => panic!("{error:?}"),
        e => panic!("{e}"),
    });
    loaded.check_for_execution().unwrap_or_else(|e| match e {
        ToolError::Query { error, .. } => panic!("{error:?}"),
        e => panic!("{e}"),
    });
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(20_000_000),
            call_depth: 256,
        },
        ..Default::default()
    }));
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
    if failure {
        assert!(
            matches!(reference.outcome,
            Ok(RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, .. }, .. })
            if rule.as_str() == "SEM-3"),
            "{name}: {:?}",
            reference.outcome
        );
    } else {
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            reference.outcome
        );
    }
    let observation = reference.observation.as_ref().unwrap();
    assert!(observation.output.is_empty());
    let expected = native::Observation::of_reference(observation);
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    let ready = native::build(&validated, &options).unwrap();
    let sites = native::Sites::new(validated.snapshot());
    let world = native::HeapWorld {
        steps: Some(100_000_000),
        ..Default::default()
    };
    let heap = if campaign {
        native::heap_campaign_in(&ready, &loaded.entry, &sites, &world).unwrap()
    } else {
        native::run_heap_in(&ready, &loaded.entry, &sites, &world).unwrap()
    };
    assert_eq!(heap.observation, expected);
    assert_eq!(heap.counters.live_heap_bytes, 0);
    if campaign {
        assert!(heap.sites > 0);
    }
    eprintln!(
        "{name}: zero live heap bytes, {} allocations, {} failure sites (campaign={campaign})",
        heap.counters.allocations, heap.sites
    );
    let configs = [
        native::Configuration {
            options: options.clone(),
            engine: native::Engine::Native {
                opt: native::OptLevel::Speed,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        },
        native::Configuration {
            options,
            engine: native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        },
    ];
    let dir = tempfile::Builder::new()
        .prefix("utf8-construction-model-16-")
        .tempdir_in(Path::new(env!("CARGO_TARGET_TMPDIR")))
        .unwrap();
    let tools = native::Toolchain::for_matrix(&dir.path().join("runtime"), &configs).unwrap();
    let mut count = 0;
    for (config, outcome) in native::differential(
        &validated,
        &loaded.entry,
        &expected,
        &configs,
        &tools,
        dir.path(),
    ) {
        assert!(
            matches!(outcome, native::Outcome::Agree),
            "{name}: {config}: {outcome:?}"
        );
        count += 1;
    }
    assert_eq!(count, 2);
    eprintln!("{name}: interpreter + checked H + LLVM + Cranelift agree");
}

fn composition(view: &str) {
    with_compiler_stack(|| {
        let cases = cases();
        let mut classes = BTreeMap::new();
        let mut accepted = 0;
        for case in &cases {
            *classes.entry(case.class).or_insert(0) += 1;
            accepted += usize::from(std::str::from_utf8(&case.bytes).is_ok());
        }
        eprintln!(
            "{} generated cases: {accepted} valid, {} invalid; {classes:?}",
            cases.len(),
            cases.len() - accepted
        );
        let helpers = std::fs::read_to_string(fixture("model")).unwrap();
        let views = std::fs::read_to_string(fixture(&format!("{view}-views"))).unwrap();
        for (batch, cases) in cases.chunks(128).enumerate() {
            let mut source = format!("{helpers}\n{views}\npub fn main() -> Result[(), ()] {{\n");
            for case in cases {
                match std::str::from_utf8(&case.bytes) {
                    Ok(text) => writeln!(
                        source,
                        "  accept({}, {})",
                        expected(text),
                        expected(&format!("é{text}𐀀"))
                    )
                    .unwrap(),
                    Err(_) => writeln!(source, "  reject({})", bytes_literal(&case.bytes)).unwrap(),
                }
            }
            source += "  Ok(())\n}\n";
            eprintln!("{view} composition batch {batch}: {} cases", cases.len());
            all_engines("model", Some(&source), false, false);
        }
    });
}

// A known compiler descriptor collision currently prevents scalar and position
// cursors in one native program. Both views check the same 414 oracle inputs,
// in separate program configurations; no additional byte cases are generated.
#[test]
fn bounded_scalar_composition_all_engines() {
    composition("scalar");
}

#[test]
fn bounded_position_composition_all_engines() {
    composition("position");
}

#[test]
fn owned_finishing_and_allocation_failure_cleanup() {
    with_compiler_stack(|| all_engines("ownership", None, false, true));
}

#[test]
fn malformed_finish_valid_reports_text_invariant() {
    with_compiler_stack(|| all_engines("invariant", None, true, true));
}

#[test]
fn existing_wrong_source_and_stale_positions_reject_statically() {
    with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/PROV");
        for (stem, rule) in [
            ("PROV-3-string-slice", "PROV-1"),
            ("PROV-1-move-ends-brand", "PROV-1"),
        ] {
            for suffix in ["ok", "bad"] {
                let path = root.join(format!("{stem}-{suffix}.hir"));
                let mut loaded = load_program(&path).unwrap();
                let result = loaded.check(LoweringPolicy::Reachable).unwrap();
                if suffix == "ok" {
                    assert!(
                        result.diagnostics.is_empty(),
                        "{path:?}: {:?}",
                        result.diagnostics
                    );
                } else {
                    assert!(
                        result.diagnostics.iter().any(|d| d.rule.as_str() == rule),
                        "{path:?}: {:?}",
                        result.diagnostics
                    );
                }
            }
        }
    });
}
