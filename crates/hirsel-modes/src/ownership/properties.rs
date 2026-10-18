use hirsel_syntax::DefPath;
use hirsel_types::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Signatures(BTreeMap<DefPath, SignatureRef>);
impl Signatures {
    pub fn read(
        &mut self,
        path: &DefPath,
        body: &mut CheckedBody,
        provider: &mut dyn SignatureProvider,
    ) -> Result<SignatureRef, QueryError> {
        self.read_with_reads(path, &mut body.resolution.reads, provider)
    }
    fn read_with_reads(
        &mut self,
        path: &DefPath,
        reads: &mut Vec<DependencyRead>,
        provider: &mut dyn SignatureProvider,
    ) -> Result<SignatureRef, QueryError> {
        if let Some(signature) = self.0.get(path) {
            return Ok(signature.clone());
        }
        let result = provider.signature(path);
        reads.push(DependencyRead::Signature {
            definition: path.clone(),
            fingerprint: result.as_ref().ok().map(|s| s.fingerprint),
        });
        let signature = result?;
        self.0.insert(path.clone(), signature.clone());
        Ok(signature)
    }
    /// A signature an earlier read retained; flow makes no new reads.
    pub fn cached(&self, path: &DefPath) -> Option<SignatureRef> {
        self.0.get(path).cloned()
    }
    pub fn dependencies(&self) -> Vec<SignatureDependency> {
        self.0
            .values()
            .map(|s| SignatureDependency {
                definition: s.definition.clone(),
                fingerprint: s.fingerprint,
            })
            .collect()
    }
}
/// SEM-3: naming a function in a nominal receiver's association promises
/// uniqueness of its builder, including for ordinary direct calls.
pub(super) fn format_association(
    types: &TypeStore,
    function: &FunctionType,
    path: &DefPath,
    signatures: &Signatures,
) -> Option<DefPath> {
    let receiver = function.parameters.first()?;
    let TypeKind::Named { definition, .. } = &types.get(receiver.ty)?.kind else {
        return None;
    };
    let signature = signatures.cached(definition)?;
    let SignatureKind::Type(nominal) = &signature.kind else {
        return None;
    };
    (nominal.format.as_ref() == Some(path)).then(|| definition.clone())
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Properties {
    pub plain: bool,
    pub flat: bool,
    pub sendable_aliased: bool,
    pub copyable: bool,
    pub valid: bool,
    pub function: bool,
    pub shared: bool,
    pub scoped_capability: bool,
}

/// Producer classification uses the shared declaration-based property proof.
pub(super) fn classify(
    body: &mut CheckedBody,
    own: &Signature,
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<Vec<Properties>, QueryError> {
    let declarations = body.resolution.generic_declarations.clone();
    let store = body.types.clone();
    let mut result = vec![];
    let mut evidence = store.clone();
    let prefix = evidence.mark();
    for (root, row) in store.iter() {
        let mut lookup = |path: &DefPath| {
            if path == &own.definition {
                return Ok(Some(std::sync::Arc::new(own.clone())));
            }
            signatures.read(path, body, provider).map(Some)
        };
        let generics = declarations.clone();
        let plain = structural_property(
            &store,
            root,
            StructuralProperty::Plain,
            &generics,
            &mut lookup,
        )?;
        let copyable = structural_property(
            &store,
            root,
            StructuralProperty::Copyable,
            &generics,
            &mut lookup,
        )?;
        let scoped_capability = if let TypeKind::Named { definition, .. } = &row.kind {
            lookup(definition)?.is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.capability == Some(CapabilityKind::Scoped)))
        } else {
            false
        };
        let flat = structural_flat(&store, root, &generics, &mut lookup)?;
        // Flow can invalidate the root's uniqueness after J has proved a
        // generic bound. Cache the same structural proof without that root
        // evidence; locality is checked separately at the call.
        let mut aliased = row.clone();
        aliased.modes.uniqueness = Uniqueness::Aliased;
        if !matches!(
            aliased.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) {
            aliased.modes.sendability = Sendability::NotSendable;
        }
        aliased.modes.relation = Relation::global();
        let aliased = evidence.push(aliased);
        let sendable_aliased =
            sendability_after_sharing(&evidence, aliased, &generics, &mut lookup)?
                == Sendability::Sendable;
        evidence.truncate_to(prefix);
        result.push(Properties {
            plain,
            flat,
            sendable_aliased,
            copyable,
            valid: semantic_equal(&store, root, &store, root).is_ok(),
            function: matches!(row.kind, TypeKind::Function(_)),
            shared: matches!(
                row.kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                }
            ),
            scoped_capability,
        });
    }
    Ok(result)
}

/// The implicit result-read permission is separate from written body bounds.
pub(super) fn result_sharing(
    body: &mut CheckedBody,
    own: &Signature,
    graphs: &[super::cfg::Graph],
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
    rows: &super::source_rows::SourceRows,
) -> Result<std::collections::BTreeSet<hirsel_syntax::ExprId>, QueryError> {
    let declarations = body.resolution.generic_declarations.clone();
    let store = body.types.clone();
    let mut reads = std::collections::BTreeSet::new();
    for graph in graphs {
        for expression in &graph.expressions {
            let Some(ty) = super::cfg::expression_type(body, *expression, rows) else {
                continue;
            };
            if structural_property(
                &store,
                ty,
                StructuralProperty::Copyable,
                &declarations.with_copyable(&graph.result_shareable),
                &mut |path| {
                    if path == &own.definition {
                        Ok(Some(std::sync::Arc::new(own.clone())))
                    } else {
                        signatures.read(path, body, provider).map(Some)
                    }
                },
            )? {
                reads.insert(*expression);
            }
        }
    }
    Ok(reads)
}

/// Flow refines locality and uniqueness. Recompute their derived summaries once
/// all graphs finish, keeping immutable type rows and every provider read.
pub(super) fn normalize(
    body: &mut CheckedBody,
    own: &Signature,
    results: &mut [(hirsel_types::NameSite, TypeId)],
    physical: &mut [(hirsel_syntax::ExprId, TypeId)],
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    let mut normalized = BTreeMap::new();
    let mut normalize = |root: &mut TypeId| -> Result<(), QueryError> {
        if let Some(replacement) = normalized.get(root) {
            *root = *replacement;
            return Ok(());
        }
        // I has replayed construction, checked receipts, sharing and writes.
        // These rows now carry current content facts, not structural guesses.
        // Canonicalize inert qualifiers without rederiving (and resurrecting)
        // evidence from a declaration or default permission.
        let replacement = canonical_current_type(
            &mut body.types,
            *root,
            &body.resolution.generic_declarations,
            &mut |path| {
                if path == &own.definition {
                    Ok(Some(std::sync::Arc::new(own.clone())))
                } else {
                    signatures
                        .read_with_reads(path, &mut body.resolution.reads, provider)
                        .map(Some)
                }
            },
        )?;
        normalized.insert(*root, replacement);
        *root = replacement;
        Ok(())
    };
    let mut arguments = |args: &mut Vec<GenericArgument>| -> Result<(), QueryError> {
        for argument in args {
            if let GenericArgument::Type(ty) = argument {
                normalize(ty)?;
            }
        }
        Ok(())
    };
    for instantiation in &mut body.instantiations {
        arguments(&mut instantiation.arguments)?;
    }
    for expression in &mut body.expressions {
        arguments(&mut expression.instantiated_arguments)?;
    }
    for expression in &mut body.expressions {
        normalize(&mut expression.ty)?;
    }
    for binding in &mut body.bindings {
        normalize(&mut binding.ty)?;
    }
    for pattern in &mut body.patterns {
        normalize(&mut pattern.ty)?;
    }
    for closure in &mut body.closures {
        normalize(&mut closure.ty)?;
        if let Some(result) = &mut closure.result_evidence {
            normalize(result)?;
        }
        for capture in &mut closure.captures {
            normalize(&mut capture.ty)?;
        }
    }
    for (_, ty) in physical {
        normalize(ty)?;
    }
    // A frame's formal result is the row its callable type publishes.
    for (_, result) in results {
        normalize(result)?;
    }
    Ok(())
}

/// J supplies field identities before ownership refines the receiving binding.
/// Bound each physical part by that final owner, retaining J's sibling brands
/// and nested callable contracts. K must not publish the earlier Global row
/// through a binding whose lifetime widened at a loop or branch join.
pub(super) fn identity_projections(
    body: &mut CheckedBody,
    own: &Signature,
    fields: &mut [(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)],
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    if fields.is_empty() {
        return Ok(());
    }
    let declarations = body.resolution.generic_declarations.clone();
    let bindings: BTreeMap<_, _> = body
        .bindings
        .iter()
        .filter_map(|row| {
            let root = match row.binding {
                ResolvedName::Local(local) => OriginRoot::Local(local),
                ResolvedName::Parameter(parameter) => {
                    OriginRoot::Parameter(ParameterRef::body(row.owner, parameter))
                }
                _ => return None,
            };
            Some(((row.owner, root), row.ty))
        })
        .collect();
    for (_, owner, origin, projections) in fields {
        let Some(&root) = bindings.get(&(*owner, origin.root.clone())) else {
            continue;
        };
        let mut parent = root;
        for (_, part) in projections {
            *part = projected_type(&mut body.types, parent, *part, &declarations, &mut |path| {
                if path == &own.definition {
                    Ok(Some(std::sync::Arc::new(own.clone())))
                } else {
                    signatures
                        .read_with_reads(path, &mut body.resolution.reads, provider)
                        .map(Some)
                }
            })?;
            parent = *part;
        }
    }
    Ok(())
}

/// The declaration-owned field lifetime precedes J's borrowed read refinement.
/// I applies this plan to the current temporary root instead of retaining a
/// projection made from an earlier, weaker call-result row.
#[derive(Clone, Copy)]
pub(super) enum ProjectionRoot {
    Value(hirsel_syntax::ExprId),
    Binding(usize),
    Constant(TypeId),
}
pub(super) struct ProjectionLifetime {
    pub root: ProjectionRoot,
    pub parts: Vec<(Relation, bool)>,
    pub physical: TypeId,
    /// Replacement traverses a counted owner, rather than inline storage alone.
    pub counted_path: bool,
    pub declared: Modes,
}
fn physical_layout(
    store: &mut TypeStore,
    ty: TypeId,
    own: &Signature,
    reads: &mut Vec<DependencyRead>,
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<TypeKind, QueryError> {
    let root = match expand_representation(
        store,
        ty,
        &mut |_, definition, arguments| {
            let signature = if definition == &own.definition {
                std::sync::Arc::new(own.clone())
            } else {
                signatures.read_with_reads(definition, reads, provider)?
            };
            Ok(Some((
                signature,
                vec![AuthoritySummary::Unknown; arguments.len()],
            )))
        },
        None,
    ) {
        Ok(root) => root,
        Err(RepresentationError {
            failure: RepresentationFailure::Query(error),
            ..
        }) => {
            return Err(error);
        }
        Err(error) => error.root,
    };
    Ok(store.get(root).unwrap().kind.clone())
}

/// Declared part rows before owner locality is applied. A projected pattern
/// already carries inherited locality and cannot identify an inferred Local part.
pub(super) fn pattern_declarations(
    body: &mut CheckedBody,
    own: &Signature,
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<BTreeMap<hirsel_syntax::PatternId, TypeId>, QueryError> {
    let rows: BTreeMap<_, _> = body
        .patterns
        .iter()
        .map(|p| (p.pattern, p.clone()))
        .collect();
    let children: std::collections::BTreeSet<_> =
        body.patterns.iter().flat_map(|p| p.subpatterns()).collect();
    let mut pending: Vec<_> = body
        .patterns
        .iter()
        .filter(|p| !children.contains(&p.pattern))
        .map(|p| (p.pattern, p.ty))
        .collect();
    let mut result = BTreeMap::new();
    while let Some((pattern, ty)) = pending.pop() {
        let row = &rows[&pattern];
        if let PatternDecisionKind::Or(alternatives) = &row.kind {
            // Alternatives declare the same place as their or-pattern.
            for alternative in alternatives {
                result.insert(*alternative, ty);
                pending.push((*alternative, ty));
            }
            continue;
        }
        if row.fields.is_empty() {
            continue;
        }
        let mut inspected = ty;
        while let TypeKind::Box(part) = body.types.get(inspected).unwrap().kind {
            inspected = part;
        }
        let kind = physical_layout(
            &mut body.types,
            inspected,
            own,
            &mut body.resolution.reads,
            signatures,
            provider,
        )?;
        for (field, child) in &row.fields {
            let part = match &kind {
                TypeKind::Record(fields) => fields.get(field.0 as usize).map(|f| f.ty),
                TypeKind::Tuple(fields) => fields.get(field.0 as usize).copied(),
                TypeKind::Variant(variants) => match &row.kind {
                    PatternDecisionKind::Constructor(ContextualName::Variant {
                        variant, ..
                    }) => variants
                        .get(variant.0 as usize)
                        .and_then(|v| v.fields.get(field.0 as usize))
                        .map(|f| f.1),
                    _ => None,
                },
                _ => None,
            };
            if let Some(part) = part {
                result.insert(*child, part);
                pending.push((*child, part));
            }
        }
    }
    Ok(result)
}

pub(super) fn projection_lifetimes(
    body: &mut CheckedBody,
    own: &Signature,
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
) -> Result<BTreeMap<hirsel_syntax::ExprId, ProjectionLifetime>, QueryError> {
    let arenas = crate::arenas::Arenas::new(body, own);
    let mut result = BTreeMap::new();
    let expression_types: BTreeMap<_, _> = body
        .expressions
        .iter()
        .map(|e| (e.expression, e.ty))
        .collect();
    let mut store = std::mem::take(&mut body.types);
    let outcome = (|| {
        for place in &body.places {
            if place.projections.is_empty() {
                continue;
            }
            let (root, mut ty) = match &place.root {
                SourceRoot::Value(root) => {
                    let Some(ty) = expression_types.get(root) else {
                        continue;
                    };
                    (ProjectionRoot::Value(*root), *ty)
                }
                SourceRoot::Binding { owner, binding } => {
                    let Some(index) = body
                        .bindings
                        .iter()
                        .position(|row| row.owner == *owner && row.binding == *binding)
                    else {
                        continue;
                    };
                    (ProjectionRoot::Binding(index), body.bindings[index].ty)
                }
                SourceRoot::Definition(path) => {
                    let signature =
                        signatures.read_with_reads(path, &mut body.resolution.reads, provider)?;
                    let SignatureKind::Const { ty: root, .. } = signature.kind else {
                        continue;
                    };
                    let Ok(root) = import_type(&signature.types, root, &mut store) else {
                        continue;
                    };
                    (ProjectionRoot::Constant(root), root)
                }
            };
            let mut parts = vec![];
            let mut counted_path = false;
            for step in &place.projections {
                let kind = physical_layout(
                    &mut store,
                    ty,
                    own,
                    &mut body.resolution.reads,
                    signatures,
                    provider,
                )?;
                counted_path |= matches!(
                    &kind,
                    TypeKind::Box(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Array
                                | BuiltinType::List
                                | BuiltinType::Map
                                | BuiltinType::SortedMap,
                            ..
                        }
                );
                let next = match (&step.kind, kind) {
                    (SourceProjectionKind::Field(FieldId(0)), TypeKind::Box(t)) => Some(t),
                    (SourceProjectionKind::Field(field), TypeKind::Record(fields)) => {
                        fields.get(field.0 as usize).map(|f| f.ty)
                    }
                    (SourceProjectionKind::Field(field), TypeKind::Tuple(fields)) => {
                        fields.get(field.0 as usize).copied()
                    }
                    (
                        SourceProjectionKind::VariantField { variant, field },
                        TypeKind::Variant(variants),
                    ) => variants
                        .get(variant.0 as usize)
                        .and_then(|v| v.fields.get(field.0 as usize))
                        .map(|f| f.1),
                    (SourceProjectionKind::Index(_), TypeKind::Builtin { kind, arguments })
                        if matches!(
                            kind,
                            BuiltinType::Array
                                | BuiltinType::List
                                | BuiltinType::Inline
                                | BuiltinType::Simd
                        ) =>
                    {
                        arguments.first().and_then(|a| match a {
                            GenericArgument::Type(t) => Some(*t),
                            _ => None,
                        })
                    }
                    // A byte is a plain u8, as V projects it; without this
                    // step `xs[i].payload[j]` published no physical rows and
                    // lowering typed the Bytes part at its receiving row.
                    (SourceProjectionKind::Index(_), TypeKind::Primitive(Primitive::Bytes)) => {
                        let mut byte = Type::boolean();
                        byte.kind = TypeKind::Primitive(Primitive::U8);
                        Some(store.push(byte))
                    }
                    _ => None,
                };
                let Some(next) = next else {
                    break;
                };
                let plain = structural_property(
                    &store,
                    next,
                    StructuralProperty::Plain,
                    &body.resolution.generic_declarations,
                    &mut |path| {
                        signatures
                            .read_with_reads(path, &mut body.resolution.reads, provider)
                            .map(Some)
                    },
                )?;
                parts.push((store.get(next).unwrap().modes.relation.clone(), plain));
                ty = next;
            }
            if parts.len() == place.projections.len() {
                // Physical storage retains the declaration's complete header and
                // nested contract. J has already specialized the occurrence's
                // owner-relative provenance and placement; keep that transport,
                // independently of its receiving modes and callable promises.
                let occurrence = store.get(expression_types[&place.expression]).unwrap();
                let mut physical = store.get(ty).unwrap().clone();
                physical.brand = occurrence.brand.clone();
                physical.modes.relation =
                    arenas.relocate(&occurrence.modes.relation, &physical.modes.relation);
                physical.allocation_target = occurrence.allocation_target.clone();
                let physical = store.push(physical);
                result.insert(
                    place.expression,
                    ProjectionLifetime {
                        root,
                        parts,
                        physical,
                        counted_path,
                        declared: store.get(ty).unwrap().modes.clone(),
                    },
                );
            }
        }
        Ok(result)
    })();
    body.types = store;
    outcome
}

/// A field place a call result reads within (#77): the path within the
/// argument, the declared row at its end and each step's declared relation
/// and plainness, which the flow folds into the argument's locality.
pub(super) struct FieldPlace {
    pub path: Vec<u32>,
    pub declared: TypeId,
    pub parts: Vec<(Relation, bool)>,
}
/// For each borrowed call argument whose callee's result relation names only
/// a field path of its parameter, that field place, read from the declared
/// layouts (#77).
pub(super) fn field_loan_places(
    body: &mut CheckedBody,
    own: &Signature,
    graphs: &[super::cfg::Graph],
    signatures: &mut Signatures,
    provider: &mut dyn SignatureProvider,
    rows: &super::source_rows::SourceRows,
) -> Result<BTreeMap<hirsel_syntax::ExprId, FieldPlace>, QueryError> {
    let mut requests = vec![];
    for node in graphs.iter().flat_map(|graph| &graph.nodes) {
        let super::cfg::Op::Call {
            arguments,
            requirements,
            result,
            constructor: false,
            ..
        } = &node.op
        else {
            continue;
        };
        for (argument, requirement) in arguments.iter().zip(requirements) {
            let Some(requirement) = requirement
                .as_ref()
                .filter(|r| r.passing == hirsel_syntax::PassingMode::Borrow)
            else {
                continue;
            };
            let parameter = ParameterRef::bound(ParamId(requirement.parameter as u32));
            let (Some(path), Some(ty)) = (
                result
                    .relation
                    .named_field_path(&parameter)
                    .map(|path| path.iter().map(|f| f.0).collect::<Vec<u32>>()),
                super::cfg::expression_type(body, argument.value, rows),
            ) else {
                continue;
            };
            requests.push((argument.value, path, ty));
        }
    }
    let mut result = BTreeMap::new();
    let mut store = std::mem::take(&mut body.types);
    let outcome = (|| {
        for (argument, path, mut ty) in requests {
            let mut found = true;
            let mut parts = vec![];
            for field in &path {
                let kind = physical_layout(
                    &mut store,
                    ty,
                    own,
                    &mut body.resolution.reads,
                    signatures,
                    provider,
                )?;
                ty = match kind {
                    TypeKind::Box(payload) if *field == 0 => payload,
                    TypeKind::Record(fields) if (*field as usize) < fields.len() => {
                        fields[*field as usize].ty
                    }
                    TypeKind::Tuple(fields) if (*field as usize) < fields.len() => {
                        fields[*field as usize]
                    }
                    _ => {
                        found = false;
                        break;
                    }
                };
                let plain = structural_property(
                    &store,
                    ty,
                    StructuralProperty::Plain,
                    &body.resolution.generic_declarations,
                    &mut |path| {
                        signatures
                            .read_with_reads(path, &mut body.resolution.reads, provider)
                            .map(Some)
                    },
                )?;
                parts.push((store.get(ty).unwrap().modes.relation.clone(), plain));
            }
            if found {
                result.insert(
                    argument,
                    FieldPlace {
                        path,
                        declared: ty,
                        parts,
                    },
                );
            }
        }
        Ok(result)
    })();
    body.types = store;
    outcome
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use crate::authority::test_support::Fixture;

    #[test]
    fn physical_layout_resolves_finite_nominal_chains_past_128() {
        for depth in [128, 129, 200] {
            let mut f = Fixture::new();
            let mut root = f.ty(TypeKind::Primitive(Primitive::U8));
            for n in 0..depth {
                root = f.nominal(
                    &format!("D{n}"),
                    None,
                    vec![],
                    AuthoritySummary::inert(),
                    Some(root),
                    false,
                );
            }
            let mut reads = vec![];
            let mut signatures = Signatures::default();
            assert_eq!(
                physical_layout(
                    &mut f.body.types,
                    root,
                    &f.signature,
                    &mut reads,
                    &mut signatures,
                    &mut f.provider
                )
                .unwrap(),
                TypeKind::Primitive(Primitive::U8)
            );
        }
    }
}
