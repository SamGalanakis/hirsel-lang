//! IBM decTest's directive/row runner (CPython Lib/test/test_decimal.py),
//! specialized to the finite, unbounded coefficient API. See the vector README.
use hirsel_driver::load_program_source;
use hirsel_engine::with_compiler_stack;
use hirsel_interp::{Integer, Limits, Outcome, Value};
use hirsel_types::Primitive;
use std::path::Path;

fn int(kind: Primitive, bits: u128) -> Value {
    Value::Integer(Integer::new(kind, bits).unwrap())
}

/// decTest accepts exponent notation, leading + and a missing integer/fraction
/// part. Construct coefficient/scale directly; the public parser is stricter.
fn decimal(token: &str) -> Option<(String, i32)> {
    let token = token.trim_matches(['\'', '"']);
    let (mantissa, exponent) = token
        .split_once(['e', 'E'])
        .map_or(Some((token, 0)), |(m, e)| Some((m, e.parse::<i64>().ok()?)))?;
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches(['+', '-']);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let scale = i64::try_from(fraction.len()).ok()? - exponent;
    let coefficient = digits.trim_start_matches('0');
    Some((
        if coefficient.is_empty() {
            "0".into()
        } else if negative {
            format!("-{coefficient}")
        } else {
            coefficient.into()
        },
        i32::try_from(scale).ok()?,
    ))
}

fn suite(text: &str, expected_counts: [usize; 5]) {
    with_compiler_stack(|| {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let mut loaded =
            load_program_source(&fixture, Some(include_str!("decimal-vectors.hir"))).unwrap();
        loaded.check_for_execution().unwrap();
        let runtime = loaded.engine.runtime_types().unwrap();
        let sources = loaded.engine.sources.clone();
        let mut entry = loaded.entry;
        entry.0.name = "check".into();
        let config = hirsel_tir::RunConfiguration {
            entry,
            hash_seed: 0,
            inputs: vec![],
        };
        let mut rounding = "half_even";
        let mut counts = [0usize; 5]; // executed, non-finite/invalid, unsupported rounding, huge expansion, context overflow
        for (line_number, line) in text.lines().enumerate() {
            let line = line.split("--").next().unwrap().trim();
            if let Some(value) = line.strip_prefix("rounding:") {
                rounding = value.trim();
            }
            if !line.contains("->") {
                continue;
            }
            let row: Vec<_> = line.split_whitespace().collect();
            let arrow = row.iter().position(|word| *word == "->").unwrap();
            let op: u64 = match row[1] {
                "add" => 0,
                "subtract" => 1,
                "multiply" => 2,
                "divide" => 3,
                "quantize" => 4,
                "apply" => 0,
                other => panic!("unknown operation {other}"),
            };
            assert!(arrow == 3 || arrow == 4);
            let (Some((a, sa)), Some((b, sb)), Some((e, se))) = (
                decimal(row[2]),
                decimal(if arrow == 3 { "0" } else { row[3] }),
                decimal(row[arrow + 1]),
            ) else {
                counts[1] += 1;
                continue;
            };
            if row.contains(&"Overflow") {
                counts[4] += 1;
                continue;
            }
            let mode: u64 = match rounding {
                "half_even" => 0,
                "half_up" => 1,
                "down" => 2,
                "up" => 3,
                "floor" => 4,
                "ceiling" => 5,
                "half_down" | "05up" if !row.contains(&"Inexact") => 2,
                "half_down" | "05up" => {
                    counts[2] += 1;
                    continue;
                }
                other => panic!("unknown rounding {other}"),
            };
            // IEEE contexts allow billion-place exponents. Do not materialize
            // billion-limb integers in an unbounded arithmetic test.
            let expansion = match op {
                0 | 1 => i64::from(sa)
                    .abs_diff(i64::from(sb))
                    .max(i64::from(sa.max(sb)).abs_diff(i64::from(se))),
                2 => (i64::from(sa) + i64::from(sb)).abs_diff(i64::from(se)),
                3 => (i64::from(se) - i64::from(sa) + i64::from(sb)).unsigned_abs(),
                _ => i64::from(sa).abs_diff(i64::from(sb)),
            };
            if expansion > 10_000 {
                counts[3] += 1;
                continue;
            }
            assert!(op != 3 || b != "0", "finite division by zero: {line}");
            let arguments = vec![
                Value::String(a),
                int(Primitive::I32, u128::from(sa as u32)),
                Value::String(b),
                int(Primitive::I32, u128::from(sb as u32)),
                Value::String(e),
                int(Primitive::I32, u128::from(se as u32)),
                int(Primitive::U64, u128::from(mode)),
                int(Primitive::U64, u128::from(op)),
            ];
            let mut library =
                hirsel_interp::library::Library::new(&config, Default::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut loaded.engine,
                &sources,
                &config,
                arguments,
                Limits {
                    steps: Some(100_000_000),
                    call_depth: 256,
                },
                &mut library,
                &runtime,
            )
            .unwrap();
            assert!(
                matches!(
                    execution.outcome,
                    Outcome::Returned {
                        value: Value::Bool(true),
                        ..
                    }
                ),
                "line {}: {line}\n{:?}",
                line_number + 1,
                execution.outcome
            );
            counts[0] += 1;
        }
        assert_eq!(counts, expected_counts, "decTest coverage changed");
    });
}

#[test]
fn dectest_add() {
    suite(
        include_str!("../../../../native-tests/library/vectors/dectest/add.decTest"),
        [1842, 220, 26, 12, 0],
    );
}
#[test]
fn dectest_subtract() {
    suite(
        include_str!("../../../../native-tests/library/vectors/dectest/subtract.decTest"),
        [581, 98, 0, 0, 2],
    );
}
#[test]
fn dectest_multiply() {
    suite(
        include_str!("../../../../native-tests/library/vectors/dectest/multiply.decTest"),
        [372, 145, 0, 4, 0],
    );
}
#[test]
fn dectest_divide() {
    suite(
        include_str!("../../../../native-tests/library/vectors/dectest/divide.decTest"),
        [450, 180, 1, 0, 0],
    );
}
#[test]
fn dectest_quantize() {
    suite(
        include_str!("../../../../native-tests/library/vectors/dectest/quantize.decTest"),
        [559, 201, 0, 15, 0],
    );
}
