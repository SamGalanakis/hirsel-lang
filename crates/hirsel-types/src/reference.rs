//! Test-only copies of main's store-walk algorithms, run over interner rows:
//! the pool differential's reference. `semantic_equal` walks both graphs
//! pairwise as main did; the `compatible*` family is main's directional
//! relation without memos or normal-form short cuts. Production code never
//! calls these; `test_api` checks every test's verdict against them.
#![allow(clippy::too_many_arguments)]
use crate::graph::{Variance, header_weakens_uniqueness};
use crate::properties::{
    crossing_requirement, receives_crossing, receives_relation, value_promises,
};
use crate::*;
use std::collections::HashSet;

/// Rooted structural equality of normalized types. Nominal identity is DefPath;
/// unreachable rows, interning order, and provenance do not participate.
/// Generic ordinals must be in corresponding binder scopes; this is not alias
/// expansion, subtyping, or proof search.
pub fn semantic_equal(
    a: &TypeInterner,
    a_root: TypeId,
    b: &TypeInterner,
    b_root: TypeId,
) -> Result<bool, GraphError> {
    main_directional_walk(a, a_root, None, b, b_root, None, false, None, false)
}

// Compare row data independently of arena identity; edge correspondence is
// checked by the worklist. This also retains canonical erased-capture policy.
fn main_row_data(row: &Type, placeholder: TypeId) -> Type {
    let mut row = row.clone();
    if let Some(ErasedCaptures::FromArguments(ps)) = &mut row.erased_captures {
        ps.sort();
        ps.dedup();
    }
    if row.erased_captures == Some(ErasedCaptures::Known(AuthoritySummary::Unknown)) {
        row.erased_captures = None;
    }
    match &mut row.kind {
        TypeKind::Tuple(ts) => ts.fill(placeholder),
        TypeKind::Box(t) => *t = placeholder,
        TypeKind::Record(fs) => fs.iter_mut().for_each(|f| f.ty = placeholder),
        TypeKind::Variant(vs) => vs
            .iter_mut()
            .flat_map(|v| &mut v.fields)
            .for_each(|f| f.1 = placeholder),
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            for argument in arguments {
                if let GenericArgument::Type(t) = argument {
                    *t = placeholder;
                }
            }
        }
        TypeKind::Function(f) => {
            for parameter in &mut f.parameters {
                parameter.name = None;
                parameter.ty = placeholder;
            }
            f.result = placeholder;
            if let Some(error) = &mut f.error {
                *error = placeholder;
            }
        }
        _ => (),
    }
    row
}

pub fn main_directional_walk(
    a: &TypeInterner,
    a_root: TypeId,
    a_row: Option<&Type>,
    b: &TypeInterner,
    b_root: TypeId,
    b_row: Option<&Type>,
    // A nested row of `b` may carry a brand where the row of `a` names none.
    forget_brands: bool,
    payloads: Option<&HashSet<TypeId>>,
    // `b` is a value received where `a` is expected: its nested rows may make
    // the stronger promise along `Variance` edges.
    received: bool,
) -> Result<bool, GraphError> {
    fn row<'a>(
        store: &'a TypeInterner,
        id: TypeId,
        root: TypeId,
        adjusted: Option<&'a Type>,
    ) -> Result<&'a Type, GraphError> {
        if id == root
            && let Some(row) = adjusted
        {
            return Ok(row);
        }
        store.get(id).ok_or(GraphError::MissingType(id))
    }
    fn valid(
        store: &TypeInterner,
        root: TypeId,
        adjusted: Option<&Type>,
    ) -> Result<(), GraphError> {
        let mut pending = vec![(root, false)];
        let mut active = HashSet::new();
        let mut done = HashSet::new();
        while let Some((id, exit)) = pending.pop() {
            if exit {
                active.remove(&id);
                done.insert(id);
                continue;
            }
            if done.contains(&id) {
                continue;
            }
            if !active.insert(id) {
                return Err(GraphError::StructuralCycle(id));
            }
            let row = row(store, id, root, adjusted)?;
            if matches!(row.kind, TypeKind::Error) {
                return Err(GraphError::RecoveryType(id));
            }
            pending.push((id, true));
            pending.extend(
                row.kind
                    .graph_edges()
                    .into_iter()
                    .rev()
                    .map(|edge| (edge.ty, false)),
            );
        }
        Ok(())
    }
    // Both complete graphs must be valid even if their roots immediately differ.
    valid(a, a_root, a_row)?;
    valid(b, b_root, b_row)?;
    let root = if received {
        Variance::Covariant
    } else {
        Variance::Exact
    };
    // `header`: the row is a callable's parameter or result.
    let mut pending = vec![(a_root, b_root, payloads.is_some(), root, false)];
    let mut seen = HashSet::new();
    while let Some((left, right, view, variance, header)) = pending.pop() {
        if !seen.insert((left, right, view, variance, header)) {
            continue;
        }
        let children_view = view && payloads.is_some_and(|ids| ids.contains(&right));
        let adjusted_root =
            left == a_root && right == b_root && (a_row.is_some() || b_row.is_some());
        let left = row(a, left, a_root, a_row)?;
        let right = row(b, right, b_root, b_row)?;
        let mut expected = main_row_data(left, a_root);
        let mut actual = main_row_data(right, a_root);
        if (a_row.is_some() || b_row.is_some())
            && variance == Variance::Exact
            && (!matches!(
                actual.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) || expected.sendability_requirement == SendabilityRequirement::Sendable)
            && expected.sendability_requirement == actual.sendability_requirement
        {
            // Mutable/owned structural equality compares fixed receiving
            // contracts. Current content is checked separately by receipt flow.
            actual.modes.sendability = expected.modes.sendability.clone();
        }
        if forget_brands && left.brand.alternatives.is_empty() {
            actual.brand = Brand::default();
        }
        // MODE-1: an owned payload read through a view may be bounded by less
        // than its receiving row allows, as a received root may: a global
        // payload (a deep copy, say) fills a frame-local slot's payload.
        if view
            && actual.modes.relation != left.modes.relation
            && crate::properties::receives_relation(
                &left.modes.relation,
                &actual.modes.relation,
                crate::properties::TypeRelation::Value,
            )
        {
            actual.modes.relation = left.modes.relation.clone();
        }
        if view
            && left.modes.uniqueness == Uniqueness::Aliased
            && actual.modes.uniqueness == Uniqueness::Unique
        {
            actual.modes.uniqueness = Uniqueness::Aliased;
            if left.modes.sendability == Sendability::NotSendable {
                actual.modes.sendability = Sendability::NotSendable;
            }
        }
        // MODE-2: a callable's result may promise uniqueness its required
        // type does not, and a parameter may ask for less than it is given.
        // Only the header row's root weakens, as a received value's root does.
        if header && header_weakens_uniqueness(&expected, &actual, variance) {
            actual.modes.uniqueness = expected.modes.uniqueness;
        }
        let subsumed = if adjusted_root {
            true
        } else {
            match variance {
                Variance::Exact => true,
                Variance::Covariant => crate::properties::nested_promise(&expected, &mut actual),
                Variance::Contravariant => {
                    crate::properties::nested_promise(&actual, &mut expected)
                }
            }
        };
        if !subsumed || expected != actual {
            return Ok(false);
        }
        let children_header = matches!(left.kind, TypeKind::Function(_));
        pending.extend(
            left.kind
                .graph_edges()
                .into_iter()
                .zip(right.kind.graph_edges())
                .zip(variance.edges(&left.kind))
                .map(|((a, b), variance)| (a.ty, b.ty, children_view, variance, children_header)),
        );
    }
    Ok(true)
}

pub fn compatible_type(
    source: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    compatible(
        source, expected, actual, relation, false, None, true, generics, lookup,
    )
}

/// The relation for a `mut` argument. The callee writes the caller's storage
/// back through its parameter, so a callable, an erased state or a cleanup
/// summary can promise neither more nor less than the parameter declares, at
/// the root or below it: every nested contract is exact.
pub fn compatible_handback_type(
    source: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    compatible(
        source,
        expected,
        actual,
        TypeRelation::Parameter,
        false,
        None,
        false,
        generics,
        lookup,
    )
}

/// The same non-writing view relation with an explicit root reception policy.
/// Owned receivers use Value so an arena relation cannot disappear into storage;
/// readonly arguments use Parameter. Mut hand-back keeps compatible_type.
pub fn compatible_view_type_in(
    source: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    if relation == TypeRelation::Exact {
        return compatible_type(source, expected, actual, relation, generics, lookup);
    }
    let data = structural_equality(source, actual, Bound::Data, generics, lookup)?;
    // Only owned payload edges are covariant. A callable's header, phantom
    // argument or interior-mutable cell remains the same contract even when
    // the containing value is taken. Nominal Data excludes hidden callbacks.
    let mut payloads = HashSet::new();
    let mut pending = vec![actual];
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = source.get(id) else { continue };
        let owned = match &row.kind {
            TypeKind::Tuple(_) | TypeKind::Box(_) | TypeKind::Record(_) | TypeKind::Variant(_) => {
                true
            }
            TypeKind::Builtin { kind, .. } => matches!(
                kind,
                BuiltinType::Array
                    | BuiltinType::List
                    | BuiltinType::Map
                    | BuiltinType::Set
                    | BuiltinType::Inline
                    | BuiltinType::Simd
                    | BuiltinType::Packed
                    | BuiltinType::Shared
                    | BuiltinType::Slots
            ),
            TypeKind::Named { .. } => {
                structural_equality(source, id, Bound::Data, generics, lookup)?
            }
            _ => false,
        };
        if owned {
            payloads.insert(id);
            pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
    }
    compatible(
        source,
        expected,
        actual,
        relation,
        data,
        Some(&payloads),
        true,
        generics,
        lookup,
    )
}

fn compatible(
    source: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    forget_brands: bool,
    payloads: Option<&HashSet<TypeId>>,
    // Nested rows may promise more in reading positions; false for hand-back.
    received: bool,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let store = source;
    if relation == TypeRelation::Exact {
        return Ok(semantic_equal(store, expected, store, actual).unwrap_or(false));
    }
    let Some(expected_row) = store.get(expected).cloned() else {
        return Ok(false);
    };
    let Some(mut actual_row) = store.get(actual).cloned() else {
        return Ok(false);
    };
    if expected_row.allocation_target.is_some()
        && expected_row.allocation_target != actual_row.allocation_target
    {
        return Ok(false);
    }
    actual_row.allocation_target = expected_row.allocation_target;
    let plain = structural_property(store, actual, StructuralProperty::Plain, generics, lookup)?;
    // A `mut` hand-back is written back through the parameter, so storage
    // that promised less than the parameter admits would return holding it:
    // the callable promise, the erased state's captures and the cleanup
    // summary are exact at the root as they are below it. A core operation
    // that only advances an erased state declares its parameter at the
    // argument's own captures, so it meets this rule by construction.
    let handed_back = received
        || (value_promises(&actual_row, &mut expected_row.clone())
            && actual_row.modes.multiplicity == expected_row.modes.multiplicity
            && (plain || crossing_requirement(&actual_row) == crossing_requirement(&expected_row)));
    let accepts = handed_back
        && value_promises(&expected_row, &mut actual_row)
        && receives_relation(
            &expected_row.modes.relation,
            &actual_row.modes.relation,
            relation,
        )
        && (expected_row.modes.uniqueness == Uniqueness::Aliased
            || actual_row.modes.uniqueness == Uniqueness::Unique
            || plain)
        && (expected_row.modes.multiplicity == Multiplicity::Once
            || actual_row.modes.multiplicity == Multiplicity::Many)
        && receives_crossing(
            &crossing_requirement(&expected_row),
            &actual_row.modes.sendability,
        );
    if !accepts {
        return Ok(false);
    }
    // Root brands may forget identity or join known alternatives. Empty actual
    // evidence cannot manufacture a branded origin; nested brands stay invariant.
    if !expected_row.brand.alternatives.is_empty()
        && (actual_row.brand.alternatives.is_empty()
            || !actual_row
                .brand
                .alternatives
                .iter()
                .all(|o| expected_row.brand.alternatives.contains(o)))
    {
        return Ok(false);
    }
    actual_row.brand = expected_row.brand.clone();
    actual_row.sendability_requirement = expected_row.sendability_requirement;
    actual_row.modes = expected_row.modes;
    Ok(main_directional_walk(
        store,
        expected,
        None,
        store,
        actual,
        Some(&actual_row),
        forget_brands,
        payloads,
        received,
    )
    .unwrap_or(false))
}

/// Replacement of caller-owned storage must supply a global value. The Local
/// row of a mut parameter or its projected part describes read access (OWN-8),
/// not permission to retain a view after the call (MODE-1, OWN-3).
pub fn compatible_storage_type(
    source: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    if !source
        .get(actual)
        .is_some_and(|row| row.modes.relation.is_global())
    {
        return Ok(false);
    }
    compatible_type(
        source,
        expected,
        actual,
        TypeRelation::Value,
        generics,
        lookup,
    )
}

/// Main's two-graph shape walk with holes.
pub fn shape_equal_with_holes(
    a: &TypeInterner,
    a_root: TypeId,
    b: &TypeInterner,
    b_root: TypeId,
    hole: impl Fn(&TypeKind) -> bool,
) -> Result<bool, GraphError> {
    fn valid(source: &TypeInterner, root: TypeId) -> Result<(), GraphError> {
        let mut pending = vec![(root, false)];
        let mut active = HashSet::new();
        let mut done = HashSet::new();
        while let Some((id, exit)) = pending.pop() {
            if exit {
                active.remove(&id);
                done.insert(id);
                continue;
            }
            if done.contains(&id) {
                continue;
            }
            if !active.insert(id) {
                return Err(GraphError::StructuralCycle(id));
            }
            let row = source.get(id).ok_or(GraphError::MissingType(id))?;
            pending.push((id, true));
            let mut edge = |t| pending.push((t, false));
            match &row.kind {
                TypeKind::Tuple(ts) => ts.iter().copied().for_each(edge),
                TypeKind::Record(fs) => fs.iter().for_each(|f| edge(f.ty)),
                TypeKind::Variant(vs) => vs.iter().flat_map(|v| &v.fields).for_each(|f| edge(f.1)),
                TypeKind::Box(t) => edge(*t),
                TypeKind::Function(f) => {
                    f.parameters.iter().for_each(|p| edge(p.ty));
                    edge(f.result);
                    if let Some(t) = f.error {
                        edge(t);
                    }
                }
                TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                    for argument in arguments {
                        if let GenericArgument::Type(t) = argument {
                            edge(*t);
                        }
                    }
                }
                TypeKind::Error => return Err(GraphError::RecoveryType(id)),
                TypeKind::Primitive(_) | TypeKind::Parameter(_) => (),
            }
        }
        Ok(())
    }
    fn arguments(
        a: &[GenericArgument],
        b: &[GenericArgument],
        pending: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| match (a, b) {
                (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                    pending.push((*a, *b));
                    true
                }
                (GenericArgument::Size(a), GenericArgument::Size(b)) => a == b,
                _ => false,
            })
    }
    valid(a, a_root)?;
    valid(b, b_root)?;
    let mut pending = vec![(a_root, b_root)];
    let mut seen = HashSet::new();
    while let Some((a_id, b_id)) = pending.pop() {
        if !seen.insert((a_id, b_id)) {
            continue;
        }
        let a = &a.get(a_id).ok_or(GraphError::MissingType(a_id))?.kind;
        let b = &b.get(b_id).ok_or(GraphError::MissingType(b_id))?.kind;
        if hole(a) || hole(b) {
            continue;
        }
        let equal = match (a, b) {
            (TypeKind::Primitive(a), TypeKind::Primitive(b)) => a == b,
            (TypeKind::Parameter(a), TypeKind::Parameter(b)) => a == b,
            (TypeKind::Tuple(a), TypeKind::Tuple(b)) => {
                pending.extend(a.iter().copied().zip(b.iter().copied()));
                a.len() == b.len()
            }
            (TypeKind::Box(a), TypeKind::Box(b)) => {
                pending.push((*a, *b));
                true
            }
            (
                TypeKind::Named {
                    definition: a,
                    arguments: aa,
                },
                TypeKind::Named {
                    definition: b,
                    arguments: ba,
                },
            ) => a == b && arguments(aa, ba, &mut pending),
            (
                TypeKind::Builtin {
                    kind: a,
                    arguments: aa,
                },
                TypeKind::Builtin {
                    kind: b,
                    arguments: ba,
                },
            ) => a == b && arguments(aa, ba, &mut pending),
            (TypeKind::Record(a), TypeKind::Record(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        pending.push((a.ty, b.ty));
                        a.name == b.name && a.growable == b.growable
                    })
            }
            (TypeKind::Variant(a), TypeKind::Variant(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        a.name == b.name
                            && a.fields.len() == b.fields.len()
                            && a.fields.iter().zip(&b.fields).all(|(a, b)| {
                                pending.push((a.1, b.1));
                                a.0 == b.0
                            })
                    })
            }
            (TypeKind::Function(a), TypeKind::Function(b)) => {
                if a.generics != b.generics || a.parameters.len() != b.parameters.len() {
                    return Ok(false);
                }
                pending.push((a.result, b.result));
                match (a.error, b.error) {
                    (Some(a), Some(b)) => pending.push((a, b)),
                    (None, None) => (),
                    _ => return Ok(false),
                }
                a.parameters.iter().zip(&b.parameters).all(|(a, b)| {
                    pending.push((a.ty, b.ty));
                    a.label == b.label && a.passing == b.passing
                })
            }
            _ => false,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}
