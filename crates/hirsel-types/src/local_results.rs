//! Signature-derived sharing obligations for local results (spec #43).
use crate::*;
use std::collections::BTreeSet;

/// Signature-derived proofs for call obligations and implicit result reads.
/// Do not install these as the body generic context: explicit copies, captures
/// and other operations still require written bounds.
/// The published signature remains unchanged. Under provisional spec #43,
/// result type parameters selected from borrowed storage require Copyable. A
/// result local to a `mut` or `take` parameter may be a value moved out of it,
/// which the body must own by its own means.
/// Nested generic callable binders introduce independent parameter ordinals.
pub fn local_result_generics(
    types: &TypeInterner,
    function: &FunctionType,
) -> Vec<GenericParameter> {
    let mut generics = function.generics.to_vec();
    let Some(result) = types.get(function.result) else {
        return generics;
    };
    let Some(sources) = result.modes.relation.named_parameters() else {
        return generics;
    };
    if !sources.iter().any(|source| {
        matches!(source,
        ParameterRef::Bound { depth: 0, parameter }
            if function.parameters.get(parameter.0 as usize)
                .is_some_and(|p| p.passing == hirsel_syntax::PassingMode::Borrow))
    }) {
        return generics;
    }
    let mut pending = vec![(function.result, 0)];
    let mut seen = std::collections::HashSet::new();
    while let Some((id, nested)) = pending.pop() {
        if !seen.insert((id, nested)) {
            continue;
        }
        let Some(row) = types.get(id) else { continue };
        match &row.kind {
            TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(nested) => {
                if let Some(GenericParameter::Type { bounds, .. }) =
                    generics.get_mut(p.parameter().0 as usize)
                {
                    if !bounds.contains(&Bound::Copyable) {
                        bounds.push(Bound::Copyable);
                    }
                }
            }
            TypeKind::Tuple(ts) => pending.extend(ts.iter().map(|t| (*t, nested))),
            TypeKind::Record(fs) => pending.extend(fs.iter().map(|f| (f.ty, nested))),
            TypeKind::Variant(vs) => pending.extend(
                vs.iter()
                    .flat_map(|v| v.fields.iter().map(|f| (f.1, nested))),
            ),
            TypeKind::Box(t) => pending.push((*t, nested)),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                pending.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some((*t, nested)),
                    GenericArgument::Size(_) => None,
                }));
            }
            TypeKind::Function(f) => {
                pending.extend(f.parameters.iter().map(|p| (p.ty, nested + 1)));
                pending.push((f.result, nested + 1));
                pending.extend(f.error.map(|t| (t, nested + 1)));
            }
            _ => (),
        }
    }
    generics
}

/// Body-local implicit return-read permissions derived solely from its signature.
pub fn local_result_permissions(
    types: &TypeInterner,
    function: &FunctionType,
    scope: SiteRef,
) -> BTreeSet<TypeParameterRef> {
    local_result_generics(types, function)
        .iter()
        .enumerate()
        .filter_map(|(ordinal, g)| {
            matches!(g, GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Copyable))
                .then_some(TypeParameterRef::body(scope, TypeParamId(ordinal as u32)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::PassingMode;
    fn row(kind: TypeKind) -> Type {
        Type {
            drop_allocation: DropAllocation::MayAllocate,
            ..Type::with_kind(kind)
        }
    }
    fn function(result: TypeId) -> FunctionType {
        FunctionType {
            state_promises: Default::default(),
            generics: vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            }]
            .into(),
            parameters: vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: result,
            }]
            .into(),
            result,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }
    }
    #[test]
    fn nested_generic_result_does_not_bind_an_outer_ordinal() {
        let mut types = TypeStore::default();
        let t = types.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let mut nested = row(TypeKind::Function(function(t)));
        nested.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let nested = types.intern(nested);
        let f = function(nested);
        assert_eq!(local_result_generics(&types, &f), f.generics);
        // A reference crossing the nongeneric callable identifies the outer binder.
        let t = types.intern(row(TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })));
        let mut callable = function(t);
        callable.generics.clear();
        let mut result = row(TypeKind::Function(callable));
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let result = types.intern(result);
        let f = function(result);
        assert!(matches!(&local_result_generics(&types, &f)[0],
            GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Copyable)));
    }
}

/// PROV-4: a result related to parameters is placed where its sources agree.
/// When every named source reaches the same arena, that arena joins the
/// result's relation, so it is the destination every body, call and
/// independent check derives (`destination`). A source reaching no arena adds
/// no bound; distinct arenas or sources without an arena leave the
/// destination to MEM-10. The join is read from the signature alone.
pub fn local_result_destination(
    types: &TypeInterner,
    result: TypeId,
    parameters: &[Parameter],
    arena: impl Fn(&Origin) -> bool,
) -> TypeId {
    let Some(mut row) = types.get(result).cloned() else {
        return result;
    };
    let Some(sources) = row.modes.relation.named_parameters() else {
        return result;
    };
    if !row.modes.relation.arenas(&arena).is_empty() {
        return result;
    }
    // None: no source yet; Some(None): only global sources, which add no bound.
    let mut joined: Option<Option<Origin>> = None;
    for source in &sources {
        let ParameterRef::Bound {
            depth: 0,
            parameter,
        } = source
        else {
            return result;
        };
        let Some(p) = parameters.get(parameter.0 as usize) else {
            return result;
        };
        let own = Origin {
            root: OriginRoot::Parameter(*source),
            fields: Default::default(),
        };
        let relation = types
            .get(p.ty)
            .map(|t| t.modes.relation.clone())
            .unwrap_or_default();
        let placed = if arena(&own) {
            Some(own)
        } else if let Some(origin) = relation.arenas(&arena).first() {
            Some((*origin).clone())
        } else if p.passing == hirsel_syntax::PassingMode::Take && relation.is_global() {
            None
        } else {
            // A local source without an arena leaves the destination to MEM-10.
            return result;
        };
        joined = Some(match (joined, placed) {
            (None, placed) => placed,
            (Some(None), placed) | (Some(placed), None) => placed,
            (Some(Some(old)), Some(new)) if old == new => Some(old),
            _ => return result,
        });
    }
    match joined {
        Some(Some(origin)) => {
            row.modes.relation = row.modes.relation.union(&Relation::place(origin));
            types.intern(row)
        }
        _ => result,
    }
}
