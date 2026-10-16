//! Declared-type rules shared by written headers and body annotations.
use hirsel_span::Span;
use hirsel_syntax as ast;
use hirsel_types::{Bound, BuiltinType};

/// LANG-7 message for a declared part (field, payload, element or type
/// argument) that carries a relation; shared with body annotations.
pub const PART_LOCALITY: &str =
    "a field, element or type argument cannot declare a relation: it is related as its owner is";

/// Whether a named function's written result relation reaches `env`
/// (`Some(true)`), names only places (`Some(false)`) or is not written
/// (`None`). `env` is MODE-1 there even on a plain value, which
/// canonicalizes the relation away.
pub fn written_local(tree: &ast::SyntaxTree, mut ty: ast::TypeExprId) -> Option<bool> {
    while let ast::TypeExprKind::Qualified { base, relation, .. } = &tree.types[ty.0 as usize].kind
    {
        if let Some(relation) = relation {
            if relation.places.contains(&ast::RelationPlace::Environment) {
                return Some(true);
            }
            if relation
                .places
                .iter()
                .any(|p| matches!(p, ast::RelationPlace::Path(_)))
            {
                return Some(false);
            }
        }
        ty = *base;
    }
    None
}

/// The span of a function type's result relation when it is `in env` alone
/// (#72), which then may name no non-Plain parameter.
pub fn environment_alone(tree: &ast::SyntaxTree, ty: ast::TypeExprId) -> Option<Span> {
    match &tree.types[ty.0 as usize].kind {
        ast::TypeExprKind::Qualified {
            relation: Some(relation),
            ..
        } if relation.places == [ast::RelationPlace::Environment] => Some(relation.origin.span),
        _ => None,
    }
}
/// MODE-1 (#72): `in env` alone would drop the result's dependence on the
/// non-Plain parameters, so the diagnostic offers them as candidate sources.
pub fn environment_candidates(candidates: &[String]) -> String {
    let named: Vec<&str> = candidates
        .iter()
        .map(String::as_str)
        .filter(|c| !c.starts_with("parameter "))
        .collect();
    let repair = match named.as_slice() {
        [] => "name the parameters and relate the result to them".to_owned(),
        [one] => format!("write `in {{env, {one}}}` or `in {one}`"),
        many => format!(
            "write `in {{env, {}}}` or name the ones it reads",
            many.join(", ")
        ),
    };
    let noun = if candidates.len() == 1 {
        "parameter"
    } else {
        "parameters"
    };
    format!(
        "`in env` alone cannot cover the non-Plain {noun} {}: {repair}",
        candidates.join(", ")
    )
}

/// The bound a builtin's first type argument must satisfy wherever the type is
/// written, in a signature or a body annotation (GEN-1).
pub fn builtin_bound(b: BuiltinType) -> Option<Bound> {
    match b {
        BuiltinType::Map | BuiltinType::Set => Some(Bound::Key),
        BuiltinType::Packed => Some(Bound::Flat),
        BuiltinType::Simd => Some(Bound::Plain),
        _ => None,
    }
}
