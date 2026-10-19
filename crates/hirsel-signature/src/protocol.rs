//! The second declaration stage runs after nominal headers have been published.
use super::*;

/// Validate an association even when no loop uses the nominal declaration.
/// Name resolution (stage one) never reads these operation headers.
pub fn validate_loop_source(
    signature: &Signature,
    span: Span,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    let SignatureKind::Type(nominal) = &signature.kind else {
        return Ok(());
    };
    let Some(association) = &nominal.loop_source else {
        return Ok(());
    };
    let open = provider.signature(&association.open)?;
    let next = provider.signature(&association.next)?;
    let (SignatureKind::Function(open_f), SignatureKind::Function(next_f)) =
        (&open.kind, &next.kind)
    else {
        return Err(diagnostic(
            const { RuleId::of("SEM-4") },
            span,
            "loop association operations must be functions",
        ));
    };
    if !matches!(open_f.parameters.as_slice(), [parameter] if matches!(open.types.get(parameter.ty), Some(Type { kind: TypeKind::Named { definition, .. }, .. }) if definition == &signature.definition))
    {
        return Err(diagnostic(
            const { RuleId::of("SEM-4") },
            span,
            "loop open must borrow the associated nominal receiver",
        ));
    }
    if signature.metadata.visibility != ast::Visibility::Private {
        for (operation, function) in [(&open, open_f), (&next, next_f)] {
            let mut pending: Vec<_> = function
                .parameters
                .iter()
                .map(|p| p.ty)
                .chain([function.result])
                .chain(function.error)
                .collect();
            let mut seen = BTreeSet::new();
            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) {
                    continue;
                }
                let row = operation.types.get(ty).ok_or_else(|| {
                    diagnostic(
                        const { RuleId::of("SEM-4") },
                        span,
                        "invalid association signature graph",
                    )
                })?;
                pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
                if let TypeKind::Named { definition, .. } = &row.kind {
                    if provider.signature(definition)?.metadata.visibility
                        == ast::Visibility::Private
                    {
                        return Err(diagnostic(
                            const { RuleId::of("PROG-2") },
                            span,
                            "a public loop association requires exported receiver, cursor and yield types",
                        ));
                    }
                }
            }
        }
    }
    let mut types = TypeStore::default();
    let receiver = symbolic_receiver(signature, nominal, &mut types);
    let loop_error = || {
        diagnostic(
            const { RuleId::of("SEM-4") },
            span,
            "loop operation receiver, arity or passing mode disagrees with its association",
        )
    };
    let cursor = specialize(
        &open.types,
        open_f,
        &mut types,
        receiver,
        PassingMode::Borrow,
        &nominal.generics,
        provider,
        0,
        &loop_error,
    )?;
    if !types
        .get(cursor)
        .unwrap()
        .modes
        .relation
        .parameter_roots()
        .contains(&ParameterRef::bound(ParamId(0)))
    {
        return Err(diagnostic(
            const { RuleId::of("SEM-4") },
            span,
            "loop cursor must retain its borrowed receiver",
        ));
    }
    let result = specialize(
        &next.types,
        next_f,
        &mut types,
        cursor,
        PassingMode::Mut,
        &nominal.generics,
        provider,
        0,
        &loop_error,
    )?;
    let core_option = ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: "Option".into(),
    });
    if association.option != core_option
        || !matches!(&types.get(result).unwrap().kind, TypeKind::Named { definition, arguments } if definition == &core_option && matches!(arguments.as_slice(), [GenericArgument::Type(_)]))
    {
        return Err(diagnostic(
            const { RuleId::of("SEM-4") },
            span,
            "loop next must return the exact core Option",
        ));
    }
    Ok(())
}

/// Validate an `@format(function)` association even when no f-string formats
/// the type (SEM-3). After substituting the type's own generic parameters, the
/// function is exactly `fn(value: T, mut out: List[u8]) -> ()`: an unlabelled
/// borrowed receiver, an unlabelled `mut` UTF-8 builder implicitly preserved unique,
/// and no error result.
pub fn validate_format(
    signature: &Signature,
    span: Span,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    let SignatureKind::Type(nominal) = &signature.kind else {
        return Ok(());
    };
    let Some(path) = &nominal.format else {
        return Ok(());
    };
    let error = || {
        diagnostic(
            const { RuleId::of("SEM-3") },
            span,
            "an @format function is `fn(value: T, mut out: List[u8]) -> ()` for its type T, \
             generic over T's own parameters, with no error result",
        )
    };
    let function = provider.signature(path)?;
    let SignatureKind::Function(f) = &function.kind else {
        return Err(error());
    };
    let mut types = TypeStore::default();
    let receiver = symbolic_receiver(signature, nominal, &mut types);
    let result = specialize(
        &function.types,
        f,
        &mut types,
        receiver,
        PassingMode::Borrow,
        &nominal.generics,
        provider,
        1,
        &error,
    )?;
    let out = &f.parameters[1];
    let builder = |ty: TypeId| {
        let byte = |argument: &GenericArgument| matches!(argument, GenericArgument::Type(t) if matches!(function.types.get(*t).map(|r| &r.kind), Some(TypeKind::Primitive(Primitive::U8))));
        matches!(function.types.get(ty).map(|r| &r.kind), Some(TypeKind::Builtin { kind: BuiltinType::List, arguments }) if matches!(arguments.as_slice(), [a] if byte(a)))
    };
    if out.passing != PassingMode::Mut
        || out.label.is_some()
        || !builder(out.ty)
        || f.error.is_some()
        || !matches!(&types.get(result).unwrap().kind, TypeKind::Tuple(items) if items.is_empty())
    {
        return Err(error());
    }
    Ok(())
}
/// Validate a `@serializable(check = function)` association even when nothing decodes
/// the type (SER-1). The function is declared in the type's own module with exactly the
/// header `fn(value: T) -> bool uses {}`: one unlabelled borrowed receiver of the type,
/// no generics, no error result, `@noalloc` and no foreign contract.
pub fn validate_serializable_check(
    signature: &Signature,
    span: Span,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    let SignatureKind::Type(nominal) = &signature.kind else {
        return Ok(());
    };
    let Some(check) = &nominal.serializable_check else {
        return Ok(());
    };
    let error = || {
        diagnostic(
            const { RuleId::of("SER-1") },
            span,
            "a serializable check is `@noalloc fn(value: T) -> bool uses {}` in T's own module",
        )
    };
    let path = &check.function;
    if path.0.package != signature.definition.0.package
        || path.0.modules != signature.definition.0.modules
    {
        return Err(error());
    }
    let function = provider.signature(path)?;
    let SignatureKind::Function(f) = &function.kind else {
        return Err(error());
    };
    let kind = |ty: TypeId| function.types.get(ty).map(|r| &r.kind);
    let receiver = match f.parameters.as_slice() {
        [p] if p.passing == PassingMode::Borrow && p.label.is_none() => kind(p.ty),
        _ => None,
    };
    if !f.generics.is_empty()
        || !matches!(receiver, Some(TypeKind::Named { definition, arguments })
            if *definition == signature.definition && arguments.is_empty())
        || !matches!(kind(f.result), Some(TypeKind::Primitive(Primitive::Bool)))
        || f.error.is_some()
        || f.allocation != Allocation::NoAlloc
        || f.uses != Uses::Only(AuthoritySummary::inert())
        || f.foreign.is_some()
        || f.incomplete
    {
        return Err(error());
    }
    Ok(())
}
/// The declaring type applied to its own generic parameters, as the
/// association's operations must accept it.
fn symbolic_receiver(
    signature: &Signature,
    nominal: &TypeDefinition,
    types: &mut TypeStore,
) -> TypeId {
    let arguments = nominal
        .generics
        .iter()
        .enumerate()
        .map(|(ordinal, g)| match g {
            GenericParameter::Type { .. } => GenericArgument::Type(types.push(Type {
                drop_allocation: DropAllocation::DependsOn(
                    vec![TypeParameterRef::bound(TypeParamId(ordinal as u32))].into(),
                ),
                ..Type::with_kind(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(
                    ordinal as u32,
                ))))
            })),
            GenericParameter::Size { .. } => GenericArgument::Size(Size::Parameter(
                SizeParameterRef::bound(SizeParamId(ordinal as u32)),
            )),
        })
        .collect();
    types.push(Type {
        drop_allocation: DropAllocation::MayAllocate,
        ..Type::with_kind(TypeKind::Named {
            definition: signature.definition.clone(),
            arguments,
        })
    })
}

fn specialize(
    source: &TypeStore,
    function: &FunctionType,
    types: &mut TypeStore,
    actual: TypeId,
    passing: PassingMode,
    receiver_generics: &[GenericParameter],
    provider: &mut dyn SignatureProvider,
    extra: usize,
    error: &dyn Fn() -> QueryError,
) -> Result<TypeId, QueryError> {
    let [parameter, rest @ ..] = function.parameters.as_slice() else {
        return Err(error());
    };
    if rest.len() != extra {
        return Err(error());
    }
    if parameter.passing != passing || parameter.label.is_some() {
        return Err(error());
    }
    let formal = import_type(source, parameter.ty, types).map_err(|_| error())?;
    let mut inferred = vec![None; function.generics.len()];
    let mut pending = vec![(formal, actual, 0)];
    let mut seen = BTreeSet::new();
    while let Some((formal, actual, depth)) = pending.pop() {
        if !seen.insert((formal, actual, depth)) {
            continue;
        }
        let a = types.get(formal).unwrap();
        let b = types.get(actual).unwrap();
        if let TypeKind::Parameter(p) = a.kind
            && p.binder() == GenericBinder::Bound(depth)
        {
            let actual = leave_binders(types, actual, depth).map_err(|_| error())?;
            let slot = inferred
                .get_mut(p.parameter().0 as usize)
                .ok_or_else(error)?;
            if let Some(GenericArgument::Type(old)) = slot {
                if !shape_equal(types, *old, types, actual).unwrap_or(false) {
                    return Err(error());
                }
            } else if slot.is_some() {
                return Err(error());
            } else {
                *slot = Some(GenericArgument::Type(actual))
            }
            continue;
        }
        match (&a.kind, &b.kind) {
            (
                TypeKind::Named {
                    definition: x,
                    arguments: xs,
                },
                TypeKind::Named {
                    definition: y,
                    arguments: ys,
                },
            ) if x == y => {
                infer_actuals(xs, ys, depth, &mut inferred, &mut pending).ok_or_else(error)?
            }
            (
                TypeKind::Builtin {
                    kind: x,
                    arguments: xs,
                },
                TypeKind::Builtin {
                    kind: y,
                    arguments: ys,
                },
            ) if x == y => {
                infer_actuals(xs, ys, depth, &mut inferred, &mut pending).ok_or_else(error)?
            }
            (TypeKind::Tuple(xs), TypeKind::Tuple(ys)) if xs.len() == ys.len() => {
                pending.extend(xs.iter().zip(ys).map(|(x, y)| (*x, *y, depth)))
            }
            (TypeKind::Box(x), TypeKind::Box(y)) => pending.push((*x, *y, depth)),
            (TypeKind::Function(x), TypeKind::Function(y))
                if x.generics == y.generics && x.parameters.len() == y.parameters.len() =>
            {
                for (x, y) in x.parameters.iter().zip(&y.parameters) {
                    if x.passing != y.passing || x.label != y.label {
                        return Err(error());
                    }
                    pending.push((x.ty, y.ty, depth + 1));
                }
                pending.push((x.result, y.result, depth + 1));
            }
            _ if shape_equal(types, formal, types, actual).unwrap_or(false) => (),
            _ => return Err(error()),
        }
    }
    let arguments: Option<Vec<_>> = inferred.into_iter().collect();
    let arguments = arguments.ok_or_else(error)?;
    let declarations = GenericDeclarations::signature(receiver_generics);
    for (generic, actual) in function.generics.iter().zip(&arguments) {
        match (generic, actual) {
            (GenericParameter::Size { .. }, GenericArgument::Size(_)) => (),
            (GenericParameter::Type { bounds, .. }, GenericArgument::Type(ty)) => {
                for bound in bounds {
                    let lookup = &mut |path: &ast::DefPath| provider.signature(path).map(Some);
                    let valid = match bound {
                        Bound::Serializable => {
                            structural_serializable(types, *ty, &declarations, lookup)?
                        }
                        Bound::Plain | Bound::Copyable | Bound::Sendable => structural_property(
                            types,
                            *ty,
                            match bound {
                                Bound::Plain => StructuralProperty::Plain,
                                Bound::Copyable => StructuralProperty::Copyable,
                                _ => StructuralProperty::Sendable,
                            },
                            &declarations,
                            lookup,
                        )?,
                        Bound::Flat => structural_flat(types, *ty, &declarations, lookup)?,
                        Bound::Data | Bound::Key => {
                            structural_equality(types, *ty, *bound, &declarations, lookup)?
                        }
                        Bound::Inert => {
                            structural_authority(
                                types,
                                *ty,
                                &NominalRoles::default(),
                                &declarations,
                                &mut |_, path| match path {
                                    Some(path) => provider.signature(path).map(Some),
                                    None => Ok(None),
                                },
                            )? == AuthoritySummary::inert()
                        }
                        Bound::NoallocDrop => match &types.get(*ty).unwrap().drop_allocation {
                            DropAllocation::NoAlloc => true,
                            DropAllocation::DependsOn(ps) => ps.iter().all(|p| {
                                declarations
                                    .bounds(*p)
                                    .iter()
                                    .any(|b| matches!(b, Bound::NoallocDrop | Bound::Plain))
                            }),
                            _ => false,
                        },
                    };
                    if !valid {
                        return Err(error());
                    }
                }
            }
            _ => return Err(error()),
        }
    }
    let parameter = substitute_type(
        source,
        parameter.ty,
        types,
        Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &NominalRoles::default(),
            arguments: &arguments,
            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .map_err(|_| error())?;
    // The symbolic nominal receiver starts without a cleanup summary. Its
    // parameter row supplies that type-owned summary, not a value-flow proof.
    let actual = if passing == PassingMode::Borrow {
        let mut row = types.get(actual).unwrap().clone();
        row.drop_allocation = types.get(parameter).unwrap().drop_allocation.clone();
        types.push(row)
    } else {
        actual
    };
    let lookup = &mut |path: &ast::DefPath| provider.signature(path).map(Some);
    let compatible = if passing == PassingMode::Mut {
        compatible_handback_type(types, parameter, actual, &declarations, lookup)?
    } else {
        compatible_view_type(types, parameter, actual, &declarations, lookup)?
    };
    if !compatible {
        return Err(error());
    }
    substitute_type(
        source,
        function.result,
        types,
        Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &NominalRoles::default(),
            arguments: &arguments,
            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .map_err(|_| error())
}
fn infer_actuals(
    a: &[GenericArgument],
    b: &[GenericArgument],
    depth: u32,
    inferred: &mut [Option<GenericArgument>],
    pending: &mut Vec<(TypeId, TypeId, u32)>,
) -> Option<()> {
    if a.len() != b.len() {
        return None;
    }
    for (a, b) in a.iter().zip(b) {
        match (a, b) {
            (GenericArgument::Type(a), GenericArgument::Type(b)) => pending.push((*a, *b, depth)),
            (GenericArgument::Size(Size::Parameter(p)), b @ GenericArgument::Size(_))
                if p.binder() == GenericBinder::Bound(depth) =>
            {
                let b = match b {
                    GenericArgument::Size(Size::Parameter(q)) => match q.binder() {
                        GenericBinder::Bound(d) if d >= depth => {
                            GenericArgument::Size(Size::Parameter(SizeParameterRef::Bound {
                                depth: d - depth,
                                parameter: q.parameter(),
                            }))
                        }
                        GenericBinder::Bound(_) => return None,
                        _ => b.clone(),
                    },
                    _ => b.clone(),
                };
                let slot = inferred.get_mut(p.parameter().0 as usize)?;
                if slot.as_ref().is_some_and(|old| old != &b) {
                    return None;
                }
                *slot = Some(b.clone());
            }
            (a, b) if a == b => (),
            _ => return None,
        }
    }
    Some(())
}

// Inference can descend into a callable, but its actual must leave that lexical
// context before becoming an operation generic. Inner references cannot escape.
fn leave_binders(store: &mut TypeStore, root: TypeId, count: u32) -> Result<TypeId, GraphError> {
    if count == 0 {
        return Ok(root);
    }
    let root = map_parameter_references(store, root, |p, nested| match p {
        ParameterRef::Bound { depth, parameter } if depth >= nested + count => {
            Ok(ParameterRef::Bound {
                depth: depth - count,
                parameter,
            })
        }
        ParameterRef::Bound { depth, .. } if depth >= nested => {
            Err(GraphError::UnboundParameter(p))
        }
        p => Ok(p),
    })?;
    map_generic_binders(store, root, |binder, nested| match binder {
        GenericBinder::Bound(depth) if depth >= nested + count => {
            Ok(GenericBinder::Bound(depth - count))
        }
        GenericBinder::Bound(depth) if depth >= nested => Err(GraphError::UnboundGeneric(binder)),
        binder => Ok(binder),
    })
}
