//! Written headers → self-contained semantic signatures (LANG-6).
//! No body is visited. Missing syntax is rejected; omissions have fixed defaults.

#[cfg(test)]
hirsel_guard::install!();

mod contracts;
mod destruction;
mod import;
pub mod protocol;
use hirsel_check_protocol::declaration::{
    PART_LOCALITY, builtin_bound, environment_alone, environment_candidates, written_local,
};
use hirsel_check_protocol::header::{Input, NormalizedDeclaration, OwnerRepresentation};
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_span::Span;
use hirsel_syntax as ast;
use hirsel_syntax::{DefId, DefMap};
use hirsel_types::encoding::fingerprint;
use hirsel_types::*;
use std::collections::{BTreeMap, BTreeSet};
type IdSet<T> = std::collections::HashSet<T, hirsel_intern::IdHasher>;

/// Normalizes one header/type declaration. TypeIds belong to the returned store.
/// Dependencies include contracts and named types; query errors propagate unchanged.
pub fn normalize(
    input: Input<'_>,
    provider: &mut dyn SignatureProvider,
) -> Result<QueryOutput<Signature>, QueryError> {
    normalize_owned(input, provider).map(|output| output.public)
}
pub fn normalize_owned(
    input: Input<'_>,
    provider: &mut dyn SignatureProvider,
) -> Result<NormalizedDeclaration, QueryError> {
    let failure = std::cell::RefCell::new(None);
    let result = normalize_inner(input, provider, &failure);
    failure.into_inner().map_or(result, Err)
}
fn normalize_inner(
    input: Input<'_>,
    provider: &mut dyn SignatureProvider,
    failure: &std::cell::RefCell<Option<QueryError>>,
) -> Result<NormalizedDeclaration, QueryError> {
    let item = &input.syntax.item;
    let name_span = item.name_span;
    let duplicate = if input.declaration_sites.is_empty() {
        input.syntax.occurrence > 0
    } else {
        let index = input
            .declaration_sites
            .iter()
            .position(|site| site.definition == item.definition && site.name_span == name_span)
            .ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    name_span,
                    "item name is missing from module declaration input",
                )
            })?;
        input.declaration_sites[..index]
            .iter()
            .any(|previous| previous.definition == item.definition)
    };
    if duplicate {
        return Err(diagnostic(
            const { RuleId::of("LANG-7") },
            name_span,
            "duplicate definition; overloading is forbidden",
        ));
    }
    let mut n = Normalizer {
        input: &input,
        provider,
        types: input.types,
        memo: input.memo,
        own_representation: None,
        generics: vec![],
        outer_generics: vec![],
        parameters: vec![],
        parameter_syntax: vec![],
        outer_parameters: vec![],
        siblings: vec![],
        dependencies: DefMap::default(),
        snapshots: DefMap::default(),
        group: BTreeMap::new(),
        group_key: None,
        group_drop: BTreeMap::new(),
        builtin_declarations: Default::default(),
        active: BTreeSet::new(),
        aliases: BTreeSet::new(),
        arena_places: BTreeSet::new(),
        current_parameter: None,
        public: item.visibility == ast::Visibility::Public,
        span: item.origin.span,
        proof_failure: failure,
    };
    if matches!(item.kind, ast::ItemKind::Type { .. })
        && item
            .attributes
            .iter()
            .any(|a| a.name.as_str() == "capability")
        && item
            .attributes
            .iter()
            .any(|a| a.name.as_str() == "serializable")
    {
        return Err(diagnostic(
            const { RuleId::of("SER-1") },
            item.origin.span,
            "a capability cannot opt into a serializable schema",
        ));
    }
    let attrs = n.attributes(&item.attributes, &item.kind)?;
    let unsafe_ffi = input
        .metadata
        .unsafe_ffi
        .contains_key(&item.definition.0.package);
    let mut implementation_authority = None;
    // The owner view keeps a carrier's field selection; a public opaque
    // export says only that the type carries erased evidence.
    let mut owner_captures = None;
    let mut kind = match &item.kind {
        ast::ItemKind::Function { signature, abi, .. } => {
            if item.visibility == ast::Visibility::PublicOpaque {
                return Err(diagnostic(
                    const { RuleId::of("PROG-2") },
                    item.origin.span,
                    "pub opaque applies only to types",
                ));
            }
            let mut f = n.function(signature, &attrs, false, abi.clone())?;
            let selected: Vec<_> = item
                .attributes
                .iter()
                .map(|a| a.name.as_str())
                .filter(|name| matches!(*name, "test" | "bench"))
                .collect();
            if selected.len() > 1 {
                return Err(diagnostic(
                    const { RuleId::of("LANG-9") },
                    item.origin.span,
                    "a function is either a @test or a @bench, not both",
                ));
            }
            if let [attribute] = selected.as_slice() {
                let unit = |id| matches!(n.types.get(id).map(|t| &t.kind), Some(TypeKind::Tuple(fields)) if fields.is_empty());
                let result = unit(f.result)
                    || matches!(n.types.get(f.result).map(|t| &t.kind), Some(TypeKind::Named { definition, arguments }) if definition.is_core("Result") && matches!(arguments.as_slice(), [GenericArgument::Type(ok), GenericArgument::Type(_)] if unit(*ok)));
                let parameters = f.parameters.is_empty()
                    || matches!(f.parameters.as_slice(), [p] if p.passing == ast::PassingMode::Borrow && matches!(n.types.get(p.ty).map(|t| &t.kind), Some(TypeKind::Named { definition, arguments }) if definition.is_core("Test") && arguments.is_empty()));
                if !f.generics.is_empty()
                    || !parameters
                    || !result
                    || f.incomplete
                    || abi.is_some()
                    || !matches!(item.kind, ast::ItemKind::Function { body: Some(_), .. })
                {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-9") },
                        item.origin.span,
                        &format!(
                            "@{attribute} requires a complete monomorphic Hirsel function with no parameters or one borrowed Test, returning () or Result[(), E]"
                        ),
                    ));
                }
            }

            if let Some(abi) = abi {
                let foreign = |message: &str| {
                    Err(diagnostic(
                        const { RuleId::of("PROG-4") },
                        item.origin.span,
                        message,
                    ))
                };
                if !f.generics.is_empty() {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-4") },
                        item.origin.span,
                        "foreign functions must be monomorphic",
                    ));
                }
                if !unsafe_ffi {
                    return foreign("extern requires an unsafe ffi package");
                }
                if abi.name() != "c" {
                    return foreign("only the c ABI is supported");
                }
                if !f.parameters.iter().any(|p| n.is_unsafe(p.ty)) {
                    return foreign("extern requires a declared Unsafe capability parameter");
                }
                for p in &f.parameters {
                    if n.is_unsafe(p.ty) || n.ffi_context(p) {
                        continue;
                    }
                    match n.ffi_class(p.ty) {
                        None => return foreign("type is not permitted by the foreign interface"),
                        Some(class) if !class.permits(p.passing) => {
                            return foreign(
                                "this parameter mode cannot pass the type to C: take transfers \
                                 only numbers and pointers, and a byte view is only borrowed",
                            );
                        }
                        Some(_) => {}
                    }
                }
                if !n.ffi_class(f.result).is_some_and(FfiClass::result) {
                    return foreign("foreign result type is not permitted");
                }
                let Some(contract) = &abi.contract else {
                    return foreign("an extern states its aliasing contract in a doc comment");
                };
                // MODE-2: a promise is proved on a body, and C has none.
                if f.state_promises.iter().any(|p| p.preserve_unique) {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-7") },
                        item.origin.span,
                        "an extern cannot declare @preserves_unique: C has no body to prove it",
                    ));
                }
                f.foreign = Some(ForeignContract {
                    abi: abi.name().to_owned(),
                    blocking: attrs.blocking,
                    aliasing_contract: contract.clone(),
                });
            }
            SignatureKind::Function(f)
        }
        ast::ItemKind::Signature { members } => n.family(members)?,
        ast::ItemKind::Implementation { members, .. } => n.implementation(members)?,
        ast::ItemKind::Const { ty, .. } => {
            if item.visibility == ast::Visibility::PublicOpaque {
                return Err(diagnostic(
                    const { RuleId::of("PROG-2") },
                    item.origin.span,
                    "pub opaque applies only to types",
                ));
            }
            SignatureKind::Const {
                ty: n.ty(*ty, Position::Result)?,
                generics: Default::default(),
            }
        }
        ast::ItemKind::Type {
            flavor,
            generics,
            bounds,
            authority,
            body,
            drop,
        } => {
            n.generics = n.generic_parameters(generics)?;
            if drop.is_some() && *flavor != ast::TypeFlavor::Resource {
                return Err(diagnostic(
                    const { RuleId::of("LANG-7") },
                    item.origin.span,
                    "only resources declare drop",
                ));
            }
            if matches!(body, ast::TypeBody::Opaque)
                && item.visibility != ast::Visibility::PublicOpaque
            {
                return Err(diagnostic(
                    const { RuleId::of("PROG-2") },
                    item.origin.span,
                    "only opaque types omit representation",
                ));
            }
            if attrs.capability.is_some() && item.visibility != ast::Visibility::PublicOpaque {
                return Err(diagnostic(
                    const { RuleId::of("PROG-2") },
                    item.origin.span,
                    "capabilities require opaque type declarations",
                ));
            }
            if item.visibility == ast::Visibility::PublicOpaque
                && (authority.is_none() || bounds.is_none())
            {
                return Err(diagnostic(
                    if attrs.capability.is_some()
                        && authority.is_none()
                        && !matches!(body, ast::TypeBody::Opaque)
                    {
                        const { RuleId::of("CAP-5") }
                    } else {
                        const { RuleId::of("LANG-6") }
                    },
                    item.name_span,
                    "opaque bounds and authority must be stated",
                ));
            }
            let check = n.serializable_check(&item.attributes)?;
            // A checked opt-in is an opaque record or variant whose decoded
            // representations its check admits; an unchecked one is transparent data.
            if check.is_some()
                && (item.visibility != ast::Visibility::PublicOpaque
                    || !generics.is_empty()
                    || !matches!(body, ast::TypeBody::Record(_) | ast::TypeBody::Variants(_)))
            {
                return Err(diagnostic(
                    const { RuleId::of("SER-1") },
                    item.origin.span,
                    "a serializable check applies to a nongeneric opaque record or variant",
                ));
            }
            if (attrs.serializable || bounds.iter().flatten().any(|b| *b == Bound::Serializable))
                && (item.visibility == ast::Visibility::PublicOpaque && check.is_none()
                    || *flavor == ast::TypeFlavor::Resource
                    || attrs.capability.is_some()
                    || matches!(body, ast::TypeBody::Distinct { fact: Some(_), .. }))
            {
                return Err(diagnostic(
                    const { RuleId::of("SER-1") },
                    item.origin.span,
                    "serializable requires transparent data without authority, brands or refinements; convert through a data DTO",
                ));
            }
            if input.stage == Some(GroupStage::Header) {
                return Ok(provisional(&n, item, &attrs, *flavor, None));
            }
            let group = n.provider.recursive_group(
                &item.definition,
                if input.stage.is_some() {
                    GroupStage::Header
                } else {
                    GroupStage::Representation
                },
            )?;
            n.group_key = group.key;
            for member in group.members {
                n.snapshots.insert(member.definition, member.clone());
                n.group.insert(member.definition, member);
            }
            n.load_group_nominals()?;
            let public_deps = n.dependencies.clone();
            let repr = n.type_body(body)?;
            n.own_representation = repr;
            if input.stage == Some(GroupStage::Representation) {
                return Ok(provisional(&n, item, &attrs, *flavor, repr));
            }
            n.settle_group_authority();
            n.settle_group_drop();
            let opaque = item.visibility == ast::Visibility::PublicOpaque;
            // A checked opaque publishes its representation as its wire contract.
            if opaque && check.is_none() {
                n.dependencies = public_deps.clone();
            }
            if let ast::TypeBody::Distinct { field, .. } = body {
                n.parameters = vec![Some(field.name.as_str().to_owned())];
            }
            let fact = if let ast::TypeBody::Distinct { fact: Some(e), .. } = body {
                if !repr.is_some_and(|r| n.satisfies(r, Bound::Plain)) {
                    return Err(diagnostic(
                        const { RuleId::of("FACT-2") },
                        item.origin.span,
                        "a distinct fact requires a plain base",
                    ));
                }
                Some(n.contract(ast::ContractKind::Requires, *e, 0)?)
            } else {
                None
            };
            if *flavor == ast::TypeFlavor::Flat
                && repr.is_some_and(|r| !n.flat_storage(r, &mut IdSet::default()))
            {
                return Err(diagnostic(
                    const { RuleId::of("MEM-12") },
                    item.origin.span,
                    "MEM-12: flat fields require pointer-free storage without capabilities, handles or resources",
                ));
            }
            let destructor_allocation = if drop.as_ref().is_some_and(|d| d.noalloc) {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            };
            let fields_drop = repr.map_or(DropAllocation::MayAllocate, |r| n.drop_summary(r));
            let noalloc_drop = fields_drop == DropAllocation::NoAlloc
                && (*flavor != ast::TypeFlavor::Resource
                    || destructor_allocation == Allocation::NoAlloc);
            let mut inferred = n.structural_bounds(repr, *flavor, attrs.capability);
            if noalloc_drop {
                inferred.push(Bound::NoallocDrop);
            }
            let declared_authority = match authority {
                Some(paths) => n.authority(paths)?,
                None => AuthoritySummary::inert(),
            };
            // CAP-5: the implementation ceiling is an owner input, including
            // its nominal dependencies. It never enters the public fingerprint.
            if attrs.capability.is_some() {
                n.dependencies = public_deps;
                if repr.is_some() {
                    implementation_authority = Some(declared_authority.clone());
                }
            }
            let carrier = n.erased_carrier(item, body, &attrs)?;
            owner_captures = carrier.as_ref().map(|fields| {
                fields
                    .iter()
                    .map(|f| *f as u32)
                    .collect::<SharedSlice<u32>>()
            });
            // CAP-2/CLO-4: a carrier's selected fields hold erased state whose
            // authority each value carries; the declaration covers the rest.
            let field_authority = match (&carrier, repr.and_then(|r| n.types.get(r))) {
                (
                    Some(selected),
                    Some(Type {
                        kind: TypeKind::Record(parts),
                        ..
                    }),
                ) => parts
                    .clone()
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !selected.contains(i))
                    .fold(AuthoritySummary::inert(), |a, (_, part)| {
                        a.join(n.type_authority(Some(part.ty)))
                    }),
                (Some(_), _) => AuthoritySummary::inert(),
                (None, _) => n.type_authority(repr),
            };
            // CAP-2: "An explicit summary cannot erase field authority."
            // Opaque clients use only the export; the owner must prove it.
            if opaque && repr.is_some() && !field_authority.is_subset_of(&declared_authority) {
                let mut error = diagnostic(
                    if attrs.capability.is_some() {
                        const { RuleId::of("CAP-5") }
                    } else {
                        const { RuleId::of("CAP-2") }
                    },
                    item.origin.span,
                    "opaque authority summary omits representation authority",
                );
                if attrs.capability.is_some() {
                    if let QueryError::Diagnostics(ds) = &mut error {
                        ds[0].notes.push(format!("owner representation retains {field_authority:?}; private ceiling is {declared_authority:?}"));
                        if let ast::TypeBody::Record(fields) = body {
                            if let Some(Type {
                                kind: TypeKind::Record(parts),
                                ..
                            }) = repr.and_then(|r| n.types.get(r))
                            {
                                for (field, part) in fields.iter().zip(parts) {
                                    let held = n.type_authority(Some(part.ty));
                                    if !held.is_subset_of(&declared_authority) {
                                        ds[0].secondary.push(hirsel_diag::Label {
                                            span: field.origin.span,
                                            message: format!(
                                                "field `{}` and its destruction retain {held:?}",
                                                field.name
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                return Err(error);
            }
            if attrs.capability == Some(CapabilityKind::Global)
                && repr.is_some_and(|root| n.scoped_representation(root))
            {
                return Err(diagnostic(
                    const { RuleId::of("MODE-6") },
                    item.origin.span,
                    "a global capability representation cannot retain scoped content",
                ));
            }
            let authority = if opaque {
                declared_authority
            } else {
                field_authority.join(declared_authority)
            };
            let authority = if attrs.capability.is_some() {
                AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(item.definition.clone())].into(),
                )
            } else {
                authority
            };
            if !opaque || repr.is_some() {
                inferred.retain(|b| *b != Bound::Inert);
                if authority == AuthoritySummary::inert() && carrier.is_none() {
                    inferred.push(Bound::Inert);
                }
            }
            let alias =
                matches!(body, ast::TypeBody::Alias(_)) && *flavor != ast::TypeFlavor::Distinct;
            // An opaque declaration publishes its declared bounds, so its private
            // representation's declarations stay out of its public hash inputs.
            if (attrs.serializable || alias)
                && (!opaque || check.is_some())
                && let Some(root) = repr
            {
                n.load_schema_nominals(root)?;
            }
            let serializable = (attrs.serializable || alias)
                && authority == AuthoritySummary::inert()
                && repr.is_some_and(|root| n.satisfies(root, Bound::Serializable));
            // A generic opt-in whose fields need more than its parameters' bounds is
            // conditionally serializable: proved here with every type parameter admitted,
            // and at each use after substitution. It publishes no serializable bound.
            let conditional = !serializable
                && attrs.serializable
                && !n.generics.is_empty()
                && matches!(&authority, AuthoritySummary::Known(terms)
                    if terms.iter().all(|t| matches!(t, AuthorityTerm::TypeParameter(_))))
                && repr.is_some_and(|root| n.conditionally_serializable(root));
            if attrs.serializable && !serializable && !conditional {
                return Err(diagnostic(
                    const { RuleId::of("SER-1") },
                    item.origin.span,
                    "every serialized field needs an admitted schema; opaque or validated types require a data DTO",
                ));
            }
            // A checked opaque's representation qualifies, but only its check admits a
            // decoded value (protocol::validate_serializable_check), so it publishes no
            // structural bound.
            if serializable && check.is_none() {
                inferred.push(Bound::Serializable);
            }
            for bound in bounds.iter().flatten() {
                if (matches!(bound, Bound::Inert | Bound::Flat | Bound::Serializable)
                    && attrs.capability.is_some())
                    || (attrs.capability == Some(CapabilityKind::Scoped)
                        && matches!(bound, Bound::Plain | Bound::Copyable | Bound::Sendable))
                    || (matches!(bound, Bound::Data | Bound::Key)
                        && (*flavor == ast::TypeFlavor::Resource || attrs.capability.is_some()))
                    || ((!opaque || repr.is_some()) && !inferred.contains(bound))
                {
                    return Err(diagnostic(
                        if *bound == Bound::Serializable {
                            const { RuleId::of("SER-1") }
                        } else if (attrs.capability.is_some() || carrier.is_some())
                            && *bound == Bound::Inert
                        {
                            const { RuleId::of("CAP-2") }
                        } else if attrs.capability.is_some() && *bound == Bound::Sendable {
                            const { RuleId::of("MODE-4") }
                        } else if attrs.capability.is_some() && *bound == Bound::Serializable {
                            const { RuleId::of("SER-1") }
                        } else {
                            const { RuleId::of("GEN-1") }
                        },
                        item.origin.span,
                        "declared bound is not structural",
                    ));
                }
            }
            if opaque {
                inferred = bounds.clone().unwrap_or_default();
            }
            inferred.sort();
            inferred.dedup();
            SignatureKind::Type(TypeDefinition {
                loop_source: n.loop_source(&item.attributes)?,
                format: n.format(&item.attributes)?,
                destructor_allocation,
                generics: n.generics.clone(),
                representation: if opaque { None } else { repr },
                bounds: inferred,
                authority,
                capability: attrs.capability,
                implementation_authority: None,
                serializable: attrs.serializable,
                serializable_check: check.map(|function| SerializableCheck {
                    function,
                    representation: repr.expect("a checked opt-in has a representation"),
                }),
                layout: Layout {
                    flat: *flavor == ast::TypeFlavor::Flat,
                    ..attrs.layout
                },
                distinct: *flavor == ast::TypeFlavor::Distinct,
                resource: *flavor == ast::TypeFlavor::Resource,
                drop: drop.as_ref().map(|_| item.definition.clone()),
                fact,
                captures: carrier.as_ref().map(|fields| {
                    if opaque {
                        SharedSlice::default()
                    } else {
                        fields.iter().map(|f| *f as u32).collect()
                    }
                }),
            })
        }
    };
    n.finish_drop_summaries(&mut kind);
    let constant_alias = match &item.kind {
        ast::ItemKind::Const { value, .. } => {
            match &input.syntax.tree.expressions[value.0 as usize].kind {
                ast::ExprKind::Path(path) if matches!(&kind, SignatureKind::Const { ty, .. } if !n.satisfies(*ty, Bound::Data)) => {
                    Some(n.definition(path, None)?)
                }
                _ => None,
            }
        }
        _ => None,
    };
    let dependencies = n.ordered_dependencies();
    let owner = match &kind {
        SignatureKind::Type(declaration) => n.own_representation.map(|root| OwnerRepresentation {
            declaration: TypeDefinition {
                representation: Some(root),
                implementation_authority,
                captures: owner_captures.clone(),
                ..declaration.clone()
            },
        }),
        _ => None,
    };
    let mut signature = Signature {
        metadata: SignatureMetadata {
            inferred_unique: Default::default(),
            constant_alias,
            track_caller: attrs.track_caller,
            visibility: item.visibility,
            alias: item.visibility != ast::Visibility::PublicOpaque
                && matches!(
                    item.kind,
                    ast::ItemKind::Type {
                        body: ast::TypeBody::Alias(_),
                        ..
                    }
                ),
            schema: vec![],
            recursive_group: n.group_key,
            implementations: if matches!(item.kind, ast::ItemKind::Function { .. }) {
                input.resolution.implementation_binders.clone()
            } else {
                vec![]
            },
            unsafe_ffi,
            wrapper: input
                .metadata
                .unsafe_ffi
                .get(&item.definition.0.package)
                .and_then(|p| p.wrappers.get(&item.definition))
                .copied(),
            roles: NominalRoles {
                unsafe_capability: input.metadata.unsafe_capability.clone(),
                result_type: input.metadata.result_type.clone(),
                tasks_capability: input.metadata.tasks_capability.clone(),
                scope_capability: input.metadata.scope_capability.clone(),
                heap: input.metadata.heap.clone(),
            },
        },

        definition: DefId::intern(&item.definition),
        kind,
        fingerprint: ast::ContentHash::default(),
    };
    signature.fingerprint = fingerprint(n.types, &signature, &dependencies).map_err(|_| {
        diagnostic(
            const { RuleId::of("LANG-6") },
            item.origin.span,
            "invalid signature type graph",
        )
    })?;
    Ok(NormalizedDeclaration {
        public: QueryOutput {
            value: signature,
            diagnostics: vec![],
            dependencies,
        },
        owner,
    })
}
fn diagnostic(rule: RuleId, span: Span, message: &str) -> QueryError {
    QueryError::Diagnostics(vec![Diagnostic::error(rule, span, message)])
}
#[derive(Clone, Copy)]
enum Position {
    Borrow,
    Take,
    Result,
    Field,
}
/// A type declaration as another member of its recursive group reads it: its
/// header, and at the representation stage its representation, with no
/// derived facts. Its bounds are empty and its authority unknown until the
/// reader settles them, so every proof through it stays conservative.
fn provisional(
    n: &Normalizer<'_, '_>,
    item: &ast::Item,
    attrs: &Attributes,
    flavor: ast::TypeFlavor,
    representation: Option<TypeId>,
) -> NormalizedDeclaration {
    let alias = item.visibility != ast::Visibility::PublicOpaque
        && matches!(
            item.kind,
            ast::ItemKind::Type {
                body: ast::TypeBody::Alias(_),
                ..
            }
        );
    NormalizedDeclaration {
        public: QueryOutput {
            value: Signature {
                metadata: SignatureMetadata {
                    visibility: item.visibility,
                    alias,
                    ..Default::default()
                },
                definition: DefId::intern(&item.definition),
                kind: SignatureKind::Type(TypeDefinition {
                    loop_source: None,
                    format: None,
                    destructor_allocation: Allocation::MayAllocate,
                    generics: n.generics.clone(),
                    representation,
                    bounds: vec![],
                    authority: AuthoritySummary::Unknown,
                    capability: attrs.capability,
                    implementation_authority: None,
                    serializable: attrs.serializable,
                    serializable_check: None,
                    layout: Layout {
                        flat: flavor == ast::TypeFlavor::Flat,
                        ..attrs.layout
                    },
                    distinct: flavor == ast::TypeFlavor::Distinct,
                    resource: flavor == ast::TypeFlavor::Resource,
                    drop: None,
                    fact: None,
                    captures: None,
                }),
                fingerprint: ast::ContentHash::default(),
            },
            diagnostics: vec![],
            dependencies: n.ordered_dependencies(),
        },
        owner: None,
    }
}
#[derive(Default)]
struct Attributes {
    track_caller: bool,
    noalloc: bool,
    incomplete: bool,
    blocking: bool,
    layout: Layout,
    capability: Option<CapabilityKind>,
    serializable: bool,
}
struct Normalizer<'a, 'b> {
    input: &'a Input<'a>,
    provider: &'b mut dyn SignatureProvider,
    types: &'a TypeInterner,
    memo: &'a TypeMemo,
    own_representation: Option<TypeId>,
    generics: Vec<GenericParameter>,
    outer_generics: Vec<Vec<GenericParameter>>,
    parameters: Vec<Option<String>>,
    parameter_syntax: Vec<ast::TypeExprId>,
    outer_parameters: Vec<(Vec<Option<String>>, Vec<ast::TypeExprId>)>,
    siblings: Vec<String>,
    dependencies: DefMap<ast::ContentHash>,
    snapshots: DefMap<SignatureRef>,
    /// The other members of this type's recursive group, as provisional
    /// signatures (headers or representations). They are read in place of
    /// their published signatures, which would wait on this one, and are not
    /// dependencies: `group_key` covers them.
    group: BTreeMap<DefId, SignatureRef>,
    group_key: Option<ast::ContentHash>,
    /// The group members' settled cleanup effects over their own parameters
    /// (`settle_group_drop`), read in place of their provisional summaries.
    group_drop: BTreeMap<DefId, DropAllocation>,
    /// The core declaration each builtin row was resolved to. Interning merges
    /// equal occurrences, so a row any occurrence spelled without a resolved
    /// declaration maps to None: its authority stays unproved.
    builtin_declarations:
        std::collections::HashMap<TypeId, Option<SignatureRef>, hirsel_intern::IdHasher>,
    active: BTreeSet<ast::TypeExprId>,
    aliases: BTreeSet<DefId>,
    /// Relation places lowered from a written arena placement (PROV-4).
    arena_places: BTreeSet<Origin>,
    /// The parameter whose type is being lowered, so `take x: T in x` names
    /// its own argument (MODE-1).
    current_parameter: Option<usize>,
    public: bool,
    span: Span,
    proof_failure: &'b std::cell::RefCell<Option<QueryError>>,
}
impl Normalizer<'_, '_> {
    fn ordered_dependency_rows(&self) -> impl Iterator<Item = (DefId, ast::ContentHash)> {
        let mut dependencies: Vec<_> = self.dependencies.iter().map(|(d, h)| (*d, *h)).collect();
        dependencies.sort_by_key(|(definition, _)| definition.path());
        dependencies.into_iter()
    }
    fn ordered_dependencies(&self) -> Vec<SignatureDependency> {
        self.ordered_dependency_rows()
            .map(|(definition, fingerprint)| SignatureDependency {
                definition,
                fingerprint,
            })
            .collect()
    }

    /// A relation's lifetime sources: every reach except its arena places,
    /// which are where its blocks live (PROV-4).
    fn lifetime(&self, relation: &Relation) -> Relation {
        relation.retain(|r| !matches!(r, Reach::Place(o) if self.arena_places.contains(o)))
    }
    /// A relation's arena places only.
    fn placement(&self, relation: &Relation) -> Relation {
        relation.retain(|r| matches!(r, Reach::Place(o) if self.arena_places.contains(o)))
    }
    fn proof_failure<T>(&self, error: QueryError, fallback: T) -> T {
        self.proof_failure.borrow_mut().get_or_insert(error);
        fallback
    }
    fn path_name(&self, p: &ast::Path) -> Option<&str> {
        if p.package.is_none() && p.segments.len() == 1 {
            Some(p.segments[0].as_str())
        } else {
            None
        }
    }
    /// A generic parameter's name as a path spells it: one segment, or an
    /// abstract implementation binder's member `H.m` (GEN-6).
    fn generic_path_name(&self, p: &ast::Path) -> Option<String> {
        if let Some(name) = self.path_name(p) {
            return Some(name.to_owned());
        }
        match p.segments.as_slice() {
            [binder, member] if p.package.is_none() => {
                let name = format!("{binder}.{member}");
                self.generic(&name).map(|_| name)
            }
            _ => None,
        }
    }
    /// A `signature` (GEN-6). Each operation header is the bodyless
    /// declaration [`family_operation_path`] names; reading it here records
    /// it as a dependency and reports a malformed header at the signature.
    fn family(&mut self, members: &[ast::MemberSpec]) -> Result<SignatureKind, QueryError> {
        let path = self.input.syntax.item.definition.clone();
        let mut family = FamilySignature { members: vec![] };
        for member in members {
            let kind = match member {
                ast::MemberSpec::Type {
                    bounds, authority, ..
                } => {
                    let mut bounds = bounds.clone();
                    if authority.as_ref().is_some_and(Vec::is_empty) {
                        bounds.push(Bound::Inert);
                    }
                    bounds.sort();
                    bounds.dedup();
                    FamilyMemberKind::Type {
                        bounds: bounds.into(),
                    }
                }
                ast::MemberSpec::Size { .. } => FamilyMemberKind::Size,
                ast::MemberSpec::Operation {
                    name,
                    signature,
                    origin,
                } => {
                    if !signature.generics.is_empty() {
                        return Err(diagnostic(
                            const { RuleId::of("GEN-6") },
                            origin.span,
                            "a signature operation is first-order: it declares no generic parameters",
                        ));
                    }
                    self.snapshot(&family_operation_path(&path, name.as_str()))?;
                    FamilyMemberKind::Operation
                }
            };
            family.members.push(FamilyMember {
                name: member.name().as_str().to_owned(),
                kind,
            });
        }
        Ok(SignatureKind::Family(family))
    }
    /// An `implementation` (GEN-6): each member of its family paired by name
    /// and kind with exactly one written member. Compatibility of the actuals
    /// is the implementation matcher's, at checking.
    fn implementation(
        &mut self,
        members: &[ast::MemberDefinition],
    ) -> Result<SignatureKind, QueryError> {
        let item = &self.input.syntax.item;
        let mut paths = self
            .input
            .resolution
            .paths
            .iter()
            .filter(|(site, _)| *site == ast::NameSite::Item)
            .filter_map(|(_, name)| match name {
                ResolvedName::Definition(path) => Some(path.clone()),
                _ => None,
            });
        let Some(family_path) = paths.next() else {
            return Err(QueryError::NotImplemented);
        };
        let targets: Vec<ast::DefPath> = paths.collect();
        let family_signature = self.snapshot(&family_path)?;
        let SignatureKind::Family(family) = &family_signature.kind else {
            return Err(diagnostic(
                const { RuleId::of("GEN-6") },
                item.name_span,
                &format!("`{family_path}` is not a signature"),
            ));
        };
        let mut operations = members
            .iter()
            .filter(|m| matches!(m, ast::MemberDefinition::Operation { .. }))
            .zip(targets);
        let operations: Vec<_> = operations.by_ref().collect();
        let mut actuals = vec![];
        for member in &family.members {
            let written = members.iter().find(|m| m.name().as_str() == member.name);
            let actual = match (&member.kind, written) {
                (FamilyMemberKind::Type { .. }, Some(ast::MemberDefinition::Type { ty, .. })) => {
                    MemberActual::Type(self.ty(*ty, Position::Field)?)
                }
                (FamilyMemberKind::Size, Some(ast::MemberDefinition::Size { value, .. })) => {
                    MemberActual::Size(*value)
                }
                (
                    FamilyMemberKind::Operation,
                    Some(written @ ast::MemberDefinition::Operation { .. }),
                ) => {
                    let Some((_, target)) = operations.iter().find(|(m, _)| *m == written) else {
                        return Err(QueryError::NotImplemented);
                    };
                    let target_signature = self.snapshot(target)?;
                    if !matches!(target_signature.kind, SignatureKind::Function(_))
                        || !target_signature.metadata.implementations.is_empty()
                    {
                        return Err(diagnostic(
                            const { RuleId::of("GEN-6") },
                            written.origin().span,
                            &format!(
                                "operation `{}` names `{target}`, which is not a function without implementation binders",
                                member.name
                            ),
                        ));
                    }
                    MemberActual::Operation(target.clone())
                }
                (kind, None) => {
                    let what = match kind {
                        FamilyMemberKind::Type { .. } => "type",
                        FamilyMemberKind::Size => "size",
                        FamilyMemberKind::Operation => "operation",
                    };
                    return Err(diagnostic(
                        const { RuleId::of("GEN-6") },
                        item.name_span,
                        &format!(
                            "implementation is missing {what} member `{}` of `{family_path}`",
                            member.name
                        ),
                    ));
                }
                (_, Some(written)) => {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-6") },
                        written.origin().span,
                        &format!(
                            "member `{}` has a different kind in `{family_path}`",
                            member.name
                        ),
                    ));
                }
            };
            actuals.push((member.name.clone(), actual));
        }
        if let Some(extra) = members
            .iter()
            .find(|m| family.member(m.name().as_str()).is_none())
        {
            return Err(diagnostic(
                const { RuleId::of("GEN-6") },
                extra.origin().span,
                &format!("`{family_path}` has no member `{}`", extra.name()),
            ));
        }
        Ok(SignatureKind::Implementation(ImplementationSignature {
            family: DefId::intern(&family_path),
            members: actuals,
        }))
    }
    fn generic_parameters(
        &self,
        gs: &[ast::GenericParam],
    ) -> Result<Vec<GenericParameter>, QueryError> {
        let mut names = BTreeSet::new();
        gs.iter()
            .map(|g| {
                let name = g.name.as_str().to_owned();
                if !names.insert(name.clone()) {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-1") },
                        self.span,
                        "duplicate generic parameter",
                    ));
                }
                Ok(match &g.kind {
                    ast::GenericKind::Size => Some(GenericParameter::Size { name }),
                    ast::GenericKind::Type(bs) => {
                        let mut bounds = bs.clone();
                        bounds.sort();
                        bounds.dedup();
                        Some(GenericParameter::Type {
                            name,
                            bounds: bounds.into(),
                        })
                    }
                    ast::GenericKind::Implementation { .. } => None,
                })
            })
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()
            .map(|mut ordinary| {
                // GEN-6: abstract binders' member parameters follow, exactly as
                // the resolver declared them.
                if gs
                    .iter()
                    .any(|g| matches!(g.kind, ast::GenericKind::Implementation { .. }))
                {
                    let scope = self.input.syntax.owner().site(ast::NameSite::Item);
                    if let Some(declared) = self
                        .input
                        .resolution
                        .generic_declarations
                        .scope_parameters(scope)
                    {
                        ordinary.extend(declared.iter().skip(ordinary.len()).cloned());
                    }
                }
                ordinary
            })
    }
    fn attributes(
        &self,
        as_: &[ast::Attribute],
        kind: &ast::ItemKind,
    ) -> Result<Attributes, QueryError> {
        let mut result = Attributes::default();
        let mut seen = BTreeSet::new();
        for a in as_ {
            let name = a.name.as_str().to_owned();
            if !seen.insert(name.clone()) {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    a.origin.span,
                    "duplicate attribute",
                ));
            }
            let function = matches!(kind, ast::ItemKind::Function { .. });
            let type_ = matches!(kind, ast::ItemKind::Type { .. });
            let valid = match name.as_str() {
                "test" | "bench" | "noalloc" | "incomplete" | "advance_only"
                | "preserves_captures" | "preserves_unique" => function,
                "captures" => function || type_,
                "track_caller" => matches!(kind, ast::ItemKind::Function { abi: None, .. }),
                "blocking" => matches!(kind, ast::ItemKind::Function { abi: Some(_), .. }),
                "layout" | "packed" | "align" | "soa" | "capability" | "loop_source" | "format"
                | "serializable" => type_,
                _ => false,
            };
            if !valid {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    a.origin.span,
                    "attribute is not permitted here",
                ));
            }
            match name.as_str() {
                "advance_only" | "preserves_captures" | "preserves_unique" | "captures" => {
                    if !matches!(a.arguments.as_slice(), [ast::AttributeArgument::StatePromise(s)] if !s.selectors.is_empty())
                    {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-6") },
                            a.origin.span,
                            "a callable contract requires nonempty parameter selectors",
                        ));
                    }
                }
                "loop_source" => {
                    if !matches!(
                        a.arguments.as_slice(),
                        [ast::AttributeArgument::LoopSource(_)]
                    ) {
                        return Err(diagnostic(
                            const { RuleId::of("SEM-4") },
                            a.origin.span,
                            "@loop_source requires open and next paths",
                        ));
                    }
                }
                "format" => {
                    if !matches!(a.arguments.as_slice(), [ast::AttributeArgument::Format(_)]) {
                        return Err(diagnostic(
                            const { RuleId::of("SEM-3") },
                            a.origin.span,
                            "@format requires one function path",
                        ));
                    }
                }
                "serializable" => {
                    if !matches!(
                        a.arguments.as_slice(),
                        [] | [ast::AttributeArgument::SerializableCheck(_)]
                    ) {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-6") },
                            a.origin.span,
                            "@serializable takes no arguments or one check function",
                        ));
                    }
                    result.serializable = true;
                }
                "test" | "bench" | "track_caller" | "noalloc" | "incomplete" | "blocking"
                | "packed" | "soa" => {
                    if !a.arguments.is_empty() {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-6") },
                            a.origin.span,
                            "attribute takes no arguments",
                        ));
                    }
                    match name.as_str() {
                        "test" | "bench" => (),
                        "track_caller" => result.track_caller = true,
                        "noalloc" => result.noalloc = true,
                        "incomplete" => result.incomplete = true,
                        "blocking" => result.blocking = true,
                        "packed" => result.layout.packed = true,
                        _ => result.layout.soa = true,
                    }
                }
                "layout" => {
                    if let [ast::AttributeArgument::Name(s)] = a.arguments.as_slice() {
                        if s.as_str() != "c" {
                            return Err(diagnostic(
                                const { RuleId::of("LANG-7") },
                                a.origin.span,
                                "unsupported layout",
                            ));
                        }
                        result.layout.c = true;
                    } else {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-7") },
                            a.origin.span,
                            "layout requires c",
                        ));
                    }
                }
                "align" => {
                    if let [ast::AttributeArgument::Integer(v)] = a.arguments.as_slice() {
                        let n = v
                            .replace('_', "")
                            .parse::<u32>()
                            .ok()
                            .filter(|n| n.is_power_of_two())
                            .ok_or_else(|| {
                                diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    a.origin.span,
                                    "alignment must be a positive power of two",
                                )
                            })?;
                        result.layout.align = Some(n);
                    } else {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-7") },
                            a.origin.span,
                            "align requires an integer",
                        ));
                    }
                }
                "capability" => {
                    result.capability = match a.arguments.as_slice() {
                        [ast::AttributeArgument::Keyword(ast::Keyword::Global)] => {
                            Some(CapabilityKind::Global)
                        }
                        [ast::AttributeArgument::Keyword(ast::Keyword::Local)] => {
                            Some(CapabilityKind::Scoped)
                        }
                        _ => {
                            return Err(diagnostic(
                                const { RuleId::of("LANG-6") },
                                a.origin.span,
                                "capability requires global or local",
                            ));
                        }
                    };
                }
                _ => unreachable!(),
            }
        }
        Ok(result)
    }
    fn loop_source(
        &mut self,
        attributes: &[ast::Attribute],
    ) -> Result<Option<LoopSourceDeclaration>, QueryError> {
        let Some(a) = attributes.iter().find(|a| a.name.as_str() == "loop_source") else {
            return Ok(None);
        };
        let [ast::AttributeArgument::LoopSource(s)] = a.arguments.as_slice() else {
            return Err(diagnostic(
                const { RuleId::of("SEM-4") },
                a.origin.span,
                "@loop_source requires open and next paths",
            ));
        };
        Ok(Some(LoopSourceDeclaration {
            open: self.definition(&s.open, None)?,
            next: self.definition(&s.next, None)?,
            option: ast::DefPath(ast::ItemPath {
                package: ast::PackageId {
                    name: "core".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: "Option".into(),
            }),
        }))
    }
    fn format(
        &mut self,
        attributes: &[ast::Attribute],
    ) -> Result<Option<ast::DefPath>, QueryError> {
        let Some(a) = attributes.iter().find(|a| a.name.as_str() == "format") else {
            return Ok(None);
        };
        let [ast::AttributeArgument::Format(s)] = a.arguments.as_slice() else {
            return Err(diagnostic(
                const { RuleId::of("SEM-3") },
                a.origin.span,
                "@format requires one function path",
            ));
        };
        Ok(Some(self.definition(&s.function, None)?))
    }
    fn serializable_check(
        &mut self,
        attributes: &[ast::Attribute],
    ) -> Result<Option<ast::DefPath>, QueryError> {
        let check = attributes
            .iter()
            .find_map(|a| match a.arguments.as_slice() {
                [ast::AttributeArgument::SerializableCheck(s)]
                    if a.name.as_str() == "serializable" =>
                {
                    Some(s)
                }
                _ => None,
            });
        check
            .map(|s| self.definition(&s.function, None))
            .transpose()
    }
    fn generic(&self, name: &str) -> Option<(u32, u32, &GenericParameter)> {
        std::iter::once(&self.generics)
            .chain(self.outer_generics.iter().rev())
            .enumerate()
            .find_map(|(depth, gs)| {
                gs.iter().enumerate().find_map(|(ordinal, g)| {
                    let n = match g {
                        GenericParameter::Type { name, .. } | GenericParameter::Size { name } => {
                            name
                        }
                    };
                    (n == name).then_some((depth as u32, ordinal as u32, g))
                })
            })
    }
    fn declarations(&self) -> GenericDeclarations {
        GenericDeclarations::from_binders(
            std::iter::once(&self.generics)
                .chain(self.outer_generics.iter().rev())
                .enumerate()
                .map(|(d, ps)| (GenericBinder::Bound(d as u32), ps.clone())),
        )
    }
    fn function(
        &mut self,
        s: &ast::FunctionSyntax,
        outer: &Attributes,
        closure: bool,
        abi: Option<ast::ForeignAbi>,
    ) -> Result<FunctionType, QueryError> {
        let saved_g = self.generics.clone();
        self.outer_generics.push(saved_g.clone());
        self.outer_parameters
            .push((self.parameters.clone(), self.parameter_syntax.clone()));
        self.parameter_syntax = s.parameters.iter().map(|p| p.ty).collect();
        let own_g = self.generic_parameters(&s.generics)?;
        // Nested function-type binders are not in the grammar (#24).
        if closure && !own_g.is_empty() {
            return Err(diagnostic(
                const { RuleId::of("GEN-1") },
                self.span,
                "function types cannot declare generic binders",
            ));
        }
        self.generics = own_g.clone();
        self.parameters = s
            .parameters
            .iter()
            .map(|p| p.name.map(|n| n.as_str().to_owned()))
            .collect();
        let mut names = BTreeSet::new();
        let mut labels = BTreeSet::new();
        for p in &s.parameters {
            if !closure && p.name.is_none() {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "declaration parameter requires a name and type",
                ));
            }
            if let Some(name) = p.name {
                if !names.insert(name) {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-12") },
                        self.span,
                        "duplicate parameter name",
                    ));
                }
            }
            if let Some(label) = p.label {
                if !labels.insert(label) {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-12") },
                        self.span,
                        "duplicate parameter label",
                    ));
                }
            }
        }
        // The item's own ABI, so an extern's `@blocking` is checked as an extern's.
        let dummy = ast::ItemKind::Function {
            signature: s.clone(),
            body: None,
            abi,
        };
        if closure
            && let Some(name) = s
                .attributes
                .iter()
                .map(|a| a.name.as_str())
                .find(|name| matches!(*name, "test" | "bench"))
        {
            return Err(diagnostic(
                const { RuleId::of("LANG-9") },
                self.span,
                &format!("@{name} selects only an ordinary function definition"),
            ));
        }
        if closure
            && let Some(attribute) = s
                .attributes
                .iter()
                .find(|a| a.name.as_str() == "track_caller")
        {
            return Err(diagnostic(
                const { RuleId::of("LANG-6") },
                attribute.origin.span,
                "@track_caller applies only to ordinary named function items",
            ));
        }
        let attrs = self.attributes(&s.attributes, &dummy)?;
        let current = self.current_parameter;
        let mut parameters = s
            .parameters
            .iter()
            .enumerate()
            .map(|(i, p)| {
                self.current_parameter = Some(i);
                let ty = self.ty(
                    p.ty,
                    if p.passing == PassingMode::Take {
                        Position::Take
                    } else {
                        Position::Borrow
                    },
                );
                self.current_parameter = current;
                let ty = ty?;
                Ok(Parameter {
                    name: p.name.map(|n| n.as_str().to_owned()),
                    label: p.label.map(|n| n.as_str().to_owned()),
                    passing: p.passing,
                    ty,
                })
            })
            .collect::<Result<Vec<_>, QueryError>>()?;
        let result = match s.result {
            Some(t) => self.ty(t, Position::Result)?,
            None => self.push(TypeKind::Tuple(vec![].into()), Position::Result)?,
        };
        // MODE-1: a named function has no environment, so its local result
        // names its sources (spec #70).
        if !closure
            && (self
                .types
                .get(result)
                .unwrap()
                .modes
                .relation
                .has_environment()
                || s.result
                    .and_then(|t| written_local(&self.input.syntax.tree, t))
                    == Some(true))
        {
            return Err(diagnostic(
                const { RuleId::of("MODE-1") },
                s.result.map_or(self.span, |t| {
                    self.input.syntax.tree.types[t.0 as usize].origin.span
                }),
                "a named function has no environment: its local result names its sources, `in x`",
            ));
        }
        // MODE-1 (#72): a function type's `in env` alone may not narrow a
        // non-Plain parameter out of its result.
        if closure
            && let Some(span) = s
                .result
                .and_then(|t| environment_alone(&self.input.syntax.tree, t))
        {
            let candidates: Vec<String> = parameters
                .iter()
                .enumerate()
                .filter(|(_, p)| !self.satisfies(p.ty, Bound::Plain))
                .map(|(i, p)| {
                    p.name
                        .as_ref()
                        .map_or_else(|| format!("parameter {}", i + 1), |n| n.to_string())
                })
                .collect();
            if !candidates.is_empty() {
                return Err(diagnostic(
                    const { RuleId::of("MODE-1") },
                    span,
                    &environment_candidates(&candidates),
                ));
            }
        }
        let arenas: BTreeSet<ParamId> = match &self.input.metadata.heap {
            Some(heap) => parameters
                .iter()
                .enumerate()
                .filter(|(_, p)| hirsel_types::is_arena_type(&self.types, p.ty, &heap.arena))
                .map(|(i, _)| ParamId(i as u32))
                .collect(),
            None => BTreeSet::new(),
        };
        let arena_parameter = |o: &Origin| {
            matches!(o.root, OriginRoot::Parameter(hirsel_types::ParameterRef::Bound {
                depth: 0,
                parameter,
            }) if o.fields.is_empty() && arenas.contains(&parameter))
        };
        let mut result =
            local_result_destination(&mut self.types, result, &parameters, arena_parameter);
        let mut state_promises = BTreeMap::<RelationPlace, StatePromise>::new();
        let mut captures = Vec::new();
        for attribute in &s.attributes {
            let [ast::AttributeArgument::StatePromise(promise)] = attribute.arguments.as_slice()
            else {
                continue;
            };
            for path in &promise.selectors {
                let (origin, _) = self.typed_origin(path, false, true)?;
                let OriginRoot::Parameter(
                    parameter @ ParameterRef::Bound {
                        depth: 0,
                        parameter: ordinal,
                    },
                ) = origin.root
                else {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-6") },
                        promise.origin.span,
                        "a callable selector names its own parameter and explicit fields",
                    ));
                };
                let selector = RelationPlace {
                    parameter,
                    fields: origin.fields,
                };
                if promise.kind == ast::StatePromiseKind::Captures {
                    captures.push(selector);
                } else {
                    let unique = promise.kind == ast::StatePromiseKind::PreserveUnique;
                    let passing = parameters[ordinal.0 as usize].passing;
                    if passing != PassingMode::Mut && !(unique && passing == PassingMode::Take) {
                        return Err(diagnostic(
                            const { RuleId::of("MEM-14") },
                            promise.origin.span,
                            "a state preservation selector requires a mut parameter",
                        ));
                    }
                    if unique && !selector.fields.is_empty() {
                        return Err(diagnostic(
                            const { RuleId::of("MODE-2") },
                            promise.origin.span,
                            "@preserves_unique selects a whole mut parameter",
                        ));
                    }
                    let row = state_promises
                        .entry(selector.clone())
                        .or_insert(StatePromise {
                            selector,
                            advance_only: false,
                            preserve_captures: false,
                            preserve_unique: false,
                            unretained: Default::default(),
                        });
                    row.advance_only |= promise.kind == ast::StatePromiseKind::AdvanceOnly;
                    row.preserve_captures |= !unique;
                    row.preserve_unique |= unique;
                }
            }
        }
        if !captures.is_empty() {
            captures.sort();
            captures.dedup();
            let mut row = self.types.get(result).unwrap().clone();
            if !matches!(
                row.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) && !self.erased_carrier_type(result)
            {
                return Err(diagnostic(
                    const { RuleId::of("CLO-4") },
                    self.span,
                    "@captures requires a callable, erased iterator or erased carrier result",
                ));
            }
            row.erased_captures = Some(ErasedCaptures::FromArguments(captures.into()));
            result = self.types.intern(row);
        }
        let state_promises: SharedSlice<_> = state_promises.into_values().collect();
        apply_state_promises(&mut self.types, &mut parameters, &state_promises);
        let authority = AuthoritySummary::Known(
            parameters
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    self.capability(p.ty).map(|_| {
                        AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(ParamId(
                            i as u32,
                        )))
                    })
                })
                .collect(),
        );
        let uses = match &s.uses {
            None => Uses::Unrestricted,
            Some(ps) => Uses::Only(self.authority(ps)?),
        };
        let authority = if closure {
            match &uses {
                Uses::Unrestricted => AuthoritySummary::Unknown,
                Uses::Only(a) => a.clone(),
            }
        } else {
            authority
        };
        let contracts = s
            .contracts
            .iter()
            .enumerate()
            .map(|(i, c)| self.contract(c.kind, c.expression, i as u32))
            .collect::<Result<_, _>>()?;
        let error = result_error(
            &self.types,
            result,
            &NominalRoles {
                unsafe_capability: self.input.metadata.unsafe_capability.clone(),
                result_type: self.input.metadata.result_type.clone(),
                tasks_capability: self.input.metadata.tasks_capability.clone(),
                scope_capability: self.input.metadata.scope_capability.clone(),
                heap: self.input.metadata.heap.clone(),
            },
        );
        self.outer_generics.pop();
        self.generics = saved_g;
        (self.parameters, self.parameter_syntax) = self.outer_parameters.pop().unwrap();
        Ok(FunctionType {
            state_promises,
            generics: own_g.into(),
            parameters: parameters.into(),
            result,
            error,
            allocation: if outer.noalloc || attrs.noalloc {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            },
            authority,
            uses,
            contracts,
            incomplete: outer.incomplete || attrs.incomplete,
            foreign: None,
        })
    }
    fn push(&mut self, kind: TypeKind, pos: Position) -> Result<TypeId, QueryError> {
        let mut modes = Modes::global();
        if matches!(pos, Position::Borrow) {
            modes.relation = Relation::environment();
        }
        let id = self.types.intern(Type {
            drop_allocation: self.drop_kind(&kind, &mut IdSet::default()),
            modes,
            ..Type::with_kind(kind)
        });
        let mut adjusted = self.types.get(id).unwrap().clone();
        let generics = self.declarations();
        // The callback records dependencies, but never accesses owner type rows.
        let types = self.types;
        let plain = structural_property(
            types,
            id,
            StructuralProperty::Plain,
            &generics,
            &mut |definition: DefId| self.property_signature(definition),
        );
        if plain? {
            adjusted.canonicalize_plain();
        }
        let normalized = self.types.intern(adjusted.clone());
        adjusted.modes.sendability = self.sendability(normalized)?;
        Ok(self.types.intern(adjusted))
    }
    fn property_signature(
        &mut self,
        definition: DefId,
    ) -> Result<Option<SignatureRef>, QueryError> {
        if definition.path() == &self.input.syntax.item.definition {
            // The current declaration is not published yet. Recursive nominal
            // proofs remain conservative until its representation is complete.
            return Ok(self.snapshots.get(&definition).cloned());
        }
        self.snapshot(definition).map(Some)
    }
    fn sendability(&mut self, root: TypeId) -> Result<Sendability, QueryError> {
        // A declared receiving contract supplies entry assumptions, with every
        // caller/result edge independently checked before receipt.
        if self.types.get(root).unwrap().sendability_requirement == SendabilityRequirement::Sendable
        {
            return Ok(Sendability::Sendable);
        }
        let generics = self.declarations();
        let types = self.types;
        structural_sendability(types, root, &generics, &mut |definition: DefId| {
            self.property_signature(definition)
        })
    }
    fn definition(
        &mut self,
        p: &ast::Path,
        id: Option<ast::TypeExprId>,
    ) -> Result<ast::DefPath, QueryError> {
        if let Some(id) = id {
            if let Some((_, ResolvedName::Definition(d))) =
                self.input.resolution.types.iter().find(|(t, _)| *t == id)
            {
                return Ok(d.clone());
            }
        }
        // Header paths without a TypeExprId (dictionary alias initializers,
        // brands and authority names) still have exact manifest lookup reads.
        // Reuse them before provider lookup, which cannot expand exposed heads.
        let resolved =
            self.input
                .resolution
                .reads
                .iter()
                .rev()
                .find_map(|read| match read.as_ref() {
                    DependencyRead::Lookup {
                        context,
                        package,
                        segments,
                        result: Ok(definition),
                        ..
                    } if context == self.input.context
                        && package == &p.package
                        && segments
                            .iter()
                            .map(String::as_str)
                            .eq(p.segments.iter().map(|s| s.as_str())) =>
                    {
                        Some(definition.clone())
                    }
                    _ => None,
                });
        if let Some(definition) = resolved {
            return Ok(definition.as_ref().clone());
        }
        self.provider.lookup(self.input.context, p).result
    }
    /// A SER-1 proof expands every nominal representation its schema reaches, not
    /// only the declarations this item names, so load (and depend on) those first.
    /// A declaration that cannot be read stays absent and the proof conservative.
    fn load_schema_nominals(&mut self, root: TypeId) -> Result<(), QueryError> {
        let own = self.input.syntax.item.definition.clone();
        let mut seen = IdSet::default();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(row) = self.types.get(id) else {
                continue;
            };
            if matches!(row.kind, TypeKind::Function(_)) {
                continue;
            }
            let named = match &row.kind {
                TypeKind::Named { definition, .. } if definition.path() != &own => {
                    Some(*definition)
                }
                _ => None,
            };
            pending.extend(row.kind.value_parts());
            let Some(definition) = named else { continue };
            let s = match self.snapshot(definition) {
                Ok(s) => s,
                Err(e @ QueryError::Operational(_)) => return Err(e),
                Err(_) => continue,
            };
            if let SignatureKind::Type(d) = &s.kind
                && let Some(representation) = d.representation
            {
                pending.push(representation);
            }
        }
        Ok(())
    }
    fn snapshot(&mut self, definition: impl Into<DefId>) -> Result<SignatureRef, QueryError> {
        let d = definition.into();
        if let Some(member) = self.group.get(&d) {
            return Ok(member.clone());
        }
        if let Some(s) = self.snapshots.get(&d) {
            self.dependencies.insert(d, s.fingerprint);
            return Ok(s.clone());
        }
        let s = self.provider.signature(d)?;
        if s.definition != d {
            return Err(diagnostic(
                const { RuleId::of("LANG-6") },
                self.span,
                "provider returned the wrong definition",
            ));
        }
        self.dependencies.insert(d, s.fingerprint);
        self.snapshots.insert(d, s.clone());
        Ok(s)
    }
    fn ty(&mut self, id: ast::TypeExprId, pos: Position) -> Result<TypeId, QueryError> {
        let t = self
            .input
            .syntax
            .tree
            .types
            .get(id.0 as usize)
            .ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "missing written type",
                )
            })?
            .clone();
        if !self.active.insert(id) {
            return Err(diagnostic(
                const { RuleId::of("LANG-7") },
                t.origin.span,
                "recursive syntax type graph",
            ));
        }
        let prev = self.span;
        self.span = t.origin.span;
        let result = match &t.kind {
            ast::TypeExprKind::Error => {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    t.origin.span,
                    "missing written type; signatures cannot infer it",
                ));
            }
            ast::TypeExprKind::Tuple(ts) => {
                let ts = ts.iter().map(|t| self.part(*t)).collect::<Result<_, _>>()?;
                self.push(TypeKind::Tuple(ts), pos)?
            }
            ast::TypeExprKind::Box(t) => {
                let t = self.part(*t)?;
                self.push(TypeKind::Box(t), pos)?
            }
            ast::TypeExprKind::Function(s) => {
                let f = self.function(s, &Attributes::default(), true, None)?;
                self.push(TypeKind::Function(f), pos)?
            }
            ast::TypeExprKind::Qualified {
                base,
                modifiers,
                brand,
                relation,
            } => {
                let id = self.ty(*base, pos)?;
                let mut t = self.types.get(id).unwrap().clone();
                let plain = self.satisfies(id, Bound::Plain);
                self.qualify(&mut t, modifiers, brand.as_ref(), relation.as_ref(), pos)?;
                if plain {
                    t.canonicalize_plain();
                }
                let declaration = self.builtin_declarations.get(&id).cloned();
                let id = self.types.intern(t);
                if let Some(declaration) = declaration {
                    self.builtin_declaration(id, declaration);
                }
                id
            }
            ast::TypeExprKind::Named { path, arguments } => {
                let name = self.generic_path_name(path);
                if name.as_deref() == Some("any") {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-7") },
                        t.origin.span,
                        "any is not a type",
                    ));
                }
                if let Some((depth, i, g)) = name.as_deref().and_then(|name| self.generic(name)) {
                    if !arguments.is_empty() {
                        return Err(diagnostic(
                            const { RuleId::of("GEN-1") },
                            t.origin.span,
                            "generic parameter cannot be applied",
                        ));
                    }
                    match g {
                        GenericParameter::Type { .. } => self.push(
                            TypeKind::Parameter(TypeParameterRef::Bound {
                                depth,
                                parameter: TypeParamId(i),
                            }),
                            pos,
                        )?,
                        GenericParameter::Size { name } => {
                            let mut error = Diagnostic::error(
                                const { RuleId::of("GEN-1") },
                                t.origin.span,
                                "size parameter cannot be used as a type",
                            );
                            error.notes.push(format!(
                                "help: `{name}` is a size parameter; use it in a size argument, for example `Inline[u8, {name}]`. A type position needs a type parameter: declare a separate `T` with `[T, {name}: size]` and use `T` here, or change the binder to `[{name}]` if it was intended as a type parameter"
                            ));
                            return Err(QueryError::Diagnostics(vec![error]));
                        }
                    }
                } else if let Some(p) = name.as_deref().and_then(primitive) {
                    if !arguments.is_empty() {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-7") },
                            t.origin.span,
                            "primitive type cannot take arguments",
                        ));
                    }
                    self.push(TypeKind::Primitive(p), pos)?
                } else {
                    let args = arguments
                        .iter()
                        .map(|a| self.argument(a))
                        .collect::<Result<Vec<_>, _>>()?;
                    // Builtin identity is the core@1 declaration's, never the
                    // spelling's: a user `Map` stays nominal and `core@1/Map` is
                    // builtin. Only an unresolved name falls back to its spelling.
                    let resolved = self
                        .input
                        .resolution
                        .types
                        .iter()
                        .find(|(t, _)| *t == id)
                        .map(|(_, r)| r);
                    let b = match resolved {
                        Some(ResolvedName::Definition(d))
                            if d.package_is("core", "1") && d.0.modules.is_empty() =>
                        {
                            builtin(&d.0.name)
                        }
                        Some(_) => None,
                        None => name.as_deref().and_then(builtin),
                    };
                    if let Some(b) = b {
                        let declaration = if let Some(ResolvedName::Definition(d)) = resolved {
                            let declaration = self.snapshot(d)?;
                            matches!(declaration.kind, SignatureKind::Type(_))
                                .then_some(declaration)
                        } else {
                            None
                        };
                        self.builtin_arguments(b, &args, declaration.as_deref())?;
                        let id = self.push(
                            TypeKind::Builtin {
                                kind: b,
                                arguments: args.into(),
                            },
                            pos,
                        )?;
                        self.builtin_declaration(id, declaration);
                        id
                    } else {
                        let d = self.definition(path, Some(id))?;
                        let own = &self.input.syntax.item;
                        let s = if own.definition == d {
                            None
                        } else {
                            Some(self.snapshot(&d)?)
                        };
                        let gs = if let Some(s) = &s {
                            match &s.kind {
                                SignatureKind::Type(t) => t.generics.clone(),
                                _ => {
                                    return Err(diagnostic(
                                        const { RuleId::of("LANG-7") },
                                        t.origin.span,
                                        "value definition used as a type",
                                    ));
                                }
                            }
                        } else {
                            self.generics.clone()
                        };
                        self.check_arguments(&gs, &args)?;
                        let public = s
                            .as_ref()
                            .map_or(own.visibility != ast::Visibility::Private, |s| {
                                s.metadata.visibility != ast::Visibility::Private
                            });
                        if self.public && !public {
                            return Err(diagnostic(
                                const { RuleId::of("PROG-2") },
                                prev,
                                "public signature mentions a private type",
                            ));
                        }
                        if s.as_ref().is_some_and(|s| s.metadata.alias) {
                            let s = s.ok_or_else(|| {
                                diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    t.origin.span,
                                    "recursive type alias",
                                )
                            })?;
                            if !self.aliases.insert(DefId::intern(&d)) {
                                return Err(diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    t.origin.span,
                                    "recursive type alias",
                                ));
                            }
                            let SignatureKind::Type(def) = &s.kind else {
                                unreachable!()
                            };
                            let r = def.representation.ok_or_else(|| {
                                diagnostic(
                                    const { RuleId::of("LANG-7") },
                                    t.origin.span,
                                    "opaque type cannot be an alias",
                                )
                            })?;
                            let imported = self.import(r, &args)?;
                            self.aliases.remove(&DefId::intern(&d));
                            let mut ty = self.types.get(imported).unwrap().clone();
                            if matches!(pos, Position::Borrow)
                                && !self.satisfies(imported, Bound::Plain)
                            {
                                ty.modes.relation = Relation::environment()
                                    .union(&self.placement(&ty.modes.relation));
                            }
                            let adjusted = self.types.intern(ty.clone());
                            ty.modes.sendability = self.sendability(adjusted)?;
                            self.types.intern(ty)
                        } else {
                            if let Some(Signature {
                                kind: SignatureKind::Type(def),
                                ..
                            }) = s.as_deref()
                            {
                                if let Some(repr) = def.representation {
                                    // A group member is read as this declaration
                                    // reads itself: its fields are its own to check.
                                    if !args.is_empty()
                                        && !self.group.contains_key(&DefId::intern(&d))
                                    {
                                        // Import normalizes actual declaration fields and records
                                        // their signature reads; proof policy lives in shared helpers.
                                        self.import(repr, &args)?;
                                    }
                                }
                            }
                            let id = self.push(
                                TypeKind::Named {
                                    definition: DefId::intern(&d),
                                    arguments: args.into(),
                                },
                                pos,
                            )?;
                            id
                        }
                    }
                }
            }
        };
        self.span = prev;
        self.active.remove(&id);
        Ok(result)
    }
    /// A field, payload, element or type argument is as local as its owner and
    /// its current content (OWN-8; spec #71).
    /// Locality written on one, or reaching it through an alias, is LANG-7.
    fn part(&mut self, id: ast::TypeExprId) -> Result<TypeId, QueryError> {
        let ty = self.ty(id, Position::Field)?;
        if !self
            .lifetime(&self.types.get(ty).unwrap().modes.relation)
            .is_global()
        {
            return Err(diagnostic(
                const { RuleId::of("LANG-7") },
                self.input.syntax.tree.types[id.0 as usize].origin.span,
                PART_LOCALITY,
            ));
        }
        Ok(ty)
    }
    fn argument(&mut self, a: &ast::TypeArgument) -> Result<GenericArgument, QueryError> {
        match a {
            ast::TypeArgument::SizeLiteral(n) => Ok(GenericArgument::Size(Size::Constant(*n))),
            ast::TypeArgument::Type(id) => {
                if let Some(ast::TypeExpr {
                    kind: ast::TypeExprKind::Named { path, arguments },
                    ..
                }) = self.input.syntax.tree.types.get(id.0 as usize)
                {
                    if arguments.is_empty() {
                        if let Some(name) = self.generic_path_name(path) {
                            if let Some((depth, i, GenericParameter::Size { .. })) =
                                self.generic(&name)
                            {
                                return Ok(GenericArgument::Size(Size::Parameter(
                                    SizeParameterRef::Bound {
                                        depth,
                                        parameter: SizeParamId(i),
                                    },
                                )));
                            }
                        }
                    }
                }
                Ok(GenericArgument::Type(self.part(*id)?))
            }
        }
    }
    fn check_arguments(
        &mut self,
        gs: &[GenericParameter],
        args: &[GenericArgument],
    ) -> Result<(), QueryError> {
        for (g, a) in gs.iter().zip(args) {
            if let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(t)) = (g, a)
                && bounds.contains(&Bound::Serializable)
            {
                self.load_schema_nominals(*t)?;
            }
        }
        if gs.len() != args.len() {
            return Err(diagnostic(
                const { RuleId::of("GEN-1") },
                self.span,
                "wrong generic argument count",
            ));
        }
        for (g, a) in gs.iter().zip(args) {
            match (g, a) {
                (GenericParameter::Size { .. }, GenericArgument::Size(_)) => {}
                (GenericParameter::Type { bounds, .. }, GenericArgument::Type(t)) => {
                    for b in bounds {
                        if !self.satisfies(*t, *b) {
                            return Err(diagnostic(
                                if *b == Bound::Serializable {
                                    const { RuleId::of("SER-1") }
                                } else {
                                    const { RuleId::of("GEN-1") }
                                },
                                self.span,
                                "type argument does not satisfy structural bound",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-1") },
                        self.span,
                        "type/size argument kind mismatch",
                    ));
                }
            }
        }
        Ok(())
    }
    fn builtin_arguments(
        &self,
        b: BuiltinType,
        a: &[GenericArgument],
        declaration: Option<&Signature>,
    ) -> Result<(), QueryError> {
        let kinds = match b {
            BuiltinType::Map | BuiltinType::SortedMap => vec![false, false],
            BuiltinType::Inline | BuiltinType::Simd => vec![false, true],
            BuiltinType::FnPtr => {
                if a.is_empty() || !a.iter().all(|a| matches!(a, GenericArgument::Type(_))) {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-1") },
                        self.span,
                        "FnPtr requires type arguments",
                    ));
                }
                return Ok(());
            }
            _ => vec![false],
        };
        if a.len() != kinds.len()
            || a.iter()
                .zip(kinds)
                .any(|(a, size)| size != matches!(a, GenericArgument::Size(_)))
        {
            return Err(diagnostic(
                const { RuleId::of("GEN-1") },
                self.span,
                "wrong builtin type/size arguments",
            ));
        }
        if let Some(declaration) = declaration {
            if let Some((rule, message)) =
                schema_failure(&self.types, a, &declaration.metadata.schema, |ty, bound| {
                    self.satisfies(ty, bound)
                })
            {
                return Err(diagnostic(rule, self.span, message));
            }
        }
        if let Some(b) = builtin_bound(b) {
            if let GenericArgument::Type(t) = a[0] {
                if !self.satisfies(t, b) {
                    return Err(diagnostic(
                        const { RuleId::of("GEN-1") },
                        self.span,
                        "builtin argument does not satisfy its bound",
                    ));
                }
            }
        }
        Ok(())
    }
    fn type_body(&mut self, b: &ast::TypeBody) -> Result<Option<TypeId>, QueryError> {
        let kind = match b {
            ast::TypeBody::Opaque => return Ok(None),
            ast::TypeBody::Alias(t) => return self.ty(*t, Position::Field).map(Some),
            ast::TypeBody::Distinct { field, .. } => TypeKind::Record(
                vec![Field {
                    growable: field.growable,
                    name: field.name,
                    ty: self.part(field.ty)?,
                }]
                .into(),
            ),
            ast::TypeBody::Record(fs) => {
                self.siblings = fs.iter().map(|f| f.name.as_str().to_owned()).collect();
                let mut seen = BTreeSet::new();
                let mut fields = vec![];
                for f in fs {
                    if !seen.insert(f.name) {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-12") },
                            f.origin.span,
                            "duplicate record field",
                        ));
                    }
                    fields.push(Field {
                        growable: f.growable,
                        name: f.name,
                        ty: self.part(f.ty)?,
                    });
                }
                TypeKind::Record(fields.into())
            }
            ast::TypeBody::Variants(vs) => {
                let mut seen = BTreeSet::new();
                let mut variants = vec![];
                for v in vs {
                    if !seen.insert(v.name) {
                        return Err(diagnostic(
                            const { RuleId::of("LANG-7") },
                            self.span,
                            "duplicate variant",
                        ));
                    }
                    let mut labels = BTreeSet::new();
                    let labelled = v.fields.first().is_some_and(|(n, _)| n.is_some());
                    let mut fields = vec![];
                    for (name, t) in &v.fields {
                        if name.is_some() != labelled || name.is_some_and(|n| !labels.insert(n)) {
                            return Err(diagnostic(
                                const { RuleId::of("LANG-12") },
                                self.span,
                                "variant payload labels must be uniform and unique",
                            ));
                        }
                        fields.push((*name, self.part(*t)?));
                    }
                    variants.push(Variant {
                        name: v.name,
                        fields: fields.into(),
                    });
                }
                TypeKind::Variant(variants.into())
            }
        };
        Ok(Some(self.push(kind, Position::Field)?))
    }
    fn parameter(&self, name: &str) -> Option<(ParameterRef, ast::TypeExprId)> {
        std::iter::once((&self.parameters, &self.parameter_syntax))
            .chain(
                self.outer_parameters
                    .iter()
                    .rev()
                    .map(|(names, syntax)| (names, syntax)),
            )
            .enumerate()
            .find_map(|(depth, (names, syntax))| {
                let index = names.iter().position(|n| n.as_deref() == Some(name))?;
                Some((
                    ParameterRef::Bound {
                        depth: depth as u32,
                        parameter: ParamId(index as u32),
                    },
                    *syntax.get(index)?,
                ))
            })
    }
    fn origin(&mut self, p: &ast::Path, sibling: bool) -> Result<Origin, QueryError> {
        self.typed_origin(p, sibling, false)
            .map(|(origin, _)| origin)
    }
    /// The origin a written path names. With `typed`, also the type of the
    /// parameter or field it ends at; a definition origin has no value type.
    fn typed_origin(
        &mut self,
        p: &ast::Path,
        sibling: bool,
        typed: bool,
    ) -> Result<(Origin, Option<TypeId>), QueryError> {
        if p.segments.is_empty() {
            return Err(diagnostic(
                const { RuleId::of("LANG-6") },
                self.span,
                "empty origin path",
            ));
        }
        let name = p.segments[0].as_str().to_owned();
        let root = if sibling {
            OriginRoot::Sibling(FieldId(
                self.siblings
                    .iter()
                    .position(|s| s == &name)
                    .ok_or_else(|| {
                        diagnostic(
                            const { RuleId::of("LANG-6") },
                            self.span,
                            "unknown sibling origin",
                        )
                    })? as u32,
            ))
        } else if let Some((parameter, _)) = self.parameter(&name) {
            OriginRoot::Parameter(parameter)
        } else {
            let d = hirsel_types::constant_identity(self.definition(p, None)?, &mut |id| {
                self.snapshot(id)
            })?;
            return Ok((
                Origin {
                    root: OriginRoot::Definition(d),
                    fields: vec![].into(),
                },
                None,
            ));
        };
        let mut current = match &root {
            OriginRoot::Parameter(_) if typed || p.segments.len() > 1 => {
                let (_, syntax) = self.parameter(&name).unwrap();
                Some(self.ty(syntax, Position::Field)?)
            }
            OriginRoot::Sibling(i) => {
                let item = &self.input.syntax.item;
                if let ast::ItemKind::Type {
                    body: ast::TypeBody::Record(fs),
                    ..
                } = &item.kind
                {
                    Some(self.ty(fs[i.0 as usize].ty, Position::Field)?)
                } else {
                    None
                }
            }
            _ => None,
        };
        let mut fields = vec![];
        for s in p.segments.iter().skip(1) {
            let name = s.as_str().to_owned();
            let id = current.ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "origin has no record fields",
                )
            })?;
            let mut id = id;
            // #98: a written field place crosses the same payload as a source read.
            while let TypeKind::Box(payload) = self.types.get(id).unwrap().kind {
                fields.push(FieldId(0));
                id = payload;
            }
            let t = self.types.get(id).unwrap().clone();
            let fs = match t.kind {
                TypeKind::Record(fs) => fs,
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let sig = self.snapshot(definition)?;
                    if let SignatureKind::Type(def) = &sig.kind {
                        if let Some(r) = def.representation {
                            if let Some(Type {
                                kind: TypeKind::Record(fs),
                                ..
                            }) = self.types.get(r)
                            {
                                let fs = fs.clone();
                                let i = fs
                                    .iter()
                                    .position(|f| f.name.as_str() == name)
                                    .ok_or_else(|| {
                                        diagnostic(
                                            const { RuleId::of("LANG-6") },
                                            self.span,
                                            "unknown origin field",
                                        )
                                    })?;
                                fields.push(FieldId(i as u32));
                                let imported = self.import(fs[i].ty, &arguments)?;
                                current = Some(imported);
                                continue;
                            }
                        }
                    }
                    return Err(diagnostic(
                        const { RuleId::of("LANG-6") },
                        self.span,
                        "origin is not a record",
                    ));
                }
                _ => {
                    return Err(diagnostic(
                        const { RuleId::of("LANG-6") },
                        self.span,
                        "origin is not a record",
                    ));
                }
            };
            let i = fs
                .iter()
                .position(|f| f.name.as_str() == name)
                .ok_or_else(|| {
                    diagnostic(
                        const { RuleId::of("LANG-6") },
                        self.span,
                        "unknown origin field",
                    )
                })?;
            fields.push(FieldId(i as u32));
            current = Some(fs[i].ty);
        }
        Ok((
            Origin {
                root,
                fields: fields.into(),
            },
            current,
        ))
    }
    /// Lower a qualified type's modifiers, brand and relation (LANG-7, MODE-1,
    /// PROV-1, PROV-4). A relation place names this callable's parameters
    /// (and their field paths, #77), an arena, `heap` or `env`.
    fn qualify(
        &mut self,
        t: &mut Type,
        modifiers: &[ast::TypeModifier],
        brand: Option<&ast::BrandSyntax>,
        relation: Option<&ast::RelationSyntax>,
        pos: Position,
    ) -> Result<(), QueryError> {
        let mut seen = BTreeSet::new();
        for m in modifiers {
            if !seen.insert(*m) {
                return Err(diagnostic(
                    const { RuleId::of("LANG-7") },
                    self.span,
                    "a modifier is written once: repeat it nowhere",
                ));
            }
            match m {
                ast::TypeModifier::Unique => t.modes.uniqueness = Uniqueness::Unique,
                ast::TypeModifier::Once => {
                    // MODE-3 describes function values. Until spec #40 defines
                    // conditional generic multiplicity, require a known callable.
                    if !matches!(t.kind, TypeKind::Function(_)) {
                        return Err(diagnostic(
                            const { RuleId::of("MODE-3") },
                            self.span,
                            "once requires a function type",
                        ));
                    }
                    t.modes.multiplicity = Multiplicity::Once;
                }
                ast::TypeModifier::Sendable => {
                    t.sendability_requirement = SendabilityRequirement::Sendable;
                }
            }
        }
        if let Some(b) = brand {
            t.brand = Brand {
                alternatives: vec![self.origin(&b.path, b.sibling)?].into(),
            }
        }
        if let Some(r) = relation {
            t.modes.relation = self.relation(&t.modes.relation, r, pos)?;
        }
        t.canonicalize_shared_uniqueness();
        let id = self.types.intern(t.clone());
        if self.satisfies(id, Bound::Plain) {
            t.canonicalize_plain();
        }
        let normalized = self.types.intern(t.clone());
        t.modes.sendability = self.sendability(normalized)?;
        // Declared receiving positions are body-entry assumptions: callers must
        // independently prove receipt before this evidence is available.
        if t.sendability_requirement == SendabilityRequirement::Sendable {
            t.modes.sendability = Sendability::Sendable;
        }
        Ok(())
    }
    /// A written relation over a position's default (MODE-1). Arena places
    /// set where fresh blocks go (PROV-4); other places bound the lifetime.
    /// A borrowed or mut parameter keeps its own root (S2), a taken parameter
    /// naming itself is local to its own argument, and `heap` is the empty
    /// relation.
    fn relation(
        &mut self,
        default: &Relation,
        r: &ast::RelationSyntax,
        pos: Position,
    ) -> Result<Relation, QueryError> {
        let span = r.origin.span;
        let mut lifetime = Relation::global();
        let mut placement = Relation::global();
        for place in &r.places {
            match place {
                ast::RelationPlace::Heap => {}
                ast::RelationPlace::Environment => {
                    // A part's relation is LANG-7 where the part is used.
                    if matches!(pos, Position::Borrow | Position::Take) {
                        return Err(diagnostic(
                            const { RuleId::of("MODE-1") },
                            span,
                            "`env` is a closure or function type's environment: a parameter is not related to it",
                        ));
                    }
                    lifetime = lifetime.union(&Relation::environment());
                }
                ast::RelationPlace::Path(p) => {
                    let parameter = (p.segments.len() == 1)
                        .then(|| self.path_name(p).map(str::to_owned))
                        .flatten()
                        .and_then(|name| self.parameter(&name));
                    let (origin, is_arena) = match parameter {
                        Some((parameter, syntax)) => {
                            // A parameter named in its own relation is its own
                            // root: `take x: T in x` is local to its argument,
                            // and a borrowed `x: T in x` adds nothing (S2).
                            if matches!(pos, Position::Take | Position::Borrow)
                                && self.current_parameter.is_some_and(|i| {
                                    parameter == ParameterRef::bound(ParamId(i as u32))
                                })
                            {
                                if matches!(pos, Position::Take) {
                                    lifetime = lifetime.union(&Relation::environment());
                                }
                                continue;
                            }
                            (
                                Origin {
                                    root: OriginRoot::Parameter(parameter),
                                    fields: vec![].into(),
                                },
                                self.arena_syntax(syntax),
                            )
                        }
                        None => {
                            let (origin, _) = self.typed_origin(p, false, false)?;
                            (origin, false)
                        }
                    };
                    if is_arena {
                        self.arena_places.insert(origin.clone());
                        placement = placement.union(&Relation::place(origin));
                    } else if matches!(origin.root, OriginRoot::Parameter(_)) {
                        lifetime = lifetime.union(&Relation::place(origin));
                    } else {
                        return Err(diagnostic(
                            const { RuleId::of("MODE-1") },
                            span,
                            "a relation names parameters, arenas, `heap` or `env`",
                        ));
                    }
                }
            }
        }
        // PROV-4: fresh blocks have one destination.
        if placement.reaches().len() > 1 {
            return Err(diagnostic(
                const { RuleId::of("PROV-4") },
                span,
                "a relation names at most one arena directly",
            ));
        }
        Ok(match pos {
            // S2: a written relation adds to a borrowed parameter's own root.
            Position::Borrow => self.lifetime(default).union(&lifetime).union(&placement),
            _ => lifetime.union(&placement),
        })
    }
    /// Whether a parameter's written type is the registry's Arena (PROV-4),
    /// judged on its syntax so a relation naming it never lowers it again.
    fn arena_syntax(&mut self, mut id: ast::TypeExprId) -> bool {
        while let ast::TypeExprKind::Qualified { base, .. } =
            &self.input.syntax.tree.types[id.0 as usize].kind
        {
            id = *base;
        }
        let ast::TypeExprKind::Named { path, arguments } =
            &self.input.syntax.tree.types[id.0 as usize].kind
        else {
            return false;
        };
        let Some(heap) = self.input.metadata.heap.clone() else {
            return false;
        };
        if !arguments.is_empty()
            || self
                .path_name(path)
                .is_some_and(|n| self.generic(n).is_some())
        {
            return false;
        }
        let path = path.clone();
        self.definition(&path, Some(id))
            .is_ok_and(|d| d == heap.arena)
    }
    fn authority(&mut self, ps: &[ast::Path]) -> Result<AuthoritySummary, QueryError> {
        let mut terms = BTreeSet::new();
        for p in ps {
            if let Some(name) = self.path_name(p) {
                if let Some((parameter, _)) = self.parameter(name) {
                    terms.insert(AuthorityTerm::Parameter(parameter));
                    continue;
                }
                if let Some((depth, i, GenericParameter::Type { .. })) = self.generic(name) {
                    terms.insert(AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                        depth,
                        parameter: TypeParamId(i),
                    }));
                    continue;
                }
            }
            let d = self.definition(p, None)?;
            let item = &self.input.syntax.item;
            if d == item.definition
                && item
                    .attributes
                    .iter()
                    .any(|a| a.name.as_str() == "capability")
            {
                // The nominal identity is fixed by the declaration before its
                // implementation ceiling is checked. Reading its unpublished
                // snapshot here would introduce a false signature cycle.
                terms.insert(AuthorityTerm::Capability(d));
                continue;
            }
            let signature = self.snapshot(&d)?;
            if self.input.syntax.item.visibility != ast::Visibility::Private
                && signature.metadata.visibility == ast::Visibility::Private
                && !self
                    .input
                    .syntax
                    .item
                    .attributes
                    .iter()
                    .any(|a| a.name.as_str() == "capability")
            {
                return Err(diagnostic(
                    const { RuleId::of("PROG-2") },
                    self.span,
                    "public authority summary mentions a private type",
                ));
            }
            terms.insert(AuthorityTerm::Capability(d));
        }
        Ok(AuthoritySummary::Known(terms.into_iter().collect()))
    }
    /// `@captures(fields)` on a type: the selected representation fields
    /// (record field ordinals; the one field of a distinct type) whose erased
    /// authority each value carries (CAP-2, CLO-4). None for an ordinary type.
    fn erased_carrier(
        &self,
        item: &ast::Item,
        body: &ast::TypeBody,
        attrs: &Attributes,
    ) -> Result<Option<Vec<usize>>, QueryError> {
        let Some(attribute) = item
            .attributes
            .iter()
            .find(|a| a.name.as_str() == "captures")
        else {
            return Ok(None);
        };
        let [ast::AttributeArgument::StatePromise(promise)] = attribute.arguments.as_slice() else {
            unreachable!("attributes() checked the selector list")
        };
        let ast::ItemKind::Type { flavor, .. } = &item.kind else {
            unreachable!("a type declaration")
        };
        let reject = |message| {
            Err(diagnostic(
                const { RuleId::of("CLO-4") },
                attribute.origin.span,
                message,
            ))
        };
        if attrs.capability.is_some()
            || attrs.serializable
            || *flavor == ast::TypeFlavor::Resource
            || *flavor == ast::TypeFlavor::Flat
            || matches!(body, ast::TypeBody::Distinct { fact: Some(_), .. })
        {
            return reject(
                "an erased carrier cannot be a capability, resource, flat, serializable or refined type",
            );
        }
        let names: Vec<&str> = match body {
            ast::TypeBody::Record(fields) => fields.iter().map(|f| f.name.as_str()).collect(),
            ast::TypeBody::Distinct { field, .. } => vec![field.name.as_str()],
            _ => {
                return reject(
                    "@captures on a type selects fields of a record or distinct representation",
                );
            }
        };
        let mut selected = Vec::new();
        for path in &promise.selectors {
            let ordinal = match (path.package.as_ref(), path.segments.as_slice()) {
                (None, [name]) => names.iter().position(|n| *n == name.as_str()),
                _ => None,
            };
            let Some(ordinal) = ordinal else {
                return reject("@captures on a type names its own representation fields");
            };
            selected.push(ordinal);
        }
        selected.sort();
        selected.dedup();
        Ok(Some(selected))
    }
    fn erased_carrier_type(&self, t: TypeId) -> bool {
        let Some(TypeKind::Named { definition, .. }) = self.types.get(t).map(|row| &row.kind)
        else {
            return false;
        };
        self.snapshots
            .get(&definition)
            .is_some_and(|s| matches!(&s.kind, SignatureKind::Type(d) if d.captures.is_some()))
    }
    fn capability(&self, t: TypeId) -> Option<CapabilityKind> {
        if let TypeKind::Named { definition, .. } = &self.types.get(t)?.kind {
            if let SignatureKind::Type(d) = &self.snapshots.get(&definition)?.kind {
                return d.capability;
            }
        }
        None
    }
    fn type_authority(&self, t: Option<TypeId>) -> AuthoritySummary {
        let Some(root) = t else {
            return AuthoritySummary::inert();
        };
        let item = &self.input.syntax.item;
        if item
            .attributes
            .iter()
            .any(|a| a.name.as_str() == "capability")
        {
            return self.authority_proof(
                root,
                &AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(item.definition.clone())].into(),
                ),
            );
        }
        // A recursive declaration has no published snapshot yet. Compute its
        // field summary as a least fixed point, then use the same declaration
        // proof as consumers. Actual recursive arguments can add constant caps.
        let mut own = AuthoritySummary::inert();
        if let Some(representation) = self.own_representation {
            // Terms range over the finite declared graph; this monotone join
            // stabilizes without spending a recursive expansion budget.
            loop {
                let next = self.authority_proof(representation, &own);
                if next == own {
                    return self.authority_proof(root, &own);
                }
                own = next;
            }
        }
        self.authority_proof(root, &AuthoritySummary::Unknown)
    }
    /// A recursive group's authority is one least fixed point over its
    /// members' representations, as a lone recursive declaration's is over
    /// its own (`type_authority`): every member starts inert and joins what
    /// its fields reach, until no member's summary grows. The members then
    /// carry their settled summaries, and this declaration's own fixed point
    /// over them reaches the same joint solution.
    fn settle_group_authority(&mut self) {
        if self.group.is_empty() {
            return;
        }
        let Some(own_root) = self.own_representation else {
            return;
        };
        let roles = self.roles();
        let mut own = AuthoritySummary::inert();
        let mut summaries: BTreeMap<DefId, AuthoritySummary> = self
            .group
            .keys()
            .map(|d| (*d, AuthoritySummary::inert()))
            .collect();
        loop {
            self.install_group_authority(&summaries);
            let mut next: BTreeMap<_, _> = BTreeMap::new();
            for (definition, member) in &self.group {
                let SignatureKind::Type(declaration) = &member.kind else {
                    continue;
                };
                let Some(root) = declaration.representation else {
                    next.insert(*definition, AuthoritySummary::Unknown);
                    continue;
                };
                let generics = GenericDeclarations::signature(&declaration.generics);
                let summary =
                    structural_authority(self.types, root, &roles, &generics, &mut |_, path| {
                        Ok::<_, hirsel_types::QueryError>(match path {
                            Some(path) if *path.path() == self.own_definition() => {
                                Some(self.own_authority_view(&own))
                            }
                            Some(path) => self.snapshots.get(&path).cloned(),
                            None => None,
                        })
                    })
                    .unwrap();
                next.insert(*definition, summary);
            }
            let next_own = self.authority_proof(own_root, &own);
            if next == summaries && next_own == own {
                return;
            }
            summaries = next;
            own = next_own;
        }
    }
    /// Loads, and depends on, the declarations the group members'
    /// representations name outside the group, which this declaration's
    /// proofs read through them. One that cannot be read stays absent and
    /// the proofs through it conservative.
    fn load_group_nominals(&mut self) -> Result<(), QueryError> {
        let own = self.own_definition();
        let members: Vec<SignatureRef> = self.group.values().cloned().collect();
        for member in members {
            let SignatureKind::Type(declaration) = &member.kind else {
                continue;
            };
            let mut pending: Vec<TypeId> = declaration.representation.into_iter().collect();
            let mut seen = IdSet::default();
            while let Some(id) = pending.pop() {
                if !seen.insert(id) {
                    continue;
                }
                let Some(row) = self.types.get(id) else {
                    continue;
                };
                pending.extend(row.kind.value_parts());
                if let TypeKind::Named { definition, .. } = &row.kind
                    && *definition.path() != own
                    && !self.group.contains_key(&definition)
                {
                    if let Err(e @ QueryError::Operational(_)) = self.snapshot(*definition) {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }
    /// Gives every group member its current authority summary.
    fn install_group_authority(&mut self, summaries: &BTreeMap<DefId, AuthoritySummary>) {
        for (definition, summary) in summaries {
            let member = self.group.get_mut(definition).unwrap();
            if let SignatureKind::Type(declaration) = &member.kind
                && declaration.authority == *summary
            {
                continue;
            }
            let mut updated = (**member).clone();
            if let SignatureKind::Type(declaration) = &mut updated.kind {
                declaration.authority = summary.clone();
            }
            *member = std::sync::Arc::new(updated);
            self.snapshots.insert(*definition, member.clone());
        }
    }
    fn roles(&self) -> NominalRoles {
        NominalRoles {
            unsafe_capability: self.input.metadata.unsafe_capability.clone(),
            result_type: self.input.metadata.result_type.clone(),
            tasks_capability: self.input.metadata.tasks_capability.clone(),
            scope_capability: self.input.metadata.scope_capability.clone(),
            heap: self.input.metadata.heap.clone(),
        }
    }
    fn own_definition(&self) -> ast::DefPath {
        self.input.syntax.item.definition.clone()
    }
    /// This declaration as its recursive references read it while its own
    /// authority summary is still a fixed-point estimate.
    fn own_authority_view(&self, own: &AuthoritySummary) -> SignatureRef {
        std::sync::Arc::new(Signature {
            metadata: SignatureMetadata::default(),
            definition: DefId::intern(&self.own_definition()),
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: Allocation::MayAllocate,
                generics: self.generics.clone(),
                representation: None,
                bounds: vec![],
                authority: own.clone(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Layout::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
            }),
            fingerprint: Default::default(),
        })
    }
    /// Record a builtin occurrence's declaration; an undeclared occurrence
    /// poisons its row, whatever order the occurrences come in.
    fn builtin_declaration(&mut self, id: TypeId, declaration: Option<SignatureRef>) {
        match declaration {
            Some(d) => {
                self.builtin_declarations.entry(id).or_insert(Some(d));
            }
            None => {
                self.builtin_declarations.insert(id, None);
            }
        }
    }
    fn authority_proof(&self, root: TypeId, own: &AuthoritySummary) -> AuthoritySummary {
        let item = &self.input.syntax.item;
        let roles = self.roles();
        structural_authority(
            &self.types,
            root,
            &roles,
            &self.declarations(),
            &mut |id, path| {
                Ok::<_, hirsel_types::QueryError>(match path {
                    Some(path)
                        if path.path() == &item.definition
                            && matches!(item.kind, ast::ItemKind::Type { .. }) =>
                    {
                        Some(self.own_authority_view(own))
                    }
                    Some(path) => self.snapshots.get(&path).cloned(),
                    None => self.builtin_declarations.get(&id).cloned().flatten(),
                })
            },
        )
        .unwrap()
    }
    fn structural_bounds(
        &self,
        t: Option<TypeId>,
        flavor: ast::TypeFlavor,
        capability: Option<CapabilityKind>,
    ) -> Vec<Bound> {
        [
            Bound::Plain,
            Bound::Flat,
            Bound::Data,
            Bound::Key,
            Bound::Sendable,
            Bound::Inert,
            Bound::Copyable,
        ]
        .into_iter()
        .filter(|b| {
            if capability.is_some()
                && (matches!(b, Bound::Inert | Bound::Flat | Bound::Data | Bound::Key)
                    || capability == Some(CapabilityKind::Scoped)
                        && matches!(b, Bound::Plain | Bound::Copyable | Bound::Sendable))
            {
                return false;
            }
            if flavor == ast::TypeFlavor::Resource
                && matches!(
                    b,
                    Bound::Plain | Bound::Copyable | Bound::Data | Bound::Key | Bound::Flat
                )
            {
                return false;
            }
            if flavor == ast::TypeFlavor::Flat {
                if *b == Bound::Plain {
                    return false;
                }
                if *b == Bound::Flat || *b == Bound::Sendable {
                    return t.is_some_and(|t| self.flat_storage(t, &mut IdSet::default()));
                }
            }
            t.is_some_and(|t| self.satisfies(t, *b))
        })
        .collect()
    }
    fn scoped_representation(&self, root: TypeId) -> bool {
        let AuthoritySummary::Known(terms) = self.type_authority(Some(root)) else {
            return true;
        };
        if terms.iter().any(|term| match term {
            AuthorityTerm::Capability(path) => self.snapshots.get(&DefId::intern(path)).is_some_and(|s|
                matches!(&s.kind, SignatureKind::Type(d) if d.capability == Some(CapabilityKind::Scoped))),
            _ => false,
        }) {
            return true;
        }
        let mut pending = vec![root];
        let mut seen = IdSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let row = self.types.get(id).unwrap();
            if !row.modes.relation.is_global() {
                return true;
            }
            if let TypeKind::Parameter(p) = &row.kind {
                let bounds = self.declarations().bounds(*p).to_vec();
                if !bounds.contains(&Bound::Plain) && !bounds.contains(&Bound::Sendable) {
                    return true;
                }
            }
            // Pointer and shape actuals describe addresses/types, not retained values.
            if matches!(
                row.kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Ptr
                        | BuiltinType::FnPtr
                        | BuiltinType::SlotId
                        | BuiltinType::TypeShape,
                    ..
                }
            ) {
                continue;
            }
            pending.extend(row.kind.value_parts());
        }
        false
    }
    fn flat_storage(&self, id: TypeId, seen: &mut IdSet<TypeId>) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let result = match &self.types.get(id).unwrap().kind {
            TypeKind::Primitive(_) => true,
            TypeKind::Parameter(p) => self.declarations().get(*p).is_some_and(|g| matches!(g, GenericParameter::Type { bounds, .. } if implies(bounds, Bound::Flat))),
            TypeKind::Tuple(ts) => ts.iter().all(|t| self.flat_storage(*t, seen)),
            TypeKind::Record(fs) => fs.iter().all(|f| self.flat_storage(f.ty, seen)),
            TypeKind::Variant(vs) => vs.iter().all(|v| v.fields.iter().all(|(_,t)| self.flat_storage(*t, seen))),
            TypeKind::Named { definition, arguments } => {
                // MEM-12: "no capability/function/handle/resource". Plain
                // alone is not layout evidence (capabilities are Plain too).
                self.snapshots.get(&definition).is_some_and(|s| matches!(&s.kind,
                    SignatureKind::Type(d) if d.capability.is_none() && !d.resource))
                    && arguments.iter().all(|a| match a {
                        GenericArgument::Type(t) => self.flat_storage(*t, seen),
                        _ => true,
                    })
                    && self.satisfies(id, Bound::Flat)
            },
            TypeKind::Builtin { kind, arguments } if matches!(kind, BuiltinType::List|BuiltinType::Inline|BuiltinType::Simd|BuiltinType::Packed) => arguments.iter().all(|a| match a { GenericArgument::Type(t) => self.flat_storage(*t, seen), _ => true }),
            _ => false,
        };
        seen.remove(&id);
        result
    }
    fn satisfies(&self, t: TypeId, b: Bound) -> bool {
        if b == Bound::Serializable {
            return self
                .serializable(t)
                .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        if matches!(b, Bound::Data | Bound::Key) {
            return structural_equality(
                self.types,
                t,
                b,
                &self.declarations(),
                &mut |definition: DefId| {
                    Ok::<_, hirsel_types::QueryError>(self.snapshots.get(&definition).cloned())
                },
            )
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        if b == Bound::Inert {
            return self.type_authority(Some(t)) == AuthoritySummary::inert();
        }
        if b == Bound::NoallocDrop {
            return self.drop_summary(t) == DropAllocation::NoAlloc;
        }
        if b == Bound::Flat {
            return structural_flat(
                self.types,
                t,
                &self.declarations(),
                &mut |definition: DefId| {
                    Ok::<_, hirsel_types::QueryError>(self.snapshots.get(&definition).cloned())
                },
            )
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        let property = match b {
            Bound::Plain => StructuralProperty::Plain,
            Bound::Copyable => StructuralProperty::Copyable,
            Bound::Sendable => StructuralProperty::Sendable,
            _ => unreachable!("every other structural bound is handled above"),
        };
        structural_property(
            self.types,
            t,
            property,
            &self.declarations(),
            &mut |definition: DefId| {
                Ok::<_, hirsel_types::QueryError>(self.snapshots.get(&definition).cloned())
            },
        )
        .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    /// SER-1 for a generic opt-in's representation, every own type parameter admitted.
    fn conditionally_serializable(&self, root: TypeId) -> bool {
        let generics: Vec<_> = self
            .generics
            .iter()
            .map(|g| match g {
                GenericParameter::Type { name, bounds } => GenericParameter::Type {
                    name: name.clone(),
                    bounds: bounds
                        .iter()
                        .copied()
                        .chain([Bound::Serializable])
                        .collect(),
                },
                g => g.clone(),
            })
            .collect();
        let declarations = GenericDeclarations::from_binders(
            std::iter::once(&generics)
                .chain(self.outer_generics.iter().rev())
                .enumerate()
                .map(|(d, ps)| (GenericBinder::Bound(d as u32), ps.clone())),
        );
        self.serializable_in(root, &declarations)
            .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    fn serializable(&self, root: TypeId) -> Result<bool, QueryError> {
        self.serializable_in(root, &self.declarations())
    }
    fn serializable_in(
        &self,
        root: TypeId,
        declarations: &GenericDeclarations,
    ) -> Result<bool, QueryError> {
        let item = &self.input.syntax.item;
        structural_serializable(self.types, root, declarations, &mut |definition: DefId| {
            let path = definition.path();
            if path != &item.definition {
                return Ok(self.snapshots.get(&definition).cloned());
            }
            let ast::ItemKind::Type { flavor, body, .. } = &item.kind else {
                return Ok(None);
            };
            let opted = item
                .attributes
                .iter()
                .any(|a| a.name.as_str() == "serializable");
            Ok(Some(std::sync::Arc::new(Signature {
                definition: DefId::intern(path),
                metadata: SignatureMetadata {
                    visibility: item.visibility,
                    alias: matches!(body, ast::TypeBody::Alias(_))
                        && *flavor != ast::TypeFlavor::Distinct,
                    ..Default::default()
                },
                kind: SignatureKind::Type(TypeDefinition {
                    loop_source: None,
                    format: None,
                    generics: self.generics.clone(),
                    representation: self.own_representation,
                    bounds: vec![],
                    authority: self.type_authority(self.own_representation),
                    capability: None,
                    implementation_authority: None,
                    serializable: opted,
                    serializable_check: None,
                    layout: Layout::default(),
                    distinct: *flavor == ast::TypeFlavor::Distinct,
                    resource: *flavor == ast::TypeFlavor::Resource,
                    drop: None,
                    fact: None,
                    captures: None,
                    destructor_allocation: Allocation::MayAllocate,
                }),
                fingerprint: Default::default(),
            })))
        })
    }
    fn is_unsafe(&self, t: TypeId) -> bool {
        matches!(&self.types.get(t).unwrap().kind,TypeKind::Named{definition,..} if self.input.metadata.unsafe_capability.as_ref()==Some(definition.path()) && self.capability(t).is_some())
    }
    /// The foreign ABI class of `t`, or None outside PROG-4's types.
    fn ffi_class(&self, t: TypeId) -> Option<FfiClass> {
        match &self.types.get(t)?.kind {
            TypeKind::Primitive(Primitive::Bytes) => Some(FfiClass::Bytes),
            TypeKind::Primitive(Primitive::String | Primitive::Bool | Primitive::Never) => None,
            TypeKind::Primitive(_) => Some(FfiClass::Numeric),
            TypeKind::Tuple(ts) if ts.is_empty() => Some(FfiClass::Unit),
            TypeKind::Builtin {
                kind: BuiltinType::Ptr,
                ..
            } => Some(FfiClass::Pointer),
            TypeKind::Builtin {
                kind: BuiltinType::FnPtr,
                arguments,
            } => match arguments.as_ref() {
                [GenericArgument::Type(f)] if self.ffi_callback(*f) => Some(FfiClass::Pointer),
                _ => None,
            },
            TypeKind::Named {
                definition,
                arguments,
            } if definition.path().package_is("core", "1")
                && definition.path().0.modules.is_empty()
                && definition.path().0.name == "Option" =>
            {
                match arguments.as_ref() {
                    [GenericArgument::Type(inner)]
                        if self.ffi_class(*inner) == Some(FfiClass::Pointer) =>
                    {
                        Some(FfiClass::NullablePointer)
                    }
                    _ => None,
                }
            }
            TypeKind::Named { definition, .. } => match &self.snapshots.get(&definition)?.kind {
                SignatureKind::Type(d) if d.layout.flat => Some(FfiClass::Record),
                SignatureKind::Type(d) if d.layout.c && implies(&d.bounds, Bound::Data) => {
                    Some(FfiClass::Record)
                }
                _ => None,
            },
            _ => None,
        }
    }
    /// A callback context, `mut ctx: unique C` of any type: C receives its
    /// exclusive address and may only hand it back to a callback (PROG-4).
    /// `mut` is already exclusive, and `unique` is canonical on plain types.
    /// A byte view is never a context: `mut` of one is refused.
    fn ffi_context(&self, p: &Parameter) -> bool {
        p.passing == PassingMode::Mut
            && matches!(self.ffi_class(p.ty), None | Some(FfiClass::Record))
    }
    /// A C-callable callback: `fn(mut ctx: unique C, ...) -> R` whose other
    /// parameters are numbers and pointers C passes by value, with a foreign
    /// result and no state promises (PROG-4).
    fn ffi_callback(&self, f: TypeId) -> bool {
        let Some(TypeKind::Function(f)) = self.types.get(f).map(|t| &t.kind) else {
            return false;
        };
        let Some((ctx, rest)) = f.parameters.split_first() else {
            return false;
        };
        self.ffi_context(ctx)
            && f.generics.is_empty()
            && f.state_promises.is_empty()
            && rest.iter().all(|p| {
                p.passing != PassingMode::Mut
                    && matches!(
                        self.ffi_class(p.ty),
                        Some(FfiClass::Numeric | FfiClass::Pointer | FfiClass::NullablePointer)
                    )
            })
            && self.ffi_class(f.result).is_some_and(FfiClass::result)
    }
}
/// How a foreign parameter or result crosses the C ABI (PROG-4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FfiClass {
    /// A sized integer or float.
    Numeric,
    /// `Ptr[T]` or a callback `FnPtr`.
    Pointer,
    /// An Option of a pointer: zero is None, otherwise the pointer.
    NullablePointer,
    /// A byte view: a pointer and a length.
    Bytes,
    /// A `@layout(c)` or flat record, passed by address.
    Record,
    Unit,
}
impl FfiClass {
    /// `take` transfers only values with no counted storage; `mut` passes a
    /// writable address of storage C may fill.
    fn permits(self, passing: PassingMode) -> bool {
        match passing {
            PassingMode::Borrow => self != Self::Unit,
            PassingMode::Take => {
                matches!(self, Self::Numeric | Self::Pointer | Self::NullablePointer)
            }
            PassingMode::Mut => matches!(
                self,
                Self::Numeric | Self::Pointer | Self::NullablePointer | Self::Record
            ),
        }
    }
    fn result(self) -> bool {
        matches!(
            self,
            Self::Numeric | Self::Pointer | Self::NullablePointer | Self::Unit
        )
    }
}
fn implies(bs: &[Bound], b: Bound) -> bool {
    bs.contains(&b)
        || (b == Bound::Copyable && bs.contains(&Bound::Plain))
        || (b == Bound::Data && bs.contains(&Bound::Key))
        || (b == Bound::Inert && bounds_imply_inert(bs))
}
fn primitive(s: &str) -> Option<Primitive> {
    Some(match s {
        "bool" => Primitive::Bool,
        "u8" => Primitive::U8,
        "u16" => Primitive::U16,
        "u32" => Primitive::U32,
        "u64" => Primitive::U64,
        "u128" => Primitive::U128,
        "i8" => Primitive::I8,
        "i16" => Primitive::I16,
        "i32" => Primitive::I32,
        "i64" => Primitive::I64,
        "i128" => Primitive::I128,
        "f32" => Primitive::F32,
        "f64" => Primitive::F64,
        "char" => Primitive::Char,
        "String" => Primitive::String,
        "Bytes" => Primitive::Bytes,
        "Never" => Primitive::Never,
        _ => return None,
    })
}
fn builtin(s: &str) -> Option<BuiltinType> {
    Some(match s {
        "Array" => BuiltinType::Array,
        "List" => BuiltinType::List,
        "Map" => BuiltinType::Map,
        "Set" => BuiltinType::Set,
        "SortedMap" => BuiltinType::SortedMap,
        "Inline" => BuiltinType::Inline,
        "Simd" => BuiltinType::Simd,
        "Packed" => BuiltinType::Packed,
        "Shared" => BuiltinType::Shared,
        "Slots" => BuiltinType::Slots,
        "SlotId" => BuiltinType::SlotId,
        "Region" => BuiltinType::Region,
        "Iter" => BuiltinType::Iter,
        "Task" => BuiltinType::Task,
        "Sender" => BuiltinType::Sender,
        "Receiver" => BuiltinType::Receiver,
        "Mutex" => BuiltinType::Mutex,
        "Atomic" => BuiltinType::Atomic,
        "Ptr" => BuiltinType::Ptr,
        "FnPtr" => BuiltinType::FnPtr,
        "TypeShape" => BuiltinType::TypeShape,
        _ => return None,
    })
}

// Field markers are signature metadata; the checker owns MEM-12 growable legality.

#[cfg(test)]
mod tests;
