use super::*;
use std::collections::BTreeMap;

pub fn brand_join(a: &Brand, b: &Brand) -> Brand {
    let mut alternatives = a.alternatives.clone();
    alternatives.extend(b.alternatives.iter().cloned());
    alternatives.sort();
    alternatives.dedup();
    Brand { alternatives }
}
pub(super) fn subset(actual: &AuthoritySummary, allowed: &AuthoritySummary) -> bool {
    match (actual, allowed) {
        (AuthoritySummary::Known(a), _) if a.is_empty() => true,
        (_, AuthoritySummary::Unknown) => false,
        (AuthoritySummary::Known(a), AuthoritySummary::Known(b)) => a.iter().all(|x| b.contains(x)),
        _ => false,
    }
}
pub(super) fn substitute_authority(
    summary: &AuthoritySummary,
    parameters: &[AuthoritySummary],
    generics: &[AuthoritySummary],
) -> AuthoritySummary {
    let AuthoritySummary::Known(terms) = summary else {
        return AuthoritySummary::Unknown;
    };
    terms.iter().fold(AuthoritySummary::inert(), |a, term| {
        let next = match term {
            AuthorityTerm::Parameter(ParameterRef::Bound {
                depth: 0,
                parameter,
            }) => parameters
                .get(parameter.0 as usize)
                .cloned()
                .unwrap_or(AuthoritySummary::Unknown),
            AuthorityTerm::TypeParameter(p) if p.binder() == GenericBinder::Bound(0) => generics
                .get(p.parameter().0 as usize)
                .cloned()
                .unwrap_or(AuthoritySummary::Unknown),
            _ => AuthoritySummary::Known(vec![term.clone()].into()),
        };
        a.join(next)
    })
}
pub(super) fn origins(
    store: &mut TypeStore,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
    actuals: &BTreeMap<ParameterRef, TypeId>,
) -> TypeId {
    instantiate_parameter_sources(store, root, arguments, actuals).expect("checked parameter graph")
}
pub(super) fn siblings(
    store: &mut TypeStore,
    root: TypeId,
    fields: &BTreeMap<FieldId, Origin>,
) -> TypeId {
    instantiate_sibling_origins(store, root, fields).expect("checked sibling graph")
}
