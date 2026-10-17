use super::*;
use hirsel_syntax::Literal;
use hirsel_types::{Primitive, TypeKind};
fn bad(span: Span, message: &str) -> LowerError {
    LowerError {
        site: span,
        message: message.into(),
    }
}
fn magnitude(s: &str) -> Option<u128> {
    let s = s.replace('_', "");
    let s = [
        "u128", "i128", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8",
    ]
    .iter()
    .find_map(|suffix| s.strip_suffix(suffix))
    .unwrap_or(&s);
    let (radix, digits) = if let Some(s) = s.strip_prefix("0x") {
        (16, s)
    } else if let Some(s) = s.strip_prefix("0o") {
        (8, s)
    } else if let Some(s) = s.strip_prefix("0b") {
        (2, s)
    } else {
        (10, s)
    };
    u128::from_str_radix(digits, radix).ok()
}
fn signed_width(ty: &TypeKind) -> Option<u32> {
    match ty {
        TypeKind::Primitive(Primitive::I8) => Some(8),
        TypeKind::Primitive(Primitive::I16) => Some(16),
        TypeKind::Primitive(Primitive::I32) => Some(32),
        TypeKind::Primitive(Primitive::I64) => Some(64),
        TypeKind::Primitive(Primitive::I128) => Some(128),
        _ => None,
    }
}
pub(super) fn signed_min(s: &str, ty: &TypeKind) -> Option<u128> {
    let width = signed_width(ty)?;
    let n = magnitude(s)?;
    (n == 1u128 << (width - 1)).then_some(n)
}
pub(super) fn constant(literal: &Literal, ty: &TypeKind, span: Span) -> Result<Constant> {
    Ok(match literal {
        Literal::Bool(value) => Constant::Bool(*value),
        Literal::Character(spelling) => Constant::Integer(
            hirsel_syntax::character_value(spelling)
                .ok_or_else(|| bad(span, "invalid checked character"))? as u128,
        ),
        Literal::Integer(spelling) => {
            // Negative literals occur in patterns; expression negatives are Unary.
            let negative = spelling.starts_with('-');
            let n = magnitude(spelling.trim_start_matches('-'))
                .ok_or_else(|| bad(span, "invalid checked integer"))?;
            let n = if negative {
                let width =
                    signed_width(ty).ok_or_else(|| bad(span, "negative non-signed literal"))?;
                0u128.wrapping_sub(n) & (u128::MAX >> (128 - width))
            } else {
                n
            };
            Constant::Integer(n)
        }
        Literal::Float(spelling) => {
            let s = spelling.replace('_', "");
            let s = s.trim_end_matches("f32").trim_end_matches("f64");
            match ty {
                TypeKind::Primitive(Primitive::F32) => Constant::F32(
                    s.parse::<f32>()
                        .map_err(|_| bad(span, "invalid checked f32"))?
                        .to_bits(),
                ),
                TypeKind::Primitive(Primitive::F64) => Constant::F64(
                    s.parse::<f64>()
                        .map_err(|_| bad(span, "invalid checked f64"))?
                        .to_bits(),
                ),
                _ => return Err(bad(span, "float has non-float checked type")),
            }
        }
        Literal::String(s) => Constant::String(
            String::from_utf8(decode(s, false, span)?)
                .map_err(|_| bad(span, "invalid string UTF-8"))?,
        ),
        Literal::Bytes(s) => Constant::Bytes(decode(s, true, span)?),
    })
}
fn decode(s: &str, bytes: bool, span: Span) -> Result<Vec<u8>> {
    let s = if bytes {
        s.strip_prefix('b').unwrap_or(s)
    } else {
        s
    };
    let s = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| bad(span, "literal spelling lacks quotes"))?;
    let mut chars = s.chars();
    let mut out = vec![];
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next().ok_or_else(|| bad(span, "unfinished escape"))? {
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            '0' => out.push(0),
            '"' => out.push(b'"'),
            '\\' => out.push(b'\\'),
            'x' => {
                let a = chars
                    .next()
                    .and_then(|c| c.to_digit(16))
                    .ok_or_else(|| bad(span, "invalid hex escape"))?;
                let b = chars
                    .next()
                    .and_then(|c| c.to_digit(16))
                    .ok_or_else(|| bad(span, "invalid hex escape"))?;
                out.push((a * 16 + b) as u8);
            }
            'u' if !bytes => {
                if chars.next() != Some('{') {
                    return Err(bad(span, "invalid Unicode escape"));
                }
                let mut digits = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    digits.push(c);
                }
                if !closed {
                    return Err(bad(span, "unfinished Unicode escape"));
                }
                let c = u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| bad(span, "invalid Unicode scalar"))?;
                let mut buffer = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            }
            _ => return Err(bad(span, "unknown checked escape")),
        }
    }
    Ok(out)
}
