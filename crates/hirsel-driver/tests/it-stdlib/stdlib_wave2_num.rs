//! Independent host text and rational oracles for ordinary numeric std source.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::Path};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir")
}

fn execute(source: &str) {
    let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(100_000_000),
            call_depth: 256,
            source: None,
        },
        ..Default::default()
    }));
    let run = run_observed_snapshot(
        &mut loaded.engine,
        &RunConfiguration {
            entry: loaded.entry,
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    assert!(
        matches!(
            run.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{:?}",
        run.outcome
    );
}

#[test]
fn source_parser_rounds_midpoints_and_unbounded_decimal_suffixes_like_rust() {
    with_compiler_stack(|| {
        let tie = "1.00000000000000011102230246251565404236316680908203125";
        let halfway_f32 = "1.000000059604644775390625";
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        for text in [
            tie.into(),
            format!("{tie}{}1", "0".repeat(1000)),
            halfway_f32.into(),
            format!("{halfway_f32}{}1", "0".repeat(1000)),
            "2.4703282292062327e-324".into(),
            "2.4703282292062328e-324".into(),
            "7.006492321624085e-46".into(),
            "7.006492321624086e-46".into(),
            "-1e-10000".into(),
            "1.7976931348623158e308".into(),
            "1.7976931348623159e308".into(),
            "3.4028235e38".into(),
            "3.4028236e38".into(),
        ] {
            for (family, bits, finite) in [
                {
                    let value: f32 = text.parse().unwrap();
                    ("f32", u64::from(value.to_bits()), value.is_finite())
                },
                {
                    let value: f64 = text.parse().unwrap();
                    ("f64", value.to_bits(), value.is_finite())
                },
            ] {
                if finite {
                    let uint = if family == "f32" { "u32" } else { "u64" };
                    source += &format!(
                        "match std@1/{family}.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, core@1/{family}.bits(value) == {bits}{uint}), Error(_) => core@1/test.check(t, false) }}\n"
                    );
                } else {
                    source += &format!(
                        "core@1/test.check(t, std@1/{family}.parse(\"{text}\") == Error(OutOfRange))\n"
                    );
                }
            }
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

/// Ryū's scientific text (`d.dddE-x`) respelled in std's ECMAScript Number::toString
/// layout, without a `+` in the exponent.
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

/// The significant digits and scientific exponent of `1.25e-3` or `1.25E-3`.
fn scientific_parts(text: &str) -> (String, i32) {
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap();
    (mantissa.replace(['.', '-'], ""), exponent.parse().unwrap())
}

/// Every d2s_test.cc and f2s_test.cc assertion of Ryū (192 vectors): std prints the
/// shortest text Ryū expects, Rust's LowerExp agrees on its digits except on exact
/// ties, and std parses both spellings back to the same bits.
#[test]
fn shortest_text_matches_ryu_d2s_and_f2s_vectors() {
    with_compiler_stack(|| {
        let rows: Vec<_> = include_str!("../../../../native-tests/library/ryu-vectors.txt")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect();
        assert_eq!(rows.len(), 192);
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        for row in rows {
            let fields: Vec<_> = row.split(' ').collect();
            let [test, hex, ryu, _line] = fields[..] else {
                panic!("{row}");
            };
            let bits = u64::from_str_radix(hex, 16).unwrap();
            let (family, uint, lower, exact) = if test == "d2s" {
                let value = f64::from_bits(bits);
                ("f64", "u64", format!("{value:e}"), format!("{value:.800e}"))
            } else {
                let value = f32::from_bits(bits as u32);
                ("f32", "u32", format!("{value:e}"), format!("{value:.200e}"))
            };
            if ryu.contains('E') && !ryu.ends_with("0E0") {
                let (digits, exponent) = scientific_parts(ryu);
                if scientific_parts(&lower) != (digits.clone(), exponent) {
                    // The value is exactly halfway between two shortest candidates:
                    // Rust takes the upper one, Ryū and std the even one.
                    let (expansion, _) = scientific_parts(&exact);
                    let tail = expansion.strip_prefix(&format!("{digits}5"));
                    let last = digits.as_bytes()[digits.len() - 1] - b'0';
                    assert!(
                        last % 2 == 0 && tail.is_some_and(|tail| tail.bytes().all(|b| b == b'0')),
                        "{row}: Rust {lower}"
                    );
                }
            }
            let expected = ecmascript_from_ryu(ryu);
            source += &format!(
                "core@1/test.check(t, std@1/{family}.to_string(core@1/{family}.from_bits({bits}{uint})) == \"{expected}\")\n"
            );
            let mut texts = vec![expected.clone()];
            if ryu.contains('E') {
                texts.push(ryu.into());
            }
            for text in texts {
                source += &format!(
                    "match std@1/{family}.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, core@1/{family}.bits(value) == {bits}{uint}), Error(_) => core@1/test.check(t, false) }}\n"
                );
            }
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

/// Rows `first .. first + 32` of the 256 independent rational vectors, each summed in both
/// orders; eight cases cover them all, so shards run them in parallel.
fn source_exact_sum_matches_rational_vectors_in_both_orders(first: usize) {
    with_compiler_stack(|| {
        let rows: Vec<_> = include_str!("../../../../native-tests/library/exact-sums.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(rows.len(), 256);
        for batch in rows[first..first + 32].chunks(16) {
            let mut source = String::from(
                "pub fn main(t: Test) -> Result[(), ()] {\nlet cases: Array[(Array[f64], u64)] = [\n",
            );
            for line in batch {
                let (input, expected) = line.split_once(" / ").unwrap();
                let mut words: Vec<_> = input.split_whitespace().collect();
                for _ in 0..2 {
                    let items = words
                        .iter()
                        .map(|word| format!("core@1/f64.from_bits(0x{word}u64)"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    source += &format!("([{items}], 0x{expected}u64),\n");
                    words.reverse();
                }
            }
            source += "]\nfor (values, expected) in cases { core@1/test.check(t, core@1/f64.bits(std@1/f64.sum_exact(values)) == expected) }\nOk(())\n}\n";
            execute(&source);
        }
    });
}

#[test]
fn source_exact_sum_matches_rational_vectors_000_to_031_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(0);
}

#[test]
fn source_exact_sum_matches_rational_vectors_032_to_063_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(32);
}

#[test]
fn source_exact_sum_matches_rational_vectors_064_to_095_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(64);
}

#[test]
fn source_exact_sum_matches_rational_vectors_096_to_127_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(96);
}

#[test]
fn source_exact_sum_matches_rational_vectors_128_to_159_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(128);
}

#[test]
fn source_exact_sum_matches_rational_vectors_160_to_191_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(160);
}

#[test]
fn source_exact_sum_matches_rational_vectors_192_to_223_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(192);
}

#[test]
fn source_exact_sum_matches_rational_vectors_224_to_255_in_both_orders() {
    source_exact_sum_matches_rational_vectors_in_both_orders(224);
}

/// Interpreter steps of `std@1/f64.parse(text)` for each text, which no corpus
/// result observes.
fn parse_steps(texts: &[&str]) -> Vec<u64> {
    with_compiler_stack(|| {
        let source = "pub fn parse(text: String) -> bool {\nstd@1/f64.parse(text) != Error(Invalid)\n}\npub fn main() -> () {\nlet _ = parse(\"1\")\n}\n";
        let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let runtime = loaded.engine.runtime_types().unwrap();
        let sources = loaded.engine.sources.clone();
        let mut entry = loaded.entry.clone();
        entry.0.name = "parse".into();
        let configuration = RunConfiguration {
            entry,
            hash_seed: 0,
            inputs: vec![],
        };
        texts
            .iter()
            .map(|text| {
                let mut library =
                    hirsel_interp::library::Library::new(&configuration, Default::default())
                        .unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut loaded.engine,
                    &sources,
                    &configuration,
                    vec![hirsel_interp::Value::String((*text).into())],
                    hirsel_interp::Limits {
                        steps: Some(100_000_000),
                        call_depth: 256,
                        source: None,
                    },
                    &mut library,
                    &runtime,
                )
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        hirsel_interp::Outcome::Returned {
                            value: hirsel_interp::Value::Bool(true),
                            ..
                        }
                    ),
                    "{text}: {:?}",
                    execution.outcome
                );
                execution.steps
            })
            .collect()
    })
}

// Eisel-Lemire decides each of these from the leading digits and a table
// product, well inside a bound linear in the decimal size (significant digits
// plus |e|); one bignum pass per significand bit once took 559k steps for 1e300.
#[test]
fn f64_parse_steps_grow_linearly_with_the_decimal_exponent() {
    let cases = [
        ("1e0", 1u64),
        ("1e30", 31),
        ("1e100", 101),
        ("1e300", 301),
        ("1e-30", 31),
        ("1e-100", 101),
        ("1e-300", 301),
        ("1.7976931348623157e308", 325),
        ("2.2250738585072014e-308", 325),
        ("5e-324", 325),
    ];
    let texts: Vec<_> = cases.iter().map(|(text, _)| *text).collect();
    let all = parse_steps(&texts);
    for ((text, size), steps) in cases.iter().zip(&all) {
        assert!(
            *steps <= 10_000 + 400 * size,
            "{text} took {steps} steps; all: {all:?}"
        );
    }
}

/// `{x:.p}` against Rust's exact fixed formatting (ties to even) at the binary64 and
/// binary32 extremes and on exact ties.
#[test]
fn fixed_precision_matches_rust_at_the_extremes() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
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
        execute(&source);
    });
}

/// `text / 2`, exactly: one more fractional digit.
fn half(text: &str) -> String {
    let mut out = String::new();
    let mut carry = 0;
    for c in format!("{text}0").chars() {
        if c == '.' {
            out.push('.');
            continue;
        }
        let digit = carry * 10 + c.to_digit(10).unwrap();
        out.push(char::from_digit(digit / 2, 10).unwrap());
        carry = digit % 2;
    }
    out
}

// Eisel-Lemire cannot round an exact midpoint between two floats, so these take
// the DecimalSeq path, whose binary shifts each make one pass over at most 768
// digits. The 1 KiB bignum it replaced took about 69k, 5.1M, 5.1M and 5.2M.
#[test]
fn f64_parse_slow_path_steps_stay_bounded() {
    // Between the largest subnormal and the smallest normal: 768 significant digits.
    let normal = half(&format!(
        "{:.1074}",
        f64::from_bits(1) * ((1u64 << 53) - 1) as f64
    ));
    let tie = "1.00000000000000011102230246251565404236316680908203125".to_string();
    let cases = [
        (tie.clone(), 80_000u64),
        (format!("{tie}{}1", "0".repeat(1000)), 800_000),
        (normal.clone(), 2_500_000),
        (format!("{normal}1"), 2_500_000),
    ];
    let texts: Vec<_> = cases.iter().map(|(text, _)| text.as_str()).collect();
    let all = parse_steps(&texts);
    for ((text, limit), steps) in cases.iter().zip(&all) {
        assert!(
            *steps <= *limit,
            "{} digits took {steps} steps; all: {all:?}",
            text.len()
        );
    }
}

#[test]
fn short_decimal_writer_matches_the_independent_inline_writer_for_every_quad() {
    with_compiler_stack(|| {
        execute(
            r#"
pub fn main(t: Test) -> Result[(), ()] {
  let mut buffer: Inline[u8, 6] = [0u8, 0u8, 0u8, 0u8, 0u8, 0u8]
  for n in 0u64..10001 {
    let Ok(count) = u64.format_inline(n, mut buffer) else { return Error(()) }
    let text = u64.to_string(n)
    let data = string.as_bytes(text)
    test.check(t, bytes.len(data) == count)
    for i in 0u64..count { test.check(t, data[i] == buffer[i]) }
    let signed: i64 = -int.narrow(n)
    let Ok(signed_count) = i64.format_inline(signed, mut buffer) else { return Error(()) }
    let signed_text = i64.to_string(signed)
    let signed_data = string.as_bytes(signed_text)
    test.check(t, bytes.len(signed_data) == signed_count)
    for i in 0u64..signed_count { test.check(t, signed_data[i] == buffer[i]) }
  }
  Ok(())
}
"#,
        )
    });
}
