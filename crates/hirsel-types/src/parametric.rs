//! MODE-2 parametricity: what a generic signature can do with the values of a
//! type parameter it has no permission to copy.
use crate::*;
use std::collections::BTreeSet;

/// How one argument of a parametric call supplies the result's content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParametricInput {
    /// The parameter's type names no parameter the result holds.
    Free,
    /// A taken or `mut` argument whose content may move into the result: it
    /// must be currently unique.
    Unique,
    /// A function argument that only receives held values by `take`: its
    /// instantiated result must be declared `unique`.
    UniqueResult,
}

/// MODE-2: a callee that gives no copying permission to the type parameters
/// its owned result holds can only move their values, never alias them. When
/// the result holds no other counted content, its content is exactly values
/// the arguments supply, so the result is unique whenever every supplying
/// argument is: the inputs, one per parameter, say which must be. `None` when
/// the signature does not qualify. `checked` says the callee has a checked
/// body; a bodyless core or foreign declaration may keep or hand back a share
/// of a borrowed argument.
pub fn parametric_inputs(
    store: &TypeStore,
    function: &FunctionType,
    checked: bool,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Option<Vec<ParametricInput>> {
    if function.foreign.is_some() || !store.get(function.result)?.modes.relation.is_global() {
        return None;
    }
    let mut held = BTreeSet::new();
    collect_parameters(store, function.result, 0, &mut held);
    if held.is_empty()
        || !held.is_disjoint(&sharing_type_parameters(store, function))
        || held.iter().any(|p| {
            !matches!(
                function.generics.get(p.0 as usize),
                Some(GenericParameter::Type { .. })
            )
        })
    {
        return None;
    }
    // Held parameters count as plain data, so the result is plain exactly when
    // they carry all of its counted content.
    let generics: Vec<_> = function
        .generics
        .iter()
        .enumerate()
        .map(|(ordinal, parameter)| match parameter {
            GenericParameter::Type { name, bounds }
                if held.contains(&TypeParamId(ordinal as u32)) =>
            {
                let mut bounds = bounds.to_vec();
                bounds.extend([Bound::Plain, Bound::Data]);
                GenericParameter::Type {
                    name: name.clone(),
                    bounds: bounds.into(),
                }
            }
            parameter => parameter.clone(),
        })
        .collect();
    let declarations = GenericDeclarations::signature(&generics);
    let plain = |root: TypeId, lookup: &mut _| {
        structural_property(
            store,
            root,
            StructuralProperty::Plain,
            &declarations,
            lookup,
        )
        .unwrap_or(false)
    };
    if !plain(function.result, lookup) {
        return None;
    }
    let mut inputs = Vec::with_capacity(function.parameters.len());
    for parameter in function.parameters.iter() {
        if !mentions(store, parameter.ty, 0, &held) {
            inputs.push(ParametricInput::Free);
            continue;
        }
        let row = store.get(parameter.ty)?;
        if let TypeKind::Function(callback) = &row.kind {
            // A callback that takes a held value owns it. One lent a held
            // value may copy it, so it must have nowhere to keep a copy: no
            // authority, no counted `mut` parameter, and a plain result and
            // error, so it supplies nothing (its environment is fixed, CLO-1).
            let mut lent = false;
            for p in callback.parameters.iter() {
                if !mentions(store, p.ty, 1, &held) {
                    continue;
                }
                if !data_shape(store, p.ty, 1, &held, lookup) {
                    return None;
                }
                match p.passing {
                    PassingMode::Take => {}
                    PassingMode::Borrow => lent = true,
                    PassingMode::Mut => return None,
                }
            }
            if lent {
                let kept = callback
                    .parameters
                    .iter()
                    .any(|p| p.passing == PassingMode::Mut && !closed_plain(store, p.ty, lookup));
                if kept
                    || !closed_plain(store, callback.result, lookup)
                    || callback
                        .error
                        .is_some_and(|e| !closed_plain(store, e, lookup))
                    || !no_authority(callback)
                {
                    return None;
                }
                inputs.push(ParametricInput::Free);
                continue;
            }
            if !data_shape(store, callback.result, 1, &held, lookup) {
                return None;
            }
            inputs.push(ParametricInput::UniqueResult);
            continue;
        }
        // A handle could hand a held value over, so only data may name a held
        // parameter. A checked body cannot move a held value out of a borrowed
        // argument, copy one, or return what it derives from the borrow (it
        // stays local, and the result is global), so a borrowed argument
        // supplies nothing.
        if !data_shape(store, parameter.ty, 0, &held, lookup) {
            return None;
        }
        if parameter.passing == PassingMode::Borrow {
            if !checked {
                return None;
            }
            inputs.push(ParametricInput::Free);
            continue;
        }
        inputs.push(ParametricInput::Unique);
    }
    Some(inputs)
}

/// MODE-2: an instantiated callback result proves uniqueness when its row is
/// written `unique`, or when it is a tuple, an inline aggregate with no block
/// of its own, whose every part does so or is plain.
pub fn unique_rows(
    store: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> bool {
    let Some(row) = store.get(root) else {
        return false;
    };
    if row.modes.uniqueness == Uniqueness::Unique {
        return true;
    }
    let TypeKind::Tuple(parts) = &row.kind else {
        return false;
    };
    parts.iter().all(|part| {
        unique_rows(store, *part, generics, lookup)
            || structural_property(store, *part, StructuralProperty::Plain, generics, lookup)
                .unwrap_or(false)
    })
}

/// `root` names no type parameter and is plain, so a value of it holds no
/// counted content, whatever the instantiation.
fn closed_plain(
    store: &TypeStore,
    root: TypeId,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> bool {
    !names_parameter(store, root)
        && structural_property(
            store,
            root,
            StructuralProperty::Plain,
            &GenericDeclarations::signature(&[]),
            lookup,
        )
        .unwrap_or(false)
}

/// The callable's `uses` grants no authority.
fn no_authority(function: &FunctionType) -> bool {
    matches!(&function.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty())
}

/// Whether any type parameter, of any binder, occurs in `root`.
fn names_parameter(store: &TypeStore, root: TypeId) -> bool {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id) else {
            return true;
        };
        match &row.kind {
            TypeKind::Parameter(_) => return true,
            TypeKind::Tuple(parts) => pending.extend(parts.iter().copied()),
            TypeKind::Box(t) => pending.push(*t),
            TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
            TypeKind::Variant(variants) => {
                pending.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
            }
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => pending
                .extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                })),
            TypeKind::Function(f) => {
                pending.extend(f.parameters.iter().map(|p| p.ty));
                pending.push(f.result);
                pending.extend(f.error);
            }
            TypeKind::Primitive(_) | TypeKind::Error => {}
        }
    }
    false
}

/// Every part of `root` that names a held parameter is data: no function,
/// handle, iterator or other builtin that could lend or hand over a held value.
fn data_shape(
    store: &TypeStore,
    root: TypeId,
    depth: u32,
    held: &BTreeSet<TypeParamId>,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> bool {
    data_shape_within(store, root, depth, held, lookup, &mut BTreeSet::new())
}

/// `data_shape`, entering each nominal declaration once per set of held
/// parameters: a recursive occurrence is data when the rest of it is.
fn data_shape_within(
    store: &TypeStore,
    root: TypeId,
    depth: u32,
    held: &BTreeSet<TypeParamId>,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
    entered: &mut BTreeSet<(hirsel_syntax::DefPath, BTreeSet<TypeParamId>)>,
) -> bool {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) || !mentions(store, id, depth, held) {
            continue;
        }
        let Some(row) = store.get(id) else {
            return false;
        };
        match &row.kind {
            TypeKind::Parameter(_) => {}
            TypeKind::Tuple(parts) => pending.extend(parts.iter().copied()),
            TypeKind::Box(t) => pending.push(*t),
            TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
            TypeKind::Variant(variants) => {
                pending.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
            }
            TypeKind::Builtin {
                kind: BuiltinType::List | BuiltinType::Map | BuiltinType::Set | BuiltinType::Array,
                arguments,
            } => pending.extend(arguments.iter().filter_map(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                GenericArgument::Size(_) => None,
            })),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                // The declaration's parts that name a parameter instantiated
                // with a held one must be data too; other parts cannot hold,
                // lend or hand over a held value. An opaque declaration hides
                // its parts.
                let Ok(Some(signature)) = lookup(definition) else {
                    return false;
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return false;
                };
                let Some(representation) = declaration.representation else {
                    return false;
                };
                let inner: BTreeSet<_> = arguments
                    .iter()
                    .enumerate()
                    .filter_map(|(ordinal, argument)| match argument {
                        GenericArgument::Type(t) if mentions(store, *t, depth, held) => {
                            Some(TypeParamId(ordinal as u32))
                        }
                        _ => None,
                    })
                    .collect();
                if entered.insert((definition.clone(), inner.clone()))
                    && !data_shape_within(
                        &signature.types,
                        representation,
                        0,
                        &inner,
                        lookup,
                        entered,
                    )
                {
                    return false;
                }
                pending.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                }));
            }
            _ => return false,
        }
    }
    true
}

fn collect_parameters(
    store: &TypeStore,
    root: TypeId,
    depth: u32,
    found: &mut BTreeSet<TypeParamId>,
) {
    walk(store, root, depth, &mut |p| {
        found.insert(p);
    });
}

fn mentions(store: &TypeStore, root: TypeId, depth: u32, held: &BTreeSet<TypeParamId>) -> bool {
    let mut any = false;
    walk(store, root, depth, &mut |p| any |= held.contains(&p));
    any
}

/// Every parameter of the binder `depth` levels above `root`.
fn walk(store: &TypeStore, root: TypeId, depth: u32, visit: &mut impl FnMut(TypeParamId)) {
    let mut pending = vec![(root, depth)];
    let mut seen = BTreeSet::new();
    while let Some((id, depth)) = pending.pop() {
        if !seen.insert((id, depth)) {
            continue;
        }
        let Some(row) = store.get(id) else { continue };
        match &row.kind {
            TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(depth) => {
                visit(p.parameter())
            }
            TypeKind::Tuple(parts) => pending.extend(parts.iter().map(|t| (*t, depth))),
            TypeKind::Box(t) => pending.push((*t, depth)),
            TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| (f.ty, depth))),
            TypeKind::Variant(variants) => pending.extend(
                variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(move |f| (f.1, depth))),
            ),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => pending
                .extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some((*t, depth)),
                    GenericArgument::Size(_) => None,
                })),
            TypeKind::Function(f) => {
                pending.extend(f.parameters.iter().map(|p| (p.ty, depth + 1)));
                pending.push((f.result, depth + 1));
                pending.extend(f.error.map(|e| (e, depth + 1)));
            }
            TypeKind::Primitive(_) | TypeKind::Parameter(_) | TypeKind::Error => {}
        }
    }
}
