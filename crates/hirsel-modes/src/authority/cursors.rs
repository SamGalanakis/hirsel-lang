//! Implicit call results use the same declared graph before and after ownership.
use super::*;

fn graph_error(error: GraphError) -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "cursor provenance".into(),
        message: format!("result graph: {error:?}"),
    })
}

pub(super) fn result(
    store: &mut TypeStore,
    roles: &NominalRoles,
    generics: &GenericDeclarations,
    path: &DefPath,
    arguments: &[GenericArgument],
    authorities: &[AuthoritySummary],
    origins: &BTreeMap<ParameterRef, OriginArgument>,
    actuals: &BTreeMap<ParameterRef, TypeId>,
    read: &mut impl FnMut(&DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let signature = read(path)?.ok_or(QueryError::NotImplemented)?;
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(QueryError::Operational(OperationalFailure {
            operation: "cursor provenance".into(),
            message: "cursor operation is not a function".into(),
        }));
    };
    let declared = import_type(&signature.types, function.result, store).map_err(graph_error)?;
    // Resolve the library parameter namespace before inserting caller generics.
    let declared =
        instantiate_parameter_sources(store, declared, origins, actuals).map_err(graph_error)?;
    let result = substitute_type_in_place(
        store,
        declared,
        Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles,
            arguments,
            authorities,
        },
    )
    .map_err(graph_error)?;
    let result = canonical_plain_type(store, result, read)?;
    normalize_type_properties(store, result, generics, read)
}

fn argument_authorities(
    store: &TypeStore,
    arguments: &[GenericArgument],
    roles: &NominalRoles,
    generics: &GenericDeclarations,
    read: &mut impl FnMut(&DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Vec<AuthoritySummary>, QueryError> {
    arguments
        .iter()
        .map(|argument| match argument {
            GenericArgument::Type(ty) => {
                structural_authority(store, *ty, roles, generics, &mut |_, path| match path {
                    Some(path) => read(path),
                    None => Ok(None),
                })
            }
            _ => Ok(AuthoritySummary::inert()),
        })
        .collect()
}

/// Phase-owned judgments used by the common implicit-call sequence.
pub(super) trait CursorRefinement {
    fn body(&self) -> &CheckedBody;
    fn body_mut(&mut self) -> &mut CheckedBody;
    fn call_result(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
    ) -> Result<TypeId, QueryError>;
    fn item(&mut self, owner: TypeId, item: TypeId) -> Result<TypeId, QueryError>;
}

pub(super) fn refine(
    reader: &mut impl CursorRefinement,
    plan: &mut CursorPlan,
    source: TypeId,
    identity: &Origin,
) -> Result<(), QueryError> {
    let actual = reader.body().types.get(source).unwrap();
    let origins = BTreeMap::from([(
        ParameterRef::bound(ParamId(0)),
        OriginArgument {
            identity: identity.clone(),
            brand: actual.brand.clone(),
            relation: actual.modes.relation.clone(),
        },
    )]);
    let previous_cursor = plan.cursor;
    plan.cursor = reader.call_result(
        &plan.open,
        &plan.open_arguments,
        &origins,
        &BTreeMap::from([(ParameterRef::bound(ParamId(0)), source)]),
    )?;
    // Transport H's exact specialization along the refined cursor graph. The
    // next operation's generic order need not be the cursor's argument order.
    let mut mapping = BTreeMap::new();
    let mut pending = vec![(previous_cursor, plan.cursor)];
    while let Some((old, new)) = pending.pop() {
        if mapping.insert(old, new).is_some() {
            continue;
        }
        let old = reader.body().types.get(old).unwrap().kind.graph_edges();
        let new = reader.body().types.get(new).unwrap().kind.graph_edges();
        if old.len() != new.len() {
            return Err(cursor_error("cursor refinement changed shape"));
        }
        pending.extend(old.iter().zip(&new).map(|(a, b)| (a.ty, b.ty)));
    }
    for argument in &mut plan.next_arguments {
        if let GenericArgument::Type(ty) = argument {
            *ty = mapping.get(ty).copied().unwrap_or(*ty);
        }
    }
    let next_origins = BTreeMap::from([(
        ParameterRef::bound(ParamId(0)),
        OriginArgument {
            identity: identity.clone(),
            brand: Brand::default(),
            relation: reader
                .body()
                .types
                .get(plan.cursor)
                .unwrap()
                .modes
                .relation
                .clone(),
        },
    )]);
    plan.step.result = reader.call_result(
        &plan.next,
        &plan.next_arguments,
        &next_origins,
        &BTreeMap::from([(ParameterRef::bound(ParamId(0)), plan.cursor)]),
    )?;
    let TypeKind::Named { arguments, .. } =
        &reader.body().types.get(plan.step.result).unwrap().kind
    else {
        return Err(cursor_error("next result has no item"));
    };
    let [GenericArgument::Type(item)] = arguments.as_slice() else {
        return Err(cursor_error("next result has invalid arguments"));
    };
    plan.step.item = reader.item(plan.step.result, *item)?;
    for instantiation in &mut reader.body_mut().instantiations {
        if instantiation.expression == plan.source && instantiation.callee == plan.next {
            instantiation.arguments = plan.next_arguments.clone();
        }
    }
    Ok(())
}
fn cursor_error(message: &str) -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "cursor provenance".into(),
        message: message.into(),
    })
}

struct CurrentContent<'a, R> {
    body: &'a mut CheckedBody,
    signature: &'a Signature,
    read: R,
}
impl<R> CursorRefinement for CurrentContent<'_, R>
where
    R: FnMut(&DefPath) -> Result<Option<SignatureRef>, QueryError>,
{
    fn body(&self) -> &CheckedBody {
        self.body
    }
    fn body_mut(&mut self) -> &mut CheckedBody {
        self.body
    }
    fn call_result(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
    ) -> Result<TypeId, QueryError> {
        let generics = &self.body.resolution.generic_declarations;
        let authorities = argument_authorities(
            &self.body.types,
            arguments,
            &self.signature.metadata.roles,
            generics,
            &mut self.read,
        )?;
        result(
            &mut self.body.types,
            &self.signature.metadata.roles,
            generics,
            path,
            arguments,
            &authorities,
            origins,
            actuals,
            &mut self.read,
        )
    }
    fn item(&mut self, owner: TypeId, item: TypeId) -> Result<TypeId, QueryError> {
        let generics = &self.body.resolution.generic_declarations;
        let item = placed_part_type(&mut self.body.types, owner, item, generics, &mut self.read)?;
        projected_type(&mut self.body.types, owner, item, generics, &mut self.read)
    }
}

/// J keeps source identity; I supplies the source's current complete value row.
/// Reinstantiate both calls so nested owner-dependent item rows follow too.
pub fn refine_cursor_plan(
    body: &mut CheckedBody,
    signature: &Signature,
    plan: &mut CursorPlan,
    identity: &Origin,
    provider: &mut dyn SignatureProvider,
) -> Result<Vec<SignatureDependency>, QueryError> {
    let source = body
        .expressions
        .iter()
        .find(|e| e.expression == plan.source)
        .ok_or_else(|| cursor_error("missing cursor source"))?
        .ty;
    let mut dependencies = BTreeMap::new();
    let mut reads = vec![];
    let read = |path: &DefPath| {
        if let Some(signature) = dependencies.get(path) {
            return Ok(Some(std::sync::Arc::clone(signature)));
        }
        let value = provider.signature(path);
        reads.push(DependencyRead::Signature {
            definition: path.clone(),
            fingerprint: value.as_ref().ok().map(|s| s.fingerprint),
        });
        let signature = value?;
        dependencies.insert(path.clone(), signature.clone());
        Ok(Some(signature))
    };
    let outcome = refine(
        &mut CurrentContent {
            body,
            signature,
            read,
        },
        plan,
        source,
        identity,
    );
    body.resolution.reads.extend(reads);
    outcome?;
    Ok(dependencies
        .values()
        .map(|s| SignatureDependency {
            definition: s.definition.clone(),
            fingerprint: s.fingerprint,
        })
        .collect())
}
