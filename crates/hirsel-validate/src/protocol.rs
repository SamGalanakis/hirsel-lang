//! Association evidence is reconstructed from immutable declarations and calls.
use super::*;
use hirsel_syntax::PassingMode;
use hirsel_types::*;

fn option_path() -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: "Option".into(),
    })
}
pub(super) fn declaration(
    t: &TypeInterner,
    memo: &TypeMemo,
    signature: &Signature,
    signatures: &Reader<'_>,
) -> bool {
    let SignatureKind::Type(nominal) = &signature.kind else {
        return true;
    };
    let Some(association) = &nominal.loop_source else {
        return true;
    };
    if association.option != option_path() {
        return false;
    }
    let (Some(open), Some(next)) = (
        signatures.get(&association.open),
        signatures.get(&association.next),
    ) else {
        return false;
    };
    let (SignatureKind::Function(open_f), SignatureKind::Function(next_f)) =
        (&open.kind, &next.kind)
    else {
        return false;
    };
    if signature.metadata.visibility != hirsel_syntax::Visibility::Private {
        for function in [open_f, next_f] {
            let mut pending: Vec<_> = function
                .parameters
                .iter()
                .map(|p| p.ty)
                .chain([function.result])
                .chain(function.error)
                .collect();
            let mut seen = HashSet::default();
            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) {
                    continue;
                }
                let Some(row) = t.get(ty) else {
                    return false;
                };
                pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
                if let TypeKind::Named { definition, .. } = &row.kind {
                    if !signatures.get(*definition).is_some_and(|s| {
                        s.metadata.visibility != hirsel_syntax::Visibility::Private
                    }) {
                        return false;
                    }
                }
            }
        }
    }
    let ([receiver], [cursor]) = (open_f.parameters.as_slice(), next_f.parameters.as_slice())
    else {
        return false;
    };
    if receiver.passing != PassingMode::Borrow
        || cursor.passing != PassingMode::Mut
        || receiver.label.is_some()
        || cursor.label.is_some()
    {
        return false;
    }
    let Some(receiver_row) = t.get(receiver.ty) else {
        return false;
    };
    if !matches!(&receiver_row.kind, TypeKind::Named { definition, arguments } if *definition == signature.definition && arguments.len() == nominal.generics.len())
    {
        return false;
    }
    let arguments = nominal
        .generics
        .iter()
        .enumerate()
        .map(|(ordinal, g)| match g {
            GenericParameter::Type { .. } => {
                let mut row = Type::unit();
                row.kind =
                    TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(ordinal as u32)));
                row.drop_allocation = DropAllocation::DependsOn(
                    vec![TypeParameterRef::bound(TypeParamId(ordinal as u32))].into(),
                );
                GenericArgument::Type(t.intern(row))
            }
            GenericParameter::Size { .. } => GenericArgument::Size(Size::Parameter(
                SizeParameterRef::bound(SizeParamId(ordinal as u32)),
            )),
        })
        .collect();
    let mut row = Type::unit();
    row.kind = TypeKind::Named {
        definition: signature.definition,
        arguments,
    };
    let receiver = t.intern(row);
    let Some(cursor) = instantiate(t, memo, open_f, receiver, &nominal.generics, signatures) else {
        return false;
    };
    if !t
        .get(cursor)
        .unwrap()
        .modes
        .relation
        .parameter_roots()
        .contains(&ParameterRef::bound(ParamId(0)))
    {
        return false;
    }
    let Some(result) = instantiate(t, memo, next_f, cursor, &nominal.generics, signatures) else {
        return false;
    };
    matches!(t.get(result), Some(Type { kind: TypeKind::Named { definition, arguments }, .. }) if *definition.path() == association.option && matches!(arguments.as_slice(), [GenericArgument::Type(_)]))
}

// V infers the two operation instantiations from declaration graphs, independently
// of H's checked actual vectors and the receipt's caller-supplied identity.
fn instantiate(
    t: &TypeInterner,
    memo: &TypeMemo,
    function: &FunctionType,
    actual: TypeId,
    receiver_generics: &[GenericParameter],
    signatures: &Reader<'_>,
) -> Option<TypeId> {
    let formal = function.parameters.first()?.ty;
    let mut arguments: Vec<Option<GenericArgument>> = vec![None; function.generics.len()];
    let mut pairs = vec![(formal, actual, 0)];
    let mut visited = HashSet::default();
    while let Some((left, right, depth)) = pairs.pop() {
        if !visited.insert((left, right, depth)) {
            continue;
        }
        let (left_row, right_row) = (t.get(left)?, t.get(right)?);
        if let TypeKind::Parameter(p) = left_row.kind
            && p.binder() == GenericBinder::Bound(depth)
        {
            let right = lift_actual(t, right, depth)?;
            let slot = arguments.get_mut(p.parameter().0 as usize)?;
            match slot {
                None => *slot = Some(GenericArgument::Type(right)),
                Some(GenericArgument::Type(previous)) if t.shape_eq(*previous, right).ok()? => (),
                _ => return None,
            }
            continue;
        }
        match (&left_row.kind, &right_row.kind) {
            (
                TypeKind::Named {
                    definition: a,
                    arguments: aa,
                },
                TypeKind::Named {
                    definition: b,
                    arguments: ba,
                },
            ) if a == b => actuals(aa, ba, depth, &mut arguments, &mut pairs)?,
            (
                TypeKind::Builtin {
                    kind: a,
                    arguments: aa,
                },
                TypeKind::Builtin {
                    kind: b,
                    arguments: ba,
                },
            ) if a == b => actuals(aa, ba, depth, &mut arguments, &mut pairs)?,
            (TypeKind::Tuple(a), TypeKind::Tuple(b)) if a.len() == b.len() => {
                pairs.extend(a.iter().zip(b).map(|(a, b)| (*a, *b, depth)))
            }
            (TypeKind::Box(a), TypeKind::Box(b)) => pairs.push((*a, *b, depth)),
            (TypeKind::Function(a), TypeKind::Function(b))
                if a.generics == b.generics && a.parameters.len() == b.parameters.len() =>
            {
                for (a, b) in a.parameters.iter().zip(&b.parameters) {
                    if a.passing != b.passing || a.label != b.label {
                        return None;
                    }
                    pairs.push((a.ty, b.ty, depth + 1));
                }
                pairs.push((a.result, b.result, depth + 1));
            }
            _ if t.shape_eq(left, right).ok()? => (),
            _ => return None,
        }
    }
    let arguments: Vec<_> = arguments.into_iter().collect::<Option<_>>()?;
    let context = GenericDeclarations::signature(receiver_generics);
    for (generic, actual) in function.generics.iter().zip(&arguments) {
        match (generic, actual) {
            (GenericParameter::Size { .. }, GenericArgument::Size(_)) => (),
            (GenericParameter::Type { bounds, .. }, GenericArgument::Type(ty)) => {
                for bound in bounds {
                    let lookup = &mut |path: DefId| {
                        Ok::<_, QueryError>(signatures.get(path).cloned().map(std::sync::Arc::new))
                    };
                    let valid = match bound {
                        Bound::Serializable => {
                            super::serialization::eligible(t, memo, *ty, &context, signatures)
                                .ok()?
                        }
                        Bound::Plain => {
                            structural_property(t, *ty, StructuralProperty::Plain, &context, lookup)
                                .ok()?
                        }
                        Bound::Copyable => structural_property(
                            t,
                            *ty,
                            StructuralProperty::Copyable,
                            &context,
                            lookup,
                        )
                        .ok()?,
                        Bound::Sendable => structural_property(
                            t,
                            *ty,
                            StructuralProperty::Sendable,
                            &context,
                            lookup,
                        )
                        .ok()?,
                        Bound::Flat => structural_flat(t, *ty, &context, lookup).ok()?,
                        Bound::Data | Bound::Key => {
                            structural_equality(t, *ty, *bound, &context, lookup).ok()?
                        }
                        Bound::Inert => {
                            structural_authority(
                                t,
                                *ty,
                                &NominalRoles::default(),
                                &context,
                                &mut |_, path| {
                                    Ok::<_, QueryError>(
                                        path.and_then(|p| signatures.get(p))
                                            .cloned()
                                            .map(std::sync::Arc::new),
                                    )
                                },
                            )
                            .ok()?
                                == AuthoritySummary::inert()
                        }
                        Bound::NoallocDrop => match &t.get(*ty)?.drop_allocation {
                            DropAllocation::NoAlloc => true,
                            DropAllocation::DependsOn(ps) => ps.iter().all(|p| {
                                context
                                    .bounds(*p)
                                    .iter()
                                    .any(|b| matches!(b, Bound::NoallocDrop | Bound::Plain))
                            }),
                            _ => false,
                        },
                    };
                    if !valid {
                        return None;
                    }
                }
            }
            _ => return None,
        }
    }
    let parameter = hirsel_types::substitute(
        t,
        memo,
        function.parameters.first()?.ty,
        &Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &NominalRoles::default(),
            arguments: &arguments,
            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .ok()?;
    // The symbolic nominal receiver starts without a cleanup summary. Its
    // parameter row supplies that type-owned summary, not a value-flow proof.
    let actual = if function.parameters.first()?.passing == PassingMode::Borrow {
        let mut row = t.get(actual)?.clone();
        row.drop_allocation = t.get(parameter)?.drop_allocation.clone();
        t.intern(row)
    } else {
        actual
    };
    let lookup = &mut |path: DefId| {
        Ok::<_, QueryError>(signatures.get(path).cloned().map(std::sync::Arc::new))
    };
    let mut memo_context = hirsel_types::ContextMemo::new(t, &context, lookup);
    let compatible = if function.parameters.first()?.passing == PassingMode::Mut {
        memo_context.handback(parameter, actual).ok()?
    } else {
        memo_context
            .view(parameter, actual, hirsel_types::TypeRelation::Parameter)
            .ok()?
    };
    if !compatible {
        return None;
    }
    hirsel_types::substitute(
        t,
        memo,
        function.result,
        &Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &NominalRoles::default(),
            arguments: &arguments,
            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .ok()
}
fn actuals(
    left: &[GenericArgument],
    right: &[GenericArgument],
    depth: u32,
    inferred: &mut [Option<GenericArgument>],
    pairs: &mut Vec<(TypeId, TypeId, u32)>,
) -> Option<()> {
    if left.len() != right.len() {
        return None;
    }
    for (left, right) in left.iter().zip(right) {
        match (left, right) {
            (GenericArgument::Type(a), GenericArgument::Type(b)) => pairs.push((*a, *b, depth)),
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
                if slot.as_ref().is_some_and(|a| a != &b) {
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

pub(super) fn receipts(
    t: &TypeInterner,
    memo: &TypeMemo,
    definition: &ReadDefinition<'_>,
    signatures: &Reader<'_>,
) -> Vec<Violation> {
    let mut errors = vec![];
    let mut seen = BTreeSet::new();
    for receipt in &definition.protocol_associations {
        let valid = (|| {
            if !seen.insert((receipt.body.0, receipt.open.0, receipt.next.0)) { return None }
            let signature = signatures.get(&receipt.association.source)?;
            if signature.fingerprint != receipt.association.fingerprint || !declaration(t, memo, signature, signatures) { return None }
            let SignatureKind::Type(nominal) = &signature.kind else { return None };
            let association = nominal.loop_source.as_ref()?;
            let body = definition.bodies.get(receipt.body.0 as usize)?;
            let open = body.blocks.get(receipt.open.0 as usize)?;
            let next = body.blocks.get(receipt.next.0 as usize)?;
            let Terminator::Call { callee: Callee::Definition { path: open_path, .. }, arguments: open_args, returns: Some(CallReturn { destination, .. }), .. } = &open.terminator else { return None };
            let Terminator::Call { callee: Callee::Definition { path: next_path, .. }, arguments: next_args, returns: Some(_), .. } = &next.terminator else { return None };
            if open_path != &association.open || next_path != &association.next { return None }
            let ([source], [cursor]) = (open_args.as_slice(), next_args.as_slice()) else { return None };
            if source.value.usage != UseKind::Borrow || cursor.value.usage != UseKind::Mut { return None }
            let source = body.places.get(source.value.place.0 as usize)?;
            let destination = body.places.get(destination.0 as usize)?;
            let cursor = body.places.get(cursor.value.place.0 as usize)?;
            if cursor.root != destination.root || cursor.projection != destination.projection { return None }
            if !matches!(t.get(source.ty), Some(Type { kind: TypeKind::Named { definition: owner, .. }, .. }) if *owner.path() == receipt.association.source) { return None }
            Some(())
        })().is_some();
        if !valid {
            errors.push(Violation {
                body: Some(receipt.body),
                block: Some(receipt.open),
                instruction: None,
                kind: ViolationKind::InvalidProtocolAssociation,
            });
        }
    }
    errors
}

fn unproved(store: &TypeInterner, f: &FunctionType) -> bool {
    matches!(
        store.get(f.result).and_then(|t| t.erased_captures.as_ref()),
        Some(ErasedCaptures::FromArguments(selectors)) if selectors.iter().any(|selector| !selector.fields.is_empty())
            || !matches!(store.get(f.result).map(|row| &row.kind), Some(TypeKind::Builtin { kind: BuiltinType::Iter, .. } | TypeKind::Named { .. }))
    )
}
/// Selected state of every callable and whole-input Iter creation are proved by
/// independent flow. Field-selected and callable
/// creation results remain fail-closed. Callee headers remain
/// declaration-owned boundaries.
pub(super) fn unproved_promises(
    t: &TypeInterner,
    definition: &ReadDefinition<'_>,
    signatures: &Reader<'_>,
) -> Vec<Violation> {
    let owner = signatures
        .get(definition.path)
        .is_some_and(|s| match &s.kind {
            SignatureKind::Function(f) => unproved(t, f),
            _ => false,
        });
    let mut errors = vec![];
    if owner {
        errors.push(Violation {
            body: Some(definition.entry),
            block: None,
            instruction: None,
            kind: ViolationKind::UnprovedStatePromise,
        })
    }
    for (body_index, body) in definition.bodies.iter().enumerate() {
        for (block_index, block) in body.blocks.iter().enumerate() {
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                let InstructionKind::Assign { destination, value } = instruction.kind else {
                    continue;
                };
                if !matches!(
                    body.rvalues.get(value.0 as usize),
                    Some(Rvalue::Closure { .. })
                ) {
                    continue;
                }
                let Some(place) = body.places.get(destination.0 as usize) else {
                    continue;
                };
                let Some(row) = t.get(place.ty) else {
                    continue;
                };
                let TypeKind::Function(f) = &row.kind else {
                    continue;
                };
                if unproved(t, f)
                    || matches!(row.erased_captures, Some(ErasedCaptures::FromArguments(_)))
                {
                    errors.push(Violation {
                        body: Some(BodyId(body_index as u32)),
                        block: Some(BlockId(block_index as u32)),
                        instruction: Some(instruction_index as u32),
                        kind: ViolationKind::UnprovedStatePromise,
                    });
                }
            }
        }
    }
    errors
}

pub(super) fn selectors(
    t: &TypeInterner,
    memo: &TypeMemo,
    signature: &Signature,
    signatures: &Reader<'_>,
) -> bool {
    fn field(
        t: &TypeInterner,
        memo: &TypeMemo,
        mut ty: TypeId,
        fields: &SharedPath<FieldId>,
        signatures: &Reader<'_>,
    ) -> bool {
        for field in fields {
            let mut seen = HashSet::default();
            loop {
                let Some(row) = t.get(ty) else {
                    return false;
                };
                match &row.kind {
                    TypeKind::Named {
                        definition,
                        arguments,
                    } => {
                        if !seen.insert(*definition) {
                            return false;
                        }
                        let Some(s) = signatures.get(*definition) else {
                            return false;
                        };
                        let SignatureKind::Type(d) = &s.kind else {
                            return false;
                        };
                        let Some(repr) = d.representation else {
                            return false;
                        };
                        let arguments = arguments.to_vec();
                        let Ok(next) = hirsel_types::substitute(
                            t,
                            memo,
                            repr,
                            &Substitution {
                                binder: Some(GenericBinder::Bound(0)),
                                roles: &s.metadata.roles,
                                arguments: &arguments,
                                authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            },
                        ) else {
                            return false;
                        };
                        ty = next;
                    }
                    TypeKind::Record(parts) => {
                        let Some(part) = parts.get(field.0 as usize) else {
                            return false;
                        };
                        ty = part.ty;
                        break;
                    }
                    _ => return false,
                }
            }
        }
        true
    }
    let mut pending = Vec::new();
    match &signature.kind {
        SignatureKind::Type(d) => pending.extend(d.representation.map(|ty| (ty, vec![]))),
        SignatureKind::Const { ty: ty, .. } => pending.push((*ty, vec![])),
        SignatureKind::Implementation(i) => {
            pending.extend(i.types().into_iter().map(|ty| (ty, vec![])))
        }
        SignatureKind::Function(_) | SignatureKind::Family(_) => (),
    }
    if let SignatureKind::Function(f) = &signature.kind {
        pending.extend(f.parameters.iter().map(|p| (p.ty, vec![f])));
        pending.push((f.result, vec![f]));
        pending.extend(f.error.map(|ty| (ty, vec![f])));
        for p in &f.state_promises {
            let ParameterRef::Bound {
                depth: 0,
                parameter,
            } = p.selector.parameter
            else {
                return false;
            };
            let Some(parameter) = f.parameters.get(parameter.0 as usize) else {
                return false;
            };
            if !field(t, memo, parameter.ty, &p.selector.fields, signatures) {
                return false;
            }
        }
    }
    let mut seen = HashSet::default();
    while let Some((ty, mut scopes)) = pending.pop() {
        let key: Vec<_> = scopes
            .iter()
            .map(|f| f.parameters.iter().map(|p| p.ty).collect::<Vec<_>>())
            .collect();
        if !seen.insert((ty, key)) {
            continue;
        }
        let Some(row) = t.get(ty) else {
            return false;
        };
        if let Some(ErasedCaptures::FromArguments(selectors)) = &row.erased_captures {
            for selector in selectors {
                let ParameterRef::Bound { depth, parameter } = selector.parameter else {
                    return false;
                };
                let Some(parameter) = scopes
                    .get(depth as usize)
                    .and_then(|f| f.parameters.get(parameter.0 as usize))
                else {
                    return false;
                };
                if !field(t, memo, parameter.ty, &selector.fields, signatures) {
                    return false;
                }
            }
        }
        if let TypeKind::Function(f) = &row.kind {
            let mut selectors = BTreeSet::new();
            for p in &f.state_promises {
                if !selectors.insert(p.selector.clone())
                    || !(p.advance_only || p.preserve_captures || p.preserve_unique)
                    || p.preserve_unique && !p.selector.fields.is_empty()
                {
                    return false;
                }
                let ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } = p.selector.parameter
                else {
                    return false;
                };
                let Some(parameter) = f.parameters.get(parameter.0 as usize) else {
                    return false;
                };
                if parameter.passing != PassingMode::Mut
                    && !(parameter.passing == PassingMode::Take
                        && p.preserve_unique
                        && !p.preserve_captures
                        && !p.advance_only)
                {
                    return false;
                }
                if !field(t, memo, parameter.ty, &p.selector.fields, signatures) {
                    return false;
                }
            }
            scopes.insert(0, f);
        }
        pending.extend(
            row.kind
                .graph_edges()
                .into_iter()
                .map(|edge| (edge.ty, scopes.clone())),
        );
    }
    true
}

fn lift_actual(store: &TypeInterner, root: TypeId, removed: u32) -> Option<TypeId> {
    if removed == 0 {
        return Some(root);
    }
    let root = map_generic_binders(store, root, |b, nested| match b {
        GenericBinder::Bound(d) if d < nested => Ok(b),
        GenericBinder::Bound(d) if d >= nested + removed => Ok(GenericBinder::Bound(d - removed)),
        GenericBinder::Bound(_) => Err(GraphError::UnboundGeneric(b)),
        _ => Ok(b),
    })
    .ok()?;
    map_parameter_references(store, root, |p, nested| match p {
        ParameterRef::Bound { depth, parameter } if depth >= nested + removed => {
            Ok(ParameterRef::Bound {
                depth: depth - removed,
                parameter,
            })
        }
        ParameterRef::Bound { depth, .. } if depth >= nested => {
            Err(GraphError::UnboundParameter(p))
        }
        _ => Ok(p),
    })
    .ok()
}
