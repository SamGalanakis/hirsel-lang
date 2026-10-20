//! The pool differential inside every unit test: main's equality and
//! compatibility entry points, answered by the interner's normal forms and
//! `ContextMemo`, each verdict asserted equal to the reference walk
//! (`reference`), main's algorithm. Test-only.
use crate::*;

/// Rooted semantic equality: `semantic_eq`, checked against main's pairwise walk.
pub fn semantic_equal(t: &TypeInterner, a: TypeId, b: TypeId) -> Result<bool, GraphError> {
    let new = t
        .validate(a)
        .and_then(|()| t.validate(b))
        .map(|()| t.semantic_eq(a, b));
    let old = crate::reference::semantic_equal(t, a, t, b);
    assert_eq!(
        new.as_ref().ok(),
        old.as_ref().ok(),
        "semantic_eq disagrees with main's walk for {a:?}, {b:?}: {new:?} vs {old:?}"
    );
    new
}

fn differential(
    what: &str,
    expected: TypeId,
    actual: TypeId,
    new: Result<bool, QueryError>,
    old: Result<bool, QueryError>,
) -> Result<bool, QueryError> {
    assert_eq!(
        new, old,
        "ContextMemo::{what} disagrees with main's relation for {expected:?} <- {actual:?}"
    );
    new
}

pub fn compatible_type(
    t: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let new = {
        let mut f = |d| lookup(d);
        let mut memo = ContextMemo::new(t, generics, &mut f);
        memo.compatible_type(expected, actual, relation)
    };
    let old = crate::reference::compatible_type(t, expected, actual, relation, generics, lookup);
    differential("compatible_type", expected, actual, new, old)
}

pub fn compatible_handback_type(
    t: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let new = {
        let mut f = |d| lookup(d);
        ContextMemo::new(t, generics, &mut f).handback(expected, actual)
    };
    let old = crate::reference::compatible_handback_type(t, expected, actual, generics, lookup);
    differential("handback", expected, actual, new, old)
}

pub fn compatible_view_type_in(
    t: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let new = {
        let mut f = |d| lookup(d);
        ContextMemo::new(t, generics, &mut f).view(expected, actual, relation)
    };
    let old =
        crate::reference::compatible_view_type_in(t, expected, actual, relation, generics, lookup);
    differential("view", expected, actual, new, old)
}

pub fn compatible_storage_type(
    t: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let new = {
        let mut f = |d| lookup(d);
        ContextMemo::new(t, generics, &mut f).storage(expected, actual)
    };
    let old = crate::reference::compatible_storage_type(t, expected, actual, generics, lookup);
    differential("storage", expected, actual, new, old)
}

/// Main's `substitute_type_in_place` and `import_type`: transport in one interner.
pub fn substitute_type_in_place(
    t: &TypeInterner,
    root: TypeId,
    substitution: Substitution<'_>,
) -> Result<TypeId, GraphError> {
    substitute(t, &TypeMemo::new(), root, &substitution)
}
