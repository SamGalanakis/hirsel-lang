//! Independent Rust integer byte conversions generate bounded Hirsel fixtures.
//! Go's unsigned scalar vectors: encoding/binary/binary_test.go:49-56,75-80,93-98,
//! commit ebe2feefc400f813aefdc8a036e66d2619da6649. No roundtrip-only oracle.
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
            .join("../../conformance/SEM/SEM-3-binary-endian-5-packet-run.hir");
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

macro_rules! vectors {
    ($test:ident, $ty:ty, $go:expr) => {
        #[test]
        fn $test() {
            let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
            let width = std::mem::size_of::<$ty>();
            // Go's asymmetric unsigned vector plus min/max and high-bit cases.
            for value in [0, 1, <$ty>::MAX, <$ty>::MAX / 2, $go, $go.reverse_bits()] {
                for (order, bytes) in [
                    ("LittleEndian", value.to_le_bytes()),
                    ("BigEndian", value.to_be_bytes()),
                ] {
                    check(
                        &mut source,
                        &format!(
                            "binary.read_{ty}({}, 0, {order}()) == Ok({value}{ty})",
                            literal(&bytes),
                            ty = stringify!($ty),
                        ),
                    );
                    for prefix in [vec![], vec![0xa5], vec![0xaa, 0, 0x55]] {
                        let mut expected = prefix.clone();
                        expected.extend(bytes);
                        writeln!(
                            source,
                            "  {{\n    let mut out: unique List[u8] = {:?}",
                            prefix
                        )
                        .unwrap();
                        writeln!(
                            source,
                            "    binary.append_{ty}({value}{ty}, {order}(), mut out)",
                            ty = stringify!($ty)
                        )
                        .unwrap();
                        check(
                            &mut source,
                            &format!("bytes.from_list(take out) == {}", literal(&expected)),
                        );
                        source.push_str("  }\n");
                    }
                }
            }
            // Each read oracle uses Rust from_{le,be}_bytes on the actual subrange.
            // Every offset and every short suffix, including empty input and len+1.
            let data: Vec<u8> = (0..24).map(|i| (i * 37 + 128) as u8).collect();
            for length in 0..=data.len() {
                let input = &data[..length];
                for offset in 0..=length + 1 {
                    for order in ["LittleEndian", "BigEndian"] {
                        let expected = if offset <= length && width <= length - offset {
                            let bytes = input[offset..offset + width].try_into().unwrap();
                            let value = if order == "LittleEndian" {
                                <$ty>::from_le_bytes(bytes)
                            } else {
                                <$ty>::from_be_bytes(bytes)
                            };
                            format!("Ok({value}{})", stringify!($ty))
                        } else {
                            format!("Error(binary.ReadError(offset: {offset}))")
                        };
                        check(
                            &mut source,
                            &format!(
                                "binary.read_{}({}, {offset}, {order}()) == {expected}",
                                stringify!($ty),
                                literal(input),
                            ),
                        );
                    }
                }
            }
            source.push_str("  Ok(())\n}\n");
            execute(&source);
        }
    };
}

vectors!(u16_rust_vectors, u16, 0x1112u16);
vectors!(u32_rust_vectors, u32, 0x13141516u32);
vectors!(u64_rust_vectors, u64, 0x1718191a1b1c1d1eu64);
