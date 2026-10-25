//! Final-position layout and atomic capacity checks. Ryū d2s.c:314-419 and
//! d2s_test.cc:37-105 / f2s_test.cc, commit 4c0618b (Apache-2.0 or BSL-1.0).
//! Adaptations: Hirsel -6/21 thresholds, lowercase e, unsigned positive exponent,
//! signed zero/nonfinite spellings, checked Inline indexing and ownership.
//! f64.txt retains all 332 prior independent vectors; f32.txt adds power neighbors.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn exact_layout_and_atomic_buffers_on_h_llvm_cranelift() {
    with_compiler_stack(|| {
        let mut source = String::new();
        let mut total = 0;
        for (family, uint, fixture) in [
            (
                "f64",
                "u64",
                include_str!("../fixtures/float-render-13/f64.txt"),
            ),
            (
                "f32",
                "u32",
                include_str!("../fixtures/float-render-13/f32.txt"),
            ),
        ] {
            source += &format!(
                r#"
fn check_{family}[n: size](t: Test, value: {family}, expected: String, mut buffer: Inline[u8, n]) -> () {{
  let bytes = core@1/string.as_bytes(expected)
  let length = core@1/bytes.len(bytes)
  let actual = std@1/{family}.format_inline(value, mut buffer)
  if n < length {{
    core@1/test.check(t, actual == Error(BufferTooSmall))
  }} else {{
    core@1/test.check(t, actual == Ok(length))
  }}
  let mut i: u64 = 0
  while i < n {{
    core@1/test.check(t, buffer[i] == if n >= length && i < length {{ bytes[i] }} else {{ 165u8 }})
    i += 1
  }}
}}
"#
            );
            let mut groups: BTreeMap<usize, Vec<(u64, &str)>> = BTreeMap::new();
            for row in fixture.lines().filter(|row| !row.starts_with('#')) {
                let (bits, text) = row.split_once(' ').unwrap();
                let bits = u64::from_str_radix(bits, 16).unwrap();
                if family == "f64" && f64::from_bits(bits).is_finite() {
                    assert_eq!(text.parse::<f64>().unwrap().to_bits(), bits);
                } else if family == "f32" && f32::from_bits(bits as u32).is_finite() {
                    assert_eq!(u64::from(text.parse::<f32>().unwrap().to_bits()), bits);
                }
                groups.entry(text.len()).or_default().push((bits, text));
                total += 1;
            }
            for (length, rows) in groups {
                source += &format!(
                    "fn group_{family}_{length}(t: Test) -> () {{\nlet rows: Array[({uint}, String)] = [\n"
                );
                for (bits, text) in rows {
                    source += &format!("({bits}{uint}, {text:?}),\n");
                }
                source += "]\nfor (bits, expected) in rows {\n";
                source += &format!(
                    "let value = core@1/{family}.from_bits(bits)\ncore@1/test.check(t, std@1/{family}.to_string(value) == expected)\n"
                );
                for (slot, capacity) in [0, length - 1, length, length + 1].into_iter().enumerate()
                {
                    let fill = vec!["165u8"; capacity].join(", ");
                    source += &format!(
                        "let mut b{slot}: Inline[u8, {capacity}] = [{fill}]\ncheck_{family}(t, value, expected, mut b{slot})\n"
                    );
                }
                source += "}\n}\n";
            }
        }
        source += "pub fn main(t: Test) -> Result[(), ()] {\n";
        for family in ["f64", "f32"] {
            let fixture = if family == "f64" {
                include_str!("../fixtures/float-render-13/f64.txt")
            } else {
                include_str!("../fixtures/float-render-13/f32.txt")
            };
            let lengths: std::collections::BTreeSet<_> = fixture
                .lines()
                .filter(|r| !r.starts_with('#'))
                .map(|r| r.split_once(' ').unwrap().1.len())
                .collect();
            for length in lengths {
                source += &format!("group_{family}_{length}(t)\n");
            }
        }
        source += "Ok(())\n}\n";
        check_engines(&source);
        eprintln!(
            "float_render_13: {total} exact vectors, four sentinel capacities per vector; H/LLVM/Cranelift"
        );
    });
}

fn check_engines(source: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
    let mut loaded = load_program_source(&fixture, Some(source)).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e:?}"));
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(100_000_000),
            call_depth: 256,
            source: None,
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
    assert!(
        matches!(
            reference.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{:?}",
        reference.outcome
    );
    let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    let configs = vec![
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
        .prefix("float-render-13-")
        .tempdir()
        .unwrap()
        .keep();
    let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
    for (config, outcome) in
        native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
    {
        assert!(
            matches!(outcome, native::Outcome::Agree),
            "{config}: {outcome:?}; {}",
            dir.display()
        );
    }
}

fn ecmascript_from_ryu(ryu: &str) -> String {
    match ryu {
        "NaN" => return "NaN".into(),
        "Infinity" => return "inf".into(),
        "-Infinity" => return "-inf".into(),
        "0E0" => return "0".into(),
        "-0E0" => return "-0".into(),
        _ => {}
    }
    let (sign, rest) = ryu.strip_prefix('-').map_or(("", ryu), |rest| ("-", rest));
    let (mantissa, exponent) = rest.split_once('E').unwrap();
    let digits = mantissa.replace('.', "");
    let exponent: i32 = exponent.parse().unwrap();
    let count = digits.len() as i32;
    let body = if !(-6..21).contains(&exponent) {
        let tail = if count > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!("{}{tail}e{exponent}", &digits[..1])
    } else if exponent < 0 {
        format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
    } else if count <= exponent + 1 {
        format!("{digits}{}", "0".repeat((exponent + 1 - count) as usize))
    } else {
        let point = (exponent + 1) as usize;
        format!("{}.{}", &digits[..point], &digits[point..])
    };
    format!("{sign}{body}")
}

#[test]
fn existing_float_controls_on_h_llvm_cranelift() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        let mut count = 0;
        for row in include_str!("../../../../native-tests/library/ryu-vectors.txt")
            .lines()
            .filter(|row| !row.starts_with('#'))
        {
            let fields: Vec<_> = row.split_whitespace().collect();
            let family = if fields[0] == "d2s" { "f64" } else { "f32" };
            let uint = if fields[0] == "d2s" { "u64" } else { "u32" };
            let bits = u64::from_str_radix(fields[1], 16).unwrap();
            let expected = ecmascript_from_ryu(fields[2]);
            source += &format!(
                "core@1/test.check(t, std@1/{family}.to_string(core@1/{family}.from_bits({bits}{uint})) == {expected:?})\n"
            );
            count += 1;
        }
        assert_eq!(count, 192);
        let wide: [(u64, usize); 12] = [
            (1, 324),
            (1, 1074),
            (f64::MAX.to_bits(), 0),
            (f64::MAX.to_bits(), 3),
            (f64::MIN_POSITIVE.to_bits(), 1022),
            ((f64::MIN_POSITIVE.to_bits() - 1) | 1 << 63, 1080),
            (0.5f64.to_bits(), 0),
            (1.5f64.to_bits(), 0),
            (2.5f64.to_bits(), 0),
            (0.125f64.to_bits(), 2),
            (9.995f64.to_bits(), 2),
            ((-0.0f64).to_bits(), 1),
        ];
        for (bits, precision) in wide {
            let expected = format!("{:.precision$}", f64::from_bits(bits));
            source += &format!(
                "core@1/test.check(t, f\"{{core@1/f64.from_bits({bits}u64):.{precision}}}\" == \"{expected}\")\n"
            );
        }
        let narrow: [(u32, usize); 4] = [
            (1, 149),
            (f32::MAX.to_bits(), 0),
            (f32::MIN_POSITIVE.to_bits(), 126),
            (0.375f32.to_bits(), 2),
        ];
        for (bits, precision) in narrow {
            let expected = format!("{:.precision$}", f32::from_bits(bits));
            source += &format!(
                "core@1/test.check(t, f\"{{core@1/f32.from_bits({bits}u32):.{precision}}}\" == \"{expected}\")\n"
            );
        }

        source += "Ok(())\n}\n";
        check_engines(&source);
        eprintln!(
            "float_render_13: existing 192 shortest and 16 fixed precision controls; H/LLVM/Cranelift"
        );
    });
}
