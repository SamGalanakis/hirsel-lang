//! Go encoding/binary/varint_test.go:75-227 at
//! ebe2feefc400f813aefdc8a036e66d2619da6649 (BSD-3-Clause).
//! Independent u128 division/remainder oracle; no signed overflow or shifts.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{fmt::Write, path::PathBuf};

fn literal(bytes: &[u8]) -> String {
    let mut text = String::from("b\"");
    for byte in bytes {
        write!(text, "\\x{byte:02x}").unwrap();
    }
    text.push('"');
    text
}

fn check(source: &mut String, expression: &str) {
    writeln!(source, "  failure.check({expression}, TextInvariant())").unwrap();
}

fn execute(source: &str) {
    with_compiler_stack(|| {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-varint-7-record-run.hir");
        let mut loaded = load_program_source(&fixture, Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(5_000_000),
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let observed = run_observed_snapshot(
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
                observed.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            observed.outcome
        );
        let observation = observed.observation.unwrap();
        assert!(observation.output.is_empty());
        // Successful typed truncation handling must never raise SEM Bounds or Arithmetic.
    });
}

fn wire(mut value: u128) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 128 {
        out.push(u8::try_from(value % 128 + 128).unwrap());
        value /= 128;
    }
    out.push(u8::try_from(value).unwrap());
    out
}

fn vector(source: &mut String, ty: &str, value: i128, bytes: &[u8]) {
    check(
        source,
        &format!(
            "varint.read_{ty}({}, 0) == Ok(({value}{ty}, {}))",
            literal(bytes),
            bytes.len()
        ),
    );
    let mut framed = vec![0xa5, 0x55];
    framed.extend(bytes);
    framed.extend([0xff, 0x33]);
    check(
        source,
        &format!(
            "varint.read_{ty}({}, 2) == Ok(({value}{ty}, {}))",
            literal(&framed),
            bytes.len() + 2
        ),
    );
    for end in 0..bytes.len() {
        check(
            source,
            &format!(
                "varint.read_{ty}({}, 2) == Error(Truncated({}))",
                literal(&framed[..end + 2]),
                end + 2
            ),
        );
    }
    writeln!(source, "  {{ let mut out: unique List[u8] = [165u8, 85u8]\n    varint.append_{ty}({value}{ty}, mut out)").unwrap();
    check(
        source,
        &format!(
            "bytes.from_list(take out) == {}",
            literal(&framed[..bytes.len() + 2])
        ),
    );
    source.push_str("  }\n");
}

#[test]
fn unsigned_radix_oracle() {
    let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
    let mut values = vec![0, 1, 127, 128, 255, 256, u64::MAX - 40, u64::MAX];
    for n in 1..=9 {
        values.extend([(1u64 << (7 * n)) - 1, 1u64 << (7 * n)]);
    }
    values.sort_unstable();
    values.dedup();
    for value in values {
        vector(
            &mut source,
            "u64",
            i128::from(value),
            &wire(u128::from(value)),
        );
    }
    source.push_str("  Ok(())\n}\n");
    execute(&source);
}

#[test]
fn signed_radix_oracle() {
    let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
    let mut values = vec![
        i64::MIN,
        i64::MIN + 1,
        -1,
        0,
        1,
        2,
        10,
        20,
        63,
        64,
        65,
        127,
        128,
        129,
        255,
        256,
        257,
        i64::MAX,
    ];
    for n in 1..=9 {
        values.push(1i64 << (7 * n - 1));
    }
    values.extend(values.clone().into_iter().filter_map(i64::checked_neg));
    values.sort_unstable();
    values.dedup();
    for value in values {
        let wide = i128::from(value);
        let encoded = if wide >= 0 { 2 * wide } else { -2 * wide - 1 };
        vector(
            &mut source,
            "i64",
            wide,
            &wire(u128::try_from(encoded).unwrap()),
        );
    }
    source.push_str("  Ok(())\n}\n");
    execute(&source);
}

#[test]
fn every_tenth_byte_and_suffix() {
    let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
    for byte in 0..=255u8 {
        let mut data = vec![0xa5];
        data.extend([0x80; 9]);
        data.push(byte);
        for suffix in [vec![], vec![0], vec![0xff, 1]] {
            let mut input = data.clone();
            input.extend(suffix);
            for ty in ["u64", "i64"] {
                let expected = if byte > 1 {
                    "Error(Overflow(10))".to_string()
                } else if byte == 0 {
                    format!("Ok((0{ty}, 11))")
                } else if ty == "u64" {
                    "Ok((9223372036854775808u64, 11))".to_string()
                } else {
                    "Ok((4611686018427387904i64, 11))".to_string()
                };
                check(
                    &mut source,
                    &format!("varint.read_{ty}({}, 1) == {expected}", literal(&input)),
                );
            }
        }
    }
    source.push_str("  Ok(())\n}\n");
    execute(&source);
}
