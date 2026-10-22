//! Explicit named integer Order values, constructed from their declared graphs.
use super::*;

pub(crate) const INTEGERS: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
];
pub(super) fn signatures(types: &Arc<TypeInterner>) -> Vec<Signature> {
    INTEGERS
        .iter()
        .map(|name| {
            let mut builder = build::Builder::new(types, "");
            let function = builder.function(&format!("a:{name},b:{name}"), "i32 uses {}", true);
            let mut signature = builder.signature(
                &format!("$intrinsic.OrderCompare_{name}"),
                SignatureKind::Function(function),
                false,
            );
            signature.metadata.visibility = hirsel_syntax::Visibility::Private;
            signature
        })
        .collect()
}
