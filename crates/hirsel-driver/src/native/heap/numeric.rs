//! Independent Heap oracles: a lexical DFA, Rust checked radix conversion and
//! Rust's decimal formatter. These do not call the production numeric kernel.
use super::{OpOutcome, OpValue, ScalarKind};

fn word(value: u64) -> OpValue {
    OpValue::Bits {
        scalar: ScalarKind::U64,
        bits: u128::from(value),
    }
}
fn words(values: impl IntoIterator<Item = u64>) -> OpOutcome {
    OpOutcome::Value(OpValue::Aggregate(values.into_iter().map(word).collect()))
}
fn bits(value: &OpValue) -> Option<u128> {
    if let OpValue::Bits { bits, .. } = value {
        Some(*bits)
    } else {
        None
    }
}

pub(super) fn call(name: &str, args: &[OpValue]) -> Option<OpOutcome> {
    match (name, args) {
        ("json_number_end", [OpValue::Bytes(data), start]) => {
            let start = bits(start)? as u64;
            let Ok(mut pos) = usize::try_from(start) else {
                return Some(words([start, 0]));
            };
            let mut state = 0;
            let valid = loop {
                let Some(&c) = data.get(pos) else {
                    break matches!(state, 2 | 3 | 5 | 8);
                };
                state = match (state, c) {
                    (0, b'-') => 1,
                    (0 | 1, b'0') => 2,
                    (0 | 1, b'1'..=b'9') => 3,
                    (2, b'0'..=b'9') => break false,
                    (2 | 3, b'.') => 4,
                    (2 | 3 | 5, b'e' | b'E') => 6,
                    (3, b'0'..=b'9') => 3,
                    (4 | 5, b'0'..=b'9') => 5,
                    (6, b'+' | b'-') => 7,
                    (6..=8, b'0'..=b'9') => 8,
                    (2 | 3 | 5 | 8, _) => break true,
                    _ => break false,
                };
                pos += 1;
            };
            Some(words([pos as u64, u64::from(valid)]))
        }
        ("integer", [OpValue::Bytes(data), radix, signed, ph, pl, nh, nl]) => {
            let radix = bits(radix)? as u32;
            let negative = data.first() == Some(&b'-');
            let text = if negative { &data[1..] } else { &data[..] };
            if !(2..=36).contains(&radix)
                || text.is_empty()
                || negative && bits(signed)? == 0
                || !text
                    .iter()
                    .all(|&b| b.is_ascii_alphanumeric() && char::from(b).is_digit(radix))
            {
                return Some(words([0, 0, 2]));
            }
            let text = std::str::from_utf8(text).ok()?;
            let limit = if negative {
                bits(nh)? << 64 | bits(nl)?
            } else {
                bits(ph)? << 64 | bits(pl)?
            };
            let result = match u128::from_str_radix(text, radix) {
                Ok(n) if n <= limit => [(n >> 64) as u64, n as u64, u64::from(negative)],
                _ => [0, 0, 3],
            };
            Some(words(result))
        }
        ("decimal_word", [value, OpValue::Sequence(buffer)]) => {
            let digits = (bits(value)? as u64).to_string();
            let mut buffer = buffer.clone();
            for (slot, byte) in buffer.iter_mut().zip(digits.bytes()) {
                *slot = OpValue::Bits {
                    scalar: ScalarKind::U8,
                    bits: u128::from(byte),
                };
            }
            Some(OpOutcome::Updated {
                value: word(digits.len() as u64),
                arguments: vec![(
                    1,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits: buffer.iter().map(bits).collect::<Option<Vec<_>>>()?,
                    },
                )],
            })
        }
        _ => None,
    }
}
