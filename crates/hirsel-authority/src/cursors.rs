//! Implicit call results use the same declared graph before and after ownership.
use super::*;

fn graph_error(error: GraphError) -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "cursor provenance".into(),
        message: format!("result graph: {error:?}"),
    })
}

pub(super) fn result(
    t: &TypeInterner,
    m: &TypeMemo,
    roles: &NominalRoles,
    generics: &GenericDeclarations,
    path: &DefPath,
    arguments: &[GenericArgument],
    authorities: &[AuthoritySummary],
    origins: &BTreeMap<ParameterRef, OriginArgument>,
    actuals: &BTreeMap<ParameterRef, TypeId>,
    read: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let signature = read(DefId::intern(path))?.ok_or(QueryError::NotImplemented)?;
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(QueryError::Operational(OperationalFailure {
            operation: "cursor provenance".into(),
            message: "cursor operation is not a function".into(),
        }));
    };
    // Resolve the library parameter namespace before inserting caller generics.
    let declared =
        instantiate_parameter_sources(t, function.result, origins, actuals).map_err(graph_error)?;
    let result = substitute(
        t,
        m,
        declared,
        &Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles,
            arguments,
            authorities,
        },
    )
    .map_err(graph_error)?;
    // Generic actuals are fixed receiving contracts, not fresh values whose
    // nested property summaries may be strengthened. In particular, a unique
    // nominal actual can still carry conservative NotSendable evidence. Keep
    // that exact contract in both the call actuals and its result graph.
    let result = canonical_current_type(t, result, generics, read)?;
    let mut row = t.row(result).clone();
    if row.modes.sendability == Sendability::NotSendable {
        row.modes.sendability = structural_sendability(t, result, generics, read)?;
    }
    Ok(t.intern(row))
}

fn argument_authorities(
    t: &TypeInterner,
    arguments: &[GenericArgument],
    roles: &NominalRoles,
    generics: &GenericDeclarations,
    read: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Vec<AuthoritySummary>, QueryError> {
    arguments
        .iter()
        .map(|argument| match argument {
            GenericArgument::Type(ty) => {
                structural_authority(t, *ty, roles, generics, &mut |_, path| match path {
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
    fn types(&self) -> &TypeInterner;
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
    let actual = reader.types().row(source).clone();
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
    let mut mapping = std::collections::HashMap::new();
    let mut pending = vec![(previous_cursor, plan.cursor)];
    while let Some((old, new)) = pending.pop() {
        if mapping.insert(old, new).is_some() {
            continue;
        }
        let old = reader.types().row(old).kind.graph_edges();
        let new = reader.types().row(new).kind.graph_edges();
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
            relation: reader.types().row(plan.cursor).modes.relation.clone(),
        },
    )]);
    plan.step.result = reader.call_result(
        &plan.next,
        &plan.next_arguments,
        &next_origins,
        &BTreeMap::from([(ParameterRef::bound(ParamId(0)), plan.cursor)]),
    )?;
    let TypeKind::Named { arguments, .. } = reader.types().row(plan.step.result).kind.clone()
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
    t: &'a TypeInterner,
    m: &'a TypeMemo,
    body: &'a mut CheckedBody,
    signature: &'a Signature,
    read: R,
}
impl<R> CursorRefinement for CurrentContent<'_, R>
where
    R: FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
{
    fn types(&self) -> &TypeInterner {
        self.t
    }
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
            self.t,
            arguments,
            &self.signature.metadata.roles,
            generics,
            &mut self.read,
        )?;
        result(
            self.t,
            self.m,
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
        let item = placed_part_type(self.t, owner, item, generics, &mut self.read)?;
        let (t, read) = (self.t, &mut self.read);
        if t.contains(item)
            && t.contains(owner)
            && t.row(item)
                .modes
                .relation
                .union(&t.row(owner).modes.relation)
                != t.row(item).modes.relation
        {
            let plain = structural_property(t, item, StructuralProperty::Plain, generics, read)?;
            return Ok(project(t, self.m, owner, item, plain));
        }
        Ok(item)
    }
}

/// J keeps source identity; I supplies the source's current complete value row.
/// Reinstantiate both calls so nested owner-dependent item rows follow too.
pub fn refine_cursor_plan(
    t: &TypeInterner,
    m: &TypeMemo,
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
    let read = |definition: DefId| {
        if let Some(signature) = dependencies.get(&definition) {
            return Ok(Some(std::sync::Arc::clone(signature)));
        }
        let value = provider.signature(definition);
        reads.push(
            DependencyRead::Signature {
                definition,
                fingerprint: value.as_ref().ok().map(|s| s.fingerprint),
            }
            .shared(),
        );
        let signature = value?;
        dependencies.insert(definition, signature.clone());
        Ok(Some(signature))
    };
    let outcome = refine(
        &mut CurrentContent {
            t,
            m,
            body,
            signature,
            read,
        },
        plan,
        source,
        identity,
    );
    std::sync::Arc::make_mut(&mut body.resolution.reads).extend(reads);
    outcome?;
    Ok(dependencies
        .values()
        .map(|s| SignatureDependency {
            definition: s.definition,
            fingerprint: s.fingerprint,
        })
        .collect())
}
