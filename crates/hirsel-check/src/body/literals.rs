use super::*;
/// Splits a numeric spelling into its core and suffix, as the lexer reads it:
/// the suffix starts at the first `u` or `i` (never a digit), or at an `f`
/// outside a hex literal, where `f` is a digit (`0x1f32` is unsuffixed).
fn split(text: &str) -> (&str, &str) {
    let hex = text.trim_start_matches('-').starts_with("0x");
    let end = text
        .find(|c| c == 'u' || c == 'i' || (c == 'f' && !hex))
        .unwrap_or(text.len());
    text.split_at(end)
}
pub(super) fn suffix(text: &str) -> Option<Primitive> {
    use Primitive::*;
    Some(match split(text).1 {
        "u8" => U8,
        "u16" => U16,
        "u32" => U32,
        "u64" => U64,
        "u128" => U128,
        "i8" => I8,
        "i16" => I16,
        "i32" => I32,
        "i64" => I64,
        "i128" => I128,
        "f32" => F32,
        "f64" => F64,
        _ => return None,
    })
}
pub(super) fn fixed_primitive(l: &ast::Literal) -> Option<Primitive> {
    match l {
        ast::Literal::Integer(s) | ast::Literal::Float(s) => suffix(s),
        ast::Literal::String(_) => Some(Primitive::String),
        ast::Literal::Bytes(_) => Some(Primitive::Bytes),
        ast::Literal::Bool(_) => Some(Primitive::Bool),
    }
}
fn width(p: Primitive) -> Option<(u32, bool)> {
    use Primitive::*;
    Some(match p {
        U8 => (8, false),
        U16 => (16, false),
        U32 => (32, false),
        U64 => (64, false),
        U128 => (128, false),
        I8 => (8, true),
        I16 => (16, true),
        I32 => (32, true),
        I64 => (64, true),
        I128 => (128, true),
        _ => return None,
    })
}
fn numeric_core(s: &str) -> String {
    split(s).0.replace('_', "")
}
impl Checker<'_, '_> {
    pub(super) fn literal(
        &mut self,
        l: &ast::Literal,
        expected: Option<TypeId>,
        span: Span,
        negative: bool,
    ) -> TypeId {
        let fixed = fixed_primitive(l);
        let expected = if expected.is_some_and(|t| self.invalid(t)) {
            // An explicit suffix supplies its own type, so its range/format
            // remains an independent obligation during parent recovery.
            if fixed.is_none() {
                return self.error();
            }
            None
        } else {
            expected
        };
        let primitive = fixed.or_else(|| {
            expected.and_then(|t| match self.kind(t) {
                TypeKind::Primitive(p) => Some(p),
                _ => None,
            })
        });
        let Some(p) = primitive else {
            self.report(
                const { RuleId::of("LANG-2") },
                span,
                "literal needs an expected type or numeric suffix",
            );
            return self.error();
        };
        match l {
            ast::Literal::Integer(s) => {
                let Some((bits, signed)) = width(p) else {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        span,
                        "integer literal requires an integer type",
                    );
                    return self.error();
                };
                let mut core = numeric_core(s);
                let sign = negative || core.starts_with('-');
                if core.starts_with('-') {
                    core.remove(0);
                }
                let (digits, radix) = if let Some(s) = core.strip_prefix("0x") {
                    (s, 16)
                } else if let Some(s) = core.strip_prefix("0o") {
                    (s, 8)
                } else if let Some(s) = core.strip_prefix("0b") {
                    (s, 2)
                } else {
                    (core.as_str(), 10)
                };
                let magnitude = u128::from_str_radix(digits, radix).ok();
                let limit = if signed {
                    let high = 1u128 << (bits - 1);
                    if sign { high } else { high - 1 }
                } else if bits == 128 {
                    u128::MAX
                } else {
                    (1u128 << bits) - 1
                };
                if magnitude.is_none_or(|n| n > limit) || (sign && !signed && magnitude != Some(0))
                {
                    self.report(
                        const { RuleId::of("SEM-1") },
                        span,
                        "integer literal is outside its type's range",
                    );
                    return self.error();
                }
            }
            ast::Literal::Float(s) => {
                let core = numeric_core(s);
                let valid = match p {
                    Primitive::F32 => core.parse::<f32>().is_ok(),
                    Primitive::F64 => core.parse::<f64>().is_ok(),
                    _ => false,
                };
                if !valid {
                    self.report(
                        const { RuleId::of("SEM-2") },
                        span,
                        "float literal requires f32 or f64",
                    );
                    return self.error();
                }
            }
            ast::Literal::String(_) => {
                if p != Primitive::String {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        span,
                        "string literal requires String",
                    );
                    return self.error();
                }
            }
            ast::Literal::Bytes(_) => {
                if p != Primitive::Bytes {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        span,
                        "bytes literal requires Bytes",
                    );
                    return self.error();
                }
            }
            ast::Literal::Bool(_) => {
                if p != Primitive::Bool {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        span,
                        "boolean literal requires bool",
                    );
                    return self.error();
                }
            }
        }
        self.primitive(p)
    }
}
