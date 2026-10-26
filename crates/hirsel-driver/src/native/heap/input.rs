//! Checked-H stdin consumes an owned launch stream, shared by projected
//! handles. Cursor supplies bounded reads (Rust std::io::Cursor); H owns the
//! updated List's growth, COW and arena accounting, as for file.read.
use super::*;
use std::io::Read;

impl Io {
    pub(super) fn read_input(&mut self, buffer: &OpValue, max: u128) -> OpOutcome {
        let mut bits = match buffer {
            OpValue::Elements {
                scalar: ScalarKind::U8,
                bits,
            } => bits.clone(),
            OpValue::Bytes(bytes) => bytes.iter().copied().map(u128::from).collect(),
            _ => return OpOutcome::Unsupported,
        };
        let position = self.stdin.position().min(self.stdin.get_ref().len() as u64) as usize;
        let count = max.min((self.stdin.get_ref().len() - position) as u128) as usize;
        let mut bytes = vec![0; count];
        let count = self.stdin.read(&mut bytes).expect("in-memory stdin cursor");
        bits.extend(bytes[..count].iter().copied().map(u128::from));
        OpOutcome::Updated {
            value: io_ok(OpValue::Bits {
                scalar: ScalarKind::U64,
                bits: count as u128,
            }),
            arguments: vec![(
                1,
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
            )],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heap_stdin_aliases_consume_binary_prefixes_and_eof_without_replacing_existing_bytes() {
        let mut io = Io::new();
        io.stdin = std::io::Cursor::new(b"G\0\xffend".to_vec());
        let first = io.grant(IoGrant::Stdin);
        let second = io.grant(IoGrant::Stdin);
        let mut buffer = OpValue::Elements {
            scalar: ScalarKind::U8,
            bits: vec![80],
        };
        for (handle, max, expected_count, expected) in [
            (first, 0, 0, b"P".as_slice()),
            (first, 2, 2, b"PG\0"),
            (second, 9, 4, b"PG\0\xffend"),
            (first, 3, 0, b"PG\0\xffend"),
        ] {
            let outcome = io
                .io(
                    Some("stdin"),
                    "read",
                    &[
                        OpValue::Handle(handle),
                        buffer.clone(),
                        OpValue::Bits {
                            scalar: ScalarKind::U64,
                            bits: max,
                        },
                    ],
                )
                .unwrap();
            if max == 0 {
                let OpOutcome::Value(value) = outcome else {
                    panic!("zero read did not return an error")
                };
                assert_eq!(
                    value,
                    OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Variant {
                            tag: 2,
                            value: Box::new(OpValue::Unit)
                        })
                    }
                );
            } else {
                let OpOutcome::Updated { value, arguments } = outcome else {
                    panic!("input did not update its exclusive buffer")
                };
                assert_eq!(
                    value,
                    io_ok(OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: expected_count
                    })
                );
                assert_eq!(arguments.len(), 1);
                assert_eq!(arguments[0].0, 1);
                buffer = arguments[0].1.clone();
            }
            assert_eq!(
                buffer,
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits: expected.iter().copied().map(u128::from).collect()
                }
            );
        }
        assert_eq!(io.stdin.position(), 6);
    }
}
