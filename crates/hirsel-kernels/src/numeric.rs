//! Strict numeric leaves. JSON grammar follows serde_json src/de.rs:462-623
//! (8c535a2); integer grammar/overflow follows Go strconv/atoi.go:70-162,215-247
//! (ebe2feef), with Hirsel's no-plus/no-underscore grammar and Invalid priority.

/// Cursor after a JSON number, or at its exact syntax error; second word is 1
/// on success. The charge includes one point per distinct input byte inspected.
pub fn json_number(data: &[u8], start: u64) -> ([u64; 2], u64) {
    let mut pos = usize::try_from(start).unwrap_or(usize::MAX);
    let peek = |p: usize| data.get(p).copied().unwrap_or(0);
    let mut c = peek(pos);
    let result = (|| {
        if c == b'-' {
            pos += 1;
            c = peek(pos);
        }
        match c {
            b'0' => {
                pos += 1;
                c = peek(pos);
                if c.is_ascii_digit() {
                    return false;
                }
            }
            b'1'..=b'9' => {
                while c.is_ascii_digit() {
                    pos += 1;
                    c = peek(pos);
                }
            }
            _ => return false,
        }
        if c == b'.' {
            pos += 1;
            c = peek(pos);
            if !c.is_ascii_digit() {
                return false;
            }
            while c.is_ascii_digit() {
                pos += 1;
                c = peek(pos);
            }
        }
        if c == b'e' || c == b'E' {
            pos += 1;
            c = peek(pos);
            if c == b'+' || c == b'-' {
                pos += 1;
                c = peek(pos);
            }
            if !c.is_ascii_digit() {
                return false;
            }
            while c.is_ascii_digit() {
                pos += 1;
                c = peek(pos);
            }
        }
        true
    })();
    let end = pos as u64;
    let examined = end
        .saturating_add(1)
        .min(data.len() as u64)
        .saturating_sub(start);
    ([end, u64::from(result)], 1 + examined)
}

/// Magnitude high/low words and status: 0 positive, 1 negative, 2 Invalid,
/// 3 OutOfRange. Validate through overflow so syntax errors always win.
pub fn integer(data: &[u8], radix: u8, signed: bool, positive: u128, negative: u128) -> [u64; 3] {
    let invalid = [0, 0, 2];
    if !(2..=36).contains(&radix) || data.is_empty() {
        return invalid;
    }
    let minus = data[0] == b'-';
    if minus && (!signed || data.len() == 1) {
        return invalid;
    }
    let limit = if minus { negative } else { positive };
    let base = u128::from(radix);
    let (cutoff, last) = (limit / base, limit % base);
    let mut magnitude = 0;
    let mut overflow = false;
    for &byte in &data[usize::from(minus)..] {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'z' => byte - b'a' + 10,
            b'A'..=b'Z' => byte - b'A' + 10,
            _ => return invalid,
        };
        if digit >= radix {
            return invalid;
        }
        let digit = u128::from(digit);
        if !overflow {
            if magnitude > cutoff || magnitude == cutoff && digit > last {
                overflow = true;
            } else {
                magnitude = magnitude * base + digit;
            }
        }
    }
    if overflow {
        [0, 0, 3]
    } else {
        [(magnitude >> 64) as u64, magnitude as u64, u64::from(minus)]
    }
}

/// itoa's allocation-free pair-table formatter (src/lib.rs:264-320,1577ed9).
/// Only the returned prefix is written; the rest of the caller's array is intact.
pub fn decimal_word(value: u64, out: &mut [u8; 20]) -> u64 {
    let mut buffer = itoa::Buffer::new();
    let digits = buffer.format(value).as_bytes();
    out[..digits.len()].copy_from_slice(digits);
    digits.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_cursor_grammar_offsets_and_work() {
        for (text, end, valid) in [
            ("0", 1, 1),
            ("-0", 2, 1),
            ("1.2300E+004]", 11, 1),
            ("1e999999999999999999999", 23, 1),
            ("01", 1, 0),
            ("-", 1, 0),
            ("1.", 2, 0),
            ("1e+", 3, 0),
            ("1x", 1, 1),
            (".1", 0, 0),
            ("+1", 0, 0),
            ("", 0, 0),
        ] {
            let (result, charge) = json_number(text.as_bytes(), 0);
            assert_eq!(result, [end, valid], "{text}");
            assert_eq!(charge, 1 + (end + 1).min(text.len() as u64));
            let prefixed = format!("xx{text}");
            let (result, shifted_charge) = json_number(prefixed.as_bytes(), 2);
            assert_eq!(result, [end + 2, valid]);
            assert_eq!(shifted_charge, charge);
        }
        assert_eq!(json_number(b"1", u64::MAX), ([u64::MAX, 0], 1));
        assert_eq!(json_number(b"1\0", 0), ([1, 1], 3));
    }

    #[test]
    fn integer_limits_and_invalid_overflow_precedence() {
        for bits in [8, 16, 32, 64, 128] {
            let max = u128::MAX >> (128 - bits);
            for n in [0, 1, max / 2, max - 1, max] {
                let s = n.to_string();
                assert_eq!(
                    integer(s.as_bytes(), 10, false, max, 0),
                    [(n >> 64) as u64, n as u64, 0]
                );
            }
            let negative = 1u128 << (bits - 1);
            let positive = negative - 1;
            assert_eq!(
                integer(
                    format!("-{negative}").as_bytes(),
                    10,
                    true,
                    positive,
                    negative
                ),
                [(negative >> 64) as u64, negative as u64, 1]
            );
            assert_eq!(
                integer(
                    negative.to_string().as_bytes(),
                    10,
                    true,
                    positive,
                    negative
                )[2],
                3
            );
        }
        let huge = "340282366920938463463374607431768211456";
        assert_eq!(integer(huge.as_bytes(), 10, false, u128::MAX, 0)[2], 3);
        assert_eq!(
            integer(format!("{huge}x").as_bytes(), 10, false, u128::MAX, 0)[2],
            2
        );
        for s in ["", "-", "+1", "1_0", "é", "9x"] {
            assert_eq!(
                integer(s.as_bytes(), 10, true, u128::MAX, u128::MAX)[2],
                2,
                "{s}"
            );
        }
        for radix in 2..=36 {
            for n in 0..radix {
                let digit = char::from_digit(u32::from(n), u32::from(radix)).unwrap();
                for s in [digit.to_string(), digit.to_ascii_uppercase().to_string()] {
                    assert_eq!(
                        integer(s.as_bytes(), radix, false, u128::MAX, 0),
                        [0, u64::from(n), 0]
                    );
                }
            }
        }
        assert_eq!(integer(b"-0", 10, true, 0, 0), [0, 0, 1]);
        assert_eq!(integer(b"1", 1, false, u128::MAX, 0)[2], 2);
        assert_eq!(integer(b"1", 37, false, u128::MAX, 0)[2], 2);
    }

    #[test]
    fn decimal_prefix_and_untouched_tail_match_rust_formatter() {
        let mut values: Vec<_> = (0..10000).collect();
        let mut power = 10u64;
        while let Some(next) = power.checked_mul(10) {
            values.extend([power - 1, power, power + 1]);
            power = next;
        }
        values.extend([power - 1, power, power + 1, u64::MAX]);
        let mut seed = 38u64;
        for _ in 0..1000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            values.push(seed);
        }
        for value in values {
            let mut out = [0xa5; 20];
            let count = decimal_word(value, &mut out) as usize;
            assert_eq!(&out[..count], value.to_string().as_bytes());
            assert!(out[count..].iter().all(|&byte| byte == 0xa5));
        }
    }
}
