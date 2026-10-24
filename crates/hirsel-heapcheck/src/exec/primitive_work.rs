//! Primitive work policy transcribed independently for post-RC H. No kernel,
//! runtime or source-interpreter implementation participates in this checker.
use crate::{
    OpValue,
    violation::{Checked, ViolationKind as V, err},
};
use hirsel_core_contract::OperationId;
use hirsel_lir::layout_types::ScalarKind;

pub(super) fn cost(id: &OperationId, arguments: &[OpValue]) -> Checked<u64> {
    let path = &id.definition.0;
    if !path.package.package_is("core", "1") || path.modules != ["bytes"] {
        return Ok(0);
    }
    match path.name.as_str() {
        "json_number_end" => {
            let [
                OpValue::Bytes(data),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                },
            ] = arguments
            else {
                return Err(err(
                    V::TypeConfusion,
                    "JSON scan requires checked bytes and a u64 cursor",
                ));
            };
            Ok(json_work(data, *bits as u64))
        }
        "integer" => {
            let [OpValue::Bytes(data), ..] = arguments else {
                return Err(err(V::TypeConfusion, "integer scan requires checked bytes"));
            };
            Ok((data.len() as u64).saturating_add(1))
        }
        "decimal_word" => Ok(9),
        "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "sha512_256" => {
            sha_work(path.name.as_str(), arguments)
        }
        _ => Ok(0),
    }
}

fn sha_work(name: &str, arguments: &[OpValue]) -> Checked<u64> {
    let (block, length_field) = match name {
        "sha1" | "sha224" | "sha256" => (64, 8),
        "sha384" | "sha512" | "sha512_256" => (128, 16),
        _ => return Ok(0),
    };
    let [OpValue::Bytes(bytes)] = arguments else {
        return Err(err(
            V::TypeConfusion,
            "SHA requires one checked byte extent",
        ));
    };
    let length = u64::try_from(bytes.len())
        .map_err(|_| err(V::Unsupported, "SHA extent length overflow"))?;
    (length / block)
        .checked_add(2 + u64::from(length % block >= block - length_field))
        .ok_or_else(|| err(V::Unsupported, "SHA primitive work overflow"))
}

// A lexical DFA independent of the production parser's grammar-phase loops.
fn json_work(data: &[u8], start: u64) -> u64 {
    let Ok(mut pos) = usize::try_from(start) else {
        return 1;
    };
    let mut state = 0;
    while let Some(&c) = data.get(pos) {
        state = match (state, c) {
            (0, b'-') => 1,
            (0 | 1, b'0') => 2,
            (0 | 1, b'1'..=b'9') => 3,
            (2, b'0'..=b'9') => break,
            (2 | 3, b'.') => 4,
            (2 | 3 | 5, b'e' | b'E') => 6,
            (3, b'0'..=b'9') => 3,
            (4 | 5, b'0'..=b'9') => 5,
            (6, b'+' | b'-') => 7,
            (6..=8, b'0'..=b'9') => 8,
            _ => break,
        };
        pos += 1;
    }
    1 + (pos as u64)
        .saturating_add(1)
        .min(data.len() as u64)
        .saturating_sub(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::DefPath;
    #[test]
    fn byte_work_counts_exact_prefix_lookahead_and_foreign_names() {
        let id = |name: &str| OperationId {
            definition: DefPath::core(name),
            declaration_digest: Default::default(),
        };
        let cursor = |n| OpValue::Bits {
            scalar: ScalarKind::U64,
            bits: n,
        };
        for (data, expected) in [
            ("", 1),
            ("0", 2),
            ("01", 3),
            ("1x", 3),
            ("-1.2E+3]", 9),
            ("1e+", 4),
            ("-", 2),
            ("1\0", 3),
        ] {
            assert_eq!(
                cost(
                    &id("bytes.json_number_end"),
                    &[OpValue::Bytes(data.as_bytes().to_vec()), cursor(0)]
                )
                .unwrap(),
                expected,
                "{data}"
            );
        }
        assert_eq!(
            cost(
                &id("bytes.json_number_end"),
                &[OpValue::Bytes(b"xx12]".to_vec()), cursor(2)]
            )
            .unwrap(),
            4
        );
        assert_eq!(
            cost(
                &id("bytes.json_number_end"),
                &[OpValue::Bytes(b"1".to_vec()), cursor(u128::from(u64::MAX))]
            )
            .unwrap(),
            1
        );
        assert_eq!(
            cost(&id("bytes.integer"), &[OpValue::Bytes(vec![0; 8192])]).unwrap(),
            8193
        );
        assert_eq!(cost(&id("bytes.decimal_word"), &[]).unwrap(), 9);
        let mut foreign = id("bytes.integer");
        foreign.definition.0.package.name = "foreign".into();
        assert_eq!(cost(&foreign, &[OpValue::Bytes(vec![0; 8192])]).unwrap(), 0);
        assert!(cost(&id("bytes.json_number_end"), &[]).is_err());
    }
}
