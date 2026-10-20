//! Private executable identities for signature-owned fact code (ordinal zero).
use hirsel_syntax::{ContentHash, DefId, DefPath, PassingMode, Visibility};
use hirsel_types::*;
pub fn fact_path(owner: &DefPath) -> DefPath {
    let mut path = owner.clone();
    path.0.name.push_str("$fact$0");
    path
}
pub fn fact_owner(path: &DefPath) -> Option<DefPath> {
    let name = path.0.name.strip_suffix("$fact$0")?;
    let mut owner = path.clone();
    owner.0.name = name.into();
    Some(owner)
}
/// The dependency list a fact helper's fingerprint covers: its owner, whose
/// fingerprint covers the fact's code hash.
pub fn fact_dependencies(owner: &Signature) -> Vec<SignatureDependency> {
    vec![SignatureDependency {
        definition: owner.definition.path().clone(),
        fingerprint: owner.fingerprint,
    }]
}
/// The callable convention is determined entirely by the declaring signature,
/// and the fingerprint is every signature's: its encoding over
/// [`fact_dependencies`].
pub fn fact_signature(t: &TypeInterner, owner: &Signature) -> Result<Signature, QueryError> {
    let SignatureKind::Type(td) = &owner.kind else {
        return Err(QueryError::NotImplemented);
    };
    if td.fact.is_none() {
        return Err(QueryError::NotImplemented);
    }
    let Some(Type {
        kind: TypeKind::Record(fields),
        ..
    }) = td.representation.and_then(|ty| t.get(ty))
    else {
        return Err(QueryError::NotImplemented);
    };
    let [field] = fields.as_slice() else {
        return Err(QueryError::NotImplemented);
    };
    let mut modes = Modes::global();
    modes.sendability = Sendability::Sendable;
    let boolean = t.intern(Type {
        modes,
        ..Type::with_kind(TypeKind::Primitive(Primitive::Bool))
    });
    let mut helper = Signature {
        definition: DefId::intern(&fact_path(owner.definition.path())),
        fingerprint: ContentHash::default(),
        metadata: SignatureMetadata {
            track_caller: false,
            visibility: Visibility::Private,
            ..owner.metadata.clone()
        },
        kind: SignatureKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: td.generics.clone().into(),
            parameters: vec![Parameter {
                name: Some(field.name.as_str().to_owned()),
                label: None,
                passing: PassingMode::Borrow,
                ty: field.ty,
            }]
            .into(),
            result: boolean,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: td.authority.clone(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
    };
    helper.fingerprint =
        encoding::fingerprint(t, &helper, &fact_dependencies(owner)).map_err(|e| {
            QueryError::Operational(OperationalFailure {
                operation: "fact helper signature".into(),
                message: format!("{e:?}"),
            })
        })?;
    Ok(helper)
}
