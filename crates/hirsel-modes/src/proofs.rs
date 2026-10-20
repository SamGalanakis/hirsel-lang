//! The structural property proofs of the mode passes, through the query's
//! `ContextMemo`: each is one `prove` mode, as the free proof functions of
//! hirsel-types define them.
use hirsel_syntax::Bound;
use hirsel_types::{
    BuiltinType, ContextMemo, ProofFlags, QueryError, Sendability, StructuralProperty, TypeId,
    TypeKind,
};

const STRUCTURAL: ProofFlags = ProofFlags {
    check_root_locality: true,
    use_evidence: true,
    shared_storage: false,
    handles: false,
};

/// `structural_property`.
pub fn property(
    memo: &mut ContextMemo<'_>,
    root: TypeId,
    property: StructuralProperty,
) -> Result<bool, QueryError> {
    let proven = memo.prove(root, property.bound(), STRUCTURAL)? == Sendability::Sendable;
    Ok(if property == StructuralProperty::NeedsRelease {
        !proven
    } else {
        proven
    })
}
/// `structural_flat`.
pub fn flat(memo: &mut ContextMemo<'_>, root: TypeId) -> Result<bool, QueryError> {
    Ok(memo.prove(root, Bound::Flat, STRUCTURAL)? == Sendability::Sendable)
}
/// `copy_detaches` (MEM-11, CONC-2).
pub fn copy_detaches(memo: &mut ContextMemo<'_>, root: TypeId) -> Result<bool, QueryError> {
    let flags = ProofFlags {
        handles: true,
        ..STRUCTURAL
    };
    Ok(memo.prove(root, Bound::Plain, flags)? == Sendability::Sendable)
}
/// `structural_sendability`: a callable's or iterator's crossing mode is
/// evidence about its captures, apart from its own locality (MODE-4).
pub fn sendability(memo: &mut ContextMemo<'_>, root: TypeId) -> Result<Sendability, QueryError> {
    let check_root_locality = !memo.types().get(root).is_some_and(|t| {
        matches!(
            t.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        )
    });
    let flags = ProofFlags {
        check_root_locality,
        use_evidence: false,
        shared_storage: false,
        handles: false,
    };
    memo.prove(root, Bound::Sendable, flags)
}
/// `content_sendability`.
pub fn content_sendability(
    memo: &mut ContextMemo<'_>,
    root: TypeId,
) -> Result<Sendability, QueryError> {
    let flags = ProofFlags {
        check_root_locality: false,
        ..STRUCTURAL
    };
    memo.prove(root, Bound::Sendable, flags)
}
/// `sendability_after_sharing`.
pub fn sendability_after_sharing(
    memo: &mut ContextMemo<'_>,
    root: TypeId,
) -> Result<Sendability, QueryError> {
    let flags = ProofFlags {
        check_root_locality: false,
        use_evidence: false,
        shared_storage: true,
        handles: false,
    };
    memo.prove(root, Bound::Sendable, flags)
}

#[cfg(test)]
mod tests;
