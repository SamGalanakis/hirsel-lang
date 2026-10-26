use crate::*;
use hirsel_intern::Symbol;
use hirsel_syntax::{Bound, ContentHash, ContractKind, DefPath, PassingMode};
use std::collections::BTreeSet;

/// A malformed hand-built semantic graph cannot have a canonical fingerprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodingError {
    BodyGeneric(SiteRef),
    InvalidSchemaParameter(TypeParameterRef),
    BodyParameter(ParameterRef),
    BodyOrigin(Origin),
    MissingType(TypeId),
    StructuralCycle(TypeId),
    RecoveryType(TypeId),
    ConflictingDependency(DefPath),
}

/// Canonical semantic bytes, excluding locations, unreachable nodes and arena IDs.
/// Every atom is u64-little-endian length framed; lists start with a framed count.
/// Sets are sorted and deduplicated; ordered declaration lists retain their order.
pub fn canonical_encoding(
    types: &TypeInterner,
    signature: &Signature,
    dependencies: &[SignatureDependency],
) -> Result<Vec<u8>, EncodingError> {
    let mut e = Encoder::default();
    encode_signature(&mut e, types, signature, dependencies)?;
    if let Some(error) = e.error {
        return Err(error);
    }
    Ok(e.bytes)
}

/// Every TypeId `signature` names directly, metadata included, in the order
/// `canonical_encoding` reaches them; repeats are kept. A walk that stops at
/// a malformed row returns the roots before it.
pub fn signature_types(types: &TypeInterner, signature: &Signature) -> Vec<TypeId> {
    let mut e = Encoder {
        roots: Some(vec![]),
        ..Encoder::default()
    };
    let _ = encode_signature(&mut e, types, signature, &[]);
    e.roots.unwrap_or_default()
}

fn encode_signature(
    e: &mut Encoder,
    types: &TypeInterner,
    signature: &Signature,
    dependencies: &[SignatureDependency],
) -> Result<(), EncodingError> {
    // No `..`: a new field must be encoded here or excluded with a reason,
    // or a changed declaration would keep its dependents' fingerprint.
    let Signature {
        metadata,
        definition,
        kind,
        // The fingerprint is this encoding's own output.
        fingerprint: _,
    } = signature;
    let SignatureMetadata {
        inferred_unique,
        visibility,
        alias,
        constant_alias,
        track_caller,
        unsafe_ffi,
        wrapper,
        roles,
        schema,
        recursive_group,
        implementations,
    } = metadata;
    let NominalRoles {
        unsafe_capability,
        result_type,
        tasks_capability,
        scope_capability,
        heap,
    } = roles;
    e.atom(b"hirsel.signature.encoding.v16");
    e.path(definition.path());
    e.tag(match visibility {
        hirsel_syntax::Visibility::Private => "private",
        hirsel_syntax::Visibility::Public => "public",
        hirsel_syntax::Visibility::PublicOpaque => "opaque",
    });
    if !inferred_unique.is_empty() {
        e.tag("private-unique-v1");
        e.number(inferred_unique.preserves_unique.len() as u64);
        for parameter in &inferred_unique.preserves_unique {
            e.number(parameter.0 as u64);
        }
        e.number(inferred_unique.results.len() as u64);
        for row in &inferred_unique.results {
            match row {
                UniqueResultRow::Root => e.tag("root"),
                UniqueResultRow::Tuple(field) => {
                    e.tag("tuple");
                    e.number(*field as u64);
                }
                UniqueResultRow::Payload { variant, field } => {
                    e.tag("payload");
                    e.number(*variant as u64);
                    e.number(*field as u64);
                }
            }
        }
    }
    e.boolean(*alias);
    if let Some(target) = constant_alias {
        e.tag("dictionary-alias-v1");
        e.path(target);
    }
    e.boolean(*track_caller);
    e.boolean(*unsafe_ffi);
    // None encodes as the retired exported-C flag's false.
    e.option(
        wrapper.as_ref(),
        |e,
         ForeignWrapper {
             kind,
             deterministic,
         }| {
            e.tag(match kind {
                WrapperKind::Pure => "pure",
                WrapperKind::Handle => "handle",
                WrapperKind::Effect => "effect",
            });
            e.boolean(*deterministic);
        },
    );
    e.option(unsafe_capability.as_ref(), |e, p| e.path(p));
    e.option(result_type.as_ref(), |e, p| e.path(p));
    e.option(tasks_capability.as_ref(), |e, p| e.path(p));
    e.option(scope_capability.as_ref(), |e, p| e.path(p));
    e.option(heap.as_ref(), |e, HeapRole { arena, operation }| {
        e.path(arena);
        e.path(operation);
    });
    // Empty schemas preserve existing canonical vectors. This framed extension
    // separates constrained signatures from every pre-schema kind tag.
    if !schema.is_empty() {
        e.tag("intrinsic-schema-v1");
        e.number(schema.len() as u64);
        for constraint in schema {
            match constraint {
                SchemaConstraint::Bound {
                    parameter,
                    bound,
                    rule,
                } => {
                    if GenericDeclarations::signature(signature.generics())
                        .get(*parameter)
                        .is_none()
                    {
                        return Err(EncodingError::InvalidSchemaParameter(*parameter));
                    }
                    e.tag("bound");
                    e.generic_reference(*parameter);
                    e.bounds(&[*bound]);
                    e.tag(rule.as_str());
                }
                SchemaConstraint::Domain {
                    parameter,
                    domain,
                    rule,
                } => {
                    e.tag("domain");
                    e.generic_reference(*parameter);
                    e.tag(match domain {
                        ScalarDomain::Integer => "integer",
                        ScalarDomain::Float => "float",
                        ScalarDomain::Scalar => "scalar",
                    });
                    e.tag(rule.as_str());
                }
                SchemaConstraint::CompleteRangeFits {
                    source,
                    destination,
                }
                | SchemaConstraint::DestinationNoWider {
                    source,
                    destination,
                } => {
                    e.tag(
                        if matches!(constraint, SchemaConstraint::CompleteRangeFits { .. }) {
                            "range-fits"
                        } else {
                            "no-wider"
                        },
                    );
                    e.generic_reference(*source);
                    e.generic_reference(*destination);
                }
                SchemaConstraint::PositiveSize(parameter) => {
                    e.tag("positive-size");
                    e.generic_reference(*parameter);
                }
                SchemaConstraint::NoallocDrop(parameter) => {
                    e.tag("noalloc-drop");
                    e.generic_reference(*parameter);
                }
            }
        }
    }
    // Outside a recursive type group this preserves existing canonical
    // vectors; a member's group key covers the members it does not list.
    if let Some(group) = recursive_group {
        e.tag("recursive-group-v1");
        e.atom(&group.0);
    }
    // Absent without binders, so existing vectors are unchanged.
    if !implementations.is_empty() {
        e.tag("implementation-binders-v1");
        e.number(implementations.len() as u64);
        for binder in implementations {
            e.atom(binder.name.as_bytes());
            e.path(binder.family.path());
            e.number(binder.members.len() as u64);
            for (member, ordinal) in &binder.members {
                e.atom(member.as_bytes());
                e.number(u64::from(*ordinal));
            }
        }
    }
    match kind {
        SignatureKind::Function(f) => {
            e.tag("function");
            e.function(f, types)?;
        }
        SignatureKind::Type(TypeDefinition {
            loop_source,
            format,
            destructor_allocation,
            generics,
            representation,
            bounds,
            authority,
            capability,
            implementation_authority,
            serializable,
            serializable_check,
            layout,
            distinct,
            resource,
            drop,
            fact,
            captures,
        }) => {
            let Layout {
                c,
                packed,
                align,
                soa,
                flat,
            } = layout;
            e.tag("type");
            e.generics(generics);
            e.boolean(loop_source.is_some());
            if let Some(LoopSourceDeclaration { open, next, option }) = loop_source {
                e.path(open);
                e.path(next);
                e.path(option);
            }
            e.boolean(format.is_some());
            if let Some(function) = format {
                e.path(function);
            }
            e.optional_type(*representation, types)?;
            e.bounds(bounds);
            e.boolean(*serializable);
            e.boolean(serializable_check.is_some());
            if let Some(SerializableCheck {
                function,
                representation,
            }) = serializable_check
            {
                e.path(function);
                e.ty(*representation, types)?;
            }
            // CAP-5 ceilings are excluded from public exports but bind private revisions.
            if *visibility == hirsel_syntax::Visibility::Private {
                e.option(implementation_authority.as_ref(), |e, ceiling| {
                    e.authority(ceiling)
                });
            }
            e.authority(authority);
            e.tag(match capability {
                None => "none",
                Some(CapabilityKind::Global) => "global",
                Some(CapabilityKind::Scoped) => "scoped",
            });
            e.boolean(*c);
            e.boolean(*packed);
            e.option(*align, |e, n| e.number(n as u64));
            e.boolean(*soa);
            e.boolean(*flat);
            e.boolean(*distinct);
            e.boolean(*resource);
            e.boolean(*destructor_allocation == Allocation::NoAlloc);
            e.option(drop.as_ref(), |e, p| e.path(p));
            e.option(fact.as_ref(), |e, c| e.contract(c));
            // Absent for ordinary declarations, so existing vectors are unchanged.
            if let Some(fields) = captures {
                e.tag("erased-carrier-v1");
                e.number(fields.len() as u64);
                for field in fields.iter() {
                    e.number(*field as u64);
                }
            }
        }
        SignatureKind::Const { ty: t, generics } => {
            e.tag("const");
            e.generics(generics);
            e.ty(*t, types)?;
        }
        SignatureKind::Family(family) => {
            e.tag("family-v1");
            e.number(family.members.len() as u64);
            for member in &family.members {
                e.atom(member.name.as_bytes());
                match &member.kind {
                    FamilyMemberKind::Type { bounds } => {
                        e.tag("type");
                        e.bounds(bounds);
                    }
                    FamilyMemberKind::Size => e.tag("size"),
                    // The header is its own declaration, a dependency.
                    FamilyMemberKind::Operation => e.tag("operation"),
                }
            }
        }
        SignatureKind::Implementation(implementation) => {
            e.tag("implementation-v1");
            e.path(implementation.family.path());
            e.number(implementation.members.len() as u64);
            for (name, actual) in &implementation.members {
                e.atom(name.as_bytes());
                match actual {
                    MemberActual::Type(ty) => {
                        e.tag("type");
                        e.ty(*ty, types)?;
                    }
                    MemberActual::Size(n) => {
                        e.tag("size");
                        e.number(*n);
                    }
                    MemberActual::Operation(target) => {
                        e.tag("operation");
                        e.path(target);
                    }
                }
            }
        }
    }
    let mut deps = dependencies.to_vec();
    deps.sort_by_key(|a| a.definition);
    for pair in deps.windows(2) {
        if pair[0].definition == pair[1].definition && pair[0].fingerprint != pair[1].fingerprint {
            return Err(EncodingError::ConflictingDependency(
                pair[0].definition.path().clone(),
            ));
        }
    }
    deps.dedup();
    e.number(deps.len() as u64);
    for d in deps {
        e.path(d.definition.path());
        e.atom(&d.fingerprint.0);
    }
    Ok(())
}

/// SHA-256 with its own domain, using the shared length-framed hash primitive.
pub fn fingerprint(
    types: &TypeInterner,
    signature: &Signature,
    dependencies: &[SignatureDependency],
) -> Result<ContentHash, EncodingError> {
    let bytes = canonical_encoding(types, signature, dependencies)?;
    Ok(ContentHash::of_parts(
        "hirsel.signature.sha256.v12",
        [bytes.as_slice()],
    ))
}

/// An opaque type's owner view (PROG-2): its public signature with the owner
/// package's declaration, whose representation rows are interned like every
/// other row. The view is the same entity seen by its own package, so it keeps
/// the public fingerprint; [`sealed`] is its inverse.
pub fn owner_view(public: &Signature, declaration: TypeDefinition) -> Signature {
    Signature {
        kind: SignatureKind::Type(declaration),
        ..public.clone()
    }
}

/// The public signature an owner view seals to: no representation (a checked
/// serializable opt-in keeps its published wire contract), no carrier field
/// ordinals and no CAP-5 implementation ceiling. The carrier
/// marker remains public. A view is consistent exactly when it seals to its
/// public signature.
pub fn sealed(view: &Signature) -> Signature {
    let mut public = view.clone();
    if let SignatureKind::Type(declaration) = &mut public.kind {
        declaration.representation = None;
        declaration.implementation_authority = None;
        declaration.captures = declaration.captures.as_ref().map(|_| Default::default());
    }
    public
}

#[derive(Default)]
pub struct Encoder {
    error: Option<EncodingError>,
    pub bytes: Vec<u8>,
    active: std::collections::HashSet<TypeId>,
    /// When recording, the ids `ty` is called with outside any row.
    roots: Option<Vec<TypeId>>,
}
impl Encoder {
    pub fn atom(&mut self, b: &[u8]) {
        self.bytes
            .extend_from_slice(&(b.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(b);
    }
    pub fn tag(&mut self, s: &str) {
        self.atom(s.as_bytes());
    }
    pub fn number(&mut self, n: u64) {
        self.atom(&n.to_le_bytes());
    }
    pub fn boolean(&mut self, b: bool) {
        self.atom(&[u8::from(b)]);
    }
    pub fn option<T>(&mut self, v: Option<T>, f: impl FnOnce(&mut Self, T)) {
        self.boolean(v.is_some());
        if let Some(v) = v {
            f(self, v);
        }
    }
    pub fn path(&mut self, p: &DefPath) {
        self.tag(&p.0.package.name);
        self.tag(&p.0.package.version);
        self.number(p.0.modules.len() as u64);
        for m in &p.0.modules {
            self.tag(m);
        }
        self.tag(&p.0.name);
    }
    fn bounds(&mut self, b: &[Bound]) {
        let b: BTreeSet<_> = b.iter().copied().collect();
        self.number(b.len() as u64);
        for b in b {
            self.tag(match b {
                Bound::Plain => "plain",
                Bound::Flat => "flat",
                Bound::Data => "data",
                Bound::Key => "key",
                Bound::Sendable => "sendable",
                Bound::Inert => "inert",
                Bound::Copyable => "copyable",
                Bound::NoallocDrop => "noalloc_drop",
                Bound::Serializable => "serializable",
            });
        }
    }
    fn generic_reference<P: GenericOrdinal>(&mut self, reference: BinderRef<P>) {
        match reference {
            BinderRef::Bound { depth, parameter } => {
                self.number(depth as u64);
                self.number(parameter.ordinal() as u64);
            }
            BinderRef::Body { scope, .. } => {
                self.error.get_or_insert(EncodingError::BodyGeneric(scope));
            }
        }
    }
    fn generics(&mut self, g: &[GenericParameter]) {
        self.number(g.len() as u64);
        for g in g {
            match g {
                GenericParameter::Type { name, bounds } => {
                    self.tag("type");
                    self.tag(name);
                    self.bounds(bounds);
                }
                GenericParameter::Size { name } => {
                    self.tag("size");
                    self.tag(name);
                }
            }
        }
    }
    pub(crate) fn authority(&mut self, a: &AuthoritySummary) {
        match a {
            AuthoritySummary::Unknown => self.tag("unknown"),
            AuthoritySummary::Known(a) => {
                self.tag("known");
                let a: BTreeSet<_> = a.iter().collect();
                self.number(a.len() as u64);
                for a in a {
                    match a {
                        AuthorityTerm::Capability(p) => {
                            self.tag("capability");
                            self.path(p);
                        }
                        AuthorityTerm::Parameter(p) => {
                            self.tag("parameter");
                            self.parameter(*p);
                        }
                        AuthorityTerm::TypeParameter(p) => {
                            self.tag("type-parameter");
                            self.generic_reference(*p);
                        }
                    }
                }
            }
        }
    }
    fn origin(&mut self, o: &Origin) {
        match &o.root {
            OriginRoot::Expression(..) => {
                self.error
                    .get_or_insert(EncodingError::BodyOrigin(o.clone()));
            }
            OriginRoot::ArgumentBrand(p) => {
                self.tag("argument-brand");
                self.parameter(*p);
            }
            OriginRoot::Parameter(p) => {
                self.tag("parameter");
                self.parameter(*p);
            }
            // A signature's locals are its own definition's: the owner is implicit.
            OriginRoot::Local(_, p) => {
                self.tag("local");
                self.number(p.0 as u64);
            }
            OriginRoot::Definition(p) => {
                self.tag("definition");
                self.path(p);
            }
            OriginRoot::Sibling(p) => {
                self.tag("sibling");
                self.number(p.0 as u64);
            }
        }
        self.number(o.fields.len() as u64);
        for f in &o.fields {
            self.number(f.0 as u64);
        }
    }
    fn parameter(&mut self, p: ParameterRef) {
        match p {
            ParameterRef::Bound { depth, parameter } => {
                self.number(depth as u64);
                self.number(parameter.0 as u64);
            }
            ParameterRef::Body { .. } => {
                self.error.get_or_insert(EncodingError::BodyParameter(p));
            }
        }
    }
    pub fn ty(&mut self, id: TypeId, s: &TypeInterner) -> Result<(), EncodingError> {
        if self.active.is_empty()
            && let Some(roots) = &mut self.roots
        {
            roots.push(id);
        }
        let t = s.get(id).ok_or(EncodingError::MissingType(id))?;
        if !self.active.insert(id) {
            return Err(EncodingError::StructuralCycle(id));
        }
        // No `..`: a new field must be encoded here or excluded with a reason.
        let Type {
            sendability_requirement,
            allocation_target,
            drop_allocation,
            kind,
            modes:
                Modes {
                    relation,
                    uniqueness,
                    multiplicity,
                    sendability,
                },
            erased_captures,
            brand,
        } = t;
        match kind {
            TypeKind::Primitive(p) => {
                self.tag("primitive");
                self.tag(match p {
                    Primitive::Bool => "bool",
                    Primitive::U8 => "u8",
                    Primitive::U16 => "u16",
                    Primitive::U32 => "u32",
                    Primitive::U64 => "u64",
                    Primitive::U128 => "u128",
                    Primitive::I8 => "i8",
                    Primitive::I16 => "i16",
                    Primitive::I32 => "i32",
                    Primitive::I64 => "i64",
                    Primitive::I128 => "i128",
                    Primitive::F32 => "f32",
                    Primitive::F64 => "f64",
                    Primitive::Char => "char",
                    Primitive::String => "String",
                    Primitive::Bytes => "Bytes",
                    Primitive::Never => "never",
                });
            }
            TypeKind::Tuple(ts) => {
                self.tag("tuple");
                self.number(ts.len() as u64);
                for t in ts {
                    self.ty(*t, s)?;
                }
            }
            TypeKind::Parameter(p) => {
                self.tag("parameter");
                self.generic_reference(*p);
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                self.tag("named");
                self.path(definition.path());
                self.arguments(arguments, s)?;
            }
            TypeKind::Builtin { kind, arguments } => {
                self.tag("builtin");
                self.tag(match kind {
                    BuiltinType::Array => "Array",
                    BuiltinType::List => "List",
                    BuiltinType::Map => "Map",
                    BuiltinType::Set => "Set",
                    BuiltinType::SortedMap => "SortedMap",
                    BuiltinType::Inline => "Inline",
                    BuiltinType::Simd => "Simd",
                    BuiltinType::Packed => "Packed",
                    BuiltinType::Shared => "Shared",
                    BuiltinType::Slots => "Slots",
                    BuiltinType::SlotId => "SlotId",
                    BuiltinType::Region => "Region",
                    BuiltinType::Iter => "Iter",
                    BuiltinType::Task => "Task",
                    BuiltinType::Sender => "Sender",
                    BuiltinType::Receiver => "Receiver",
                    BuiltinType::Mutex => "Mutex",
                    BuiltinType::Atomic => "Atomic",
                    BuiltinType::Ptr => "Ptr",
                    BuiltinType::FnPtr => "FnPtr",
                    BuiltinType::TypeShape => "TypeShape",
                });
                self.arguments(arguments, s)?;
            }
            TypeKind::Box(t) => {
                self.tag("box");
                self.ty(*t, s)?;
            }
            TypeKind::Record(fs) => {
                self.tag("record");
                self.number(fs.len() as u64);
                for f in fs {
                    self.tag(f.name.as_str());
                    self.boolean(f.growable);
                    self.ty(f.ty, s)?;
                }
            }
            TypeKind::Variant(vs) => {
                self.tag("variant");
                self.number(vs.len() as u64);
                for v in vs {
                    self.tag(v.name.as_str());
                    self.number(v.fields.len() as u64);
                    for (n, t) in &v.fields {
                        self.option(n.map(Symbol::as_str), |e, n| e.tag(n));
                        self.ty(*t, s)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                self.tag("function");
                self.function(f, s)?;
            }
            TypeKind::Error => return Err(EncodingError::RecoveryType(id)),
        }
        self.tag("relation");
        self.number(relation.reaches().len() as u64);
        for reach in relation.reaches() {
            match reach {
                Reach::Place(o) => {
                    self.tag("place");
                    self.origin(o);
                }
                Reach::Environment => self.tag("env"),
                Reach::Region(r) => {
                    self.tag("region");
                    self.number(r.0 as u64);
                }
            }
        }
        self.tag(match uniqueness {
            Uniqueness::Aliased => "aliased",
            Uniqueness::Unique => "unique",
        });
        self.tag(match multiplicity {
            Multiplicity::Many => "many",
            Multiplicity::Once => "once",
        });
        self.tag(match sendability_requirement {
            SendabilityRequirement::None => "crossing-unrequired",
            SendabilityRequirement::Sendable => "crossing-required",
        });
        match sendability {
            Sendability::Sendable => self.tag("sendable"),
            Sendability::NotSendable => self.tag("not-sendable"),
            Sendability::DependsOn(ps) => {
                self.tag("depends");
                let ps: BTreeSet<_> = ps.iter().collect();
                self.number(ps.len() as u64);
                for p in ps {
                    self.generic_reference(*p);
                }
            }
        }
        match drop_allocation {
            DropAllocation::NoAlloc => self.tag("drop-noalloc"),
            DropAllocation::MayAllocate => self.tag("drop-may-allocate"),
            DropAllocation::DependsOn(ps) => {
                self.tag("drop-depends");
                let ps: BTreeSet<_> = ps.iter().collect();
                self.number(ps.len() as u64);
                for p in ps {
                    self.generic_reference(*p);
                }
            }
        }
        self.boolean(*allocation_target == Some(AllocationTarget::Heap));
        match erased_captures {
            None | Some(ErasedCaptures::Known(AuthoritySummary::Unknown)) => {
                self.tag("captures-unknown")
            }
            Some(ErasedCaptures::Known(a)) => {
                self.tag("captures-known");
                self.authority(a);
            }
            Some(ErasedCaptures::FromArguments(ps)) => {
                self.tag("captures-from");
                let selectors: BTreeSet<_> = ps.iter().collect();
                self.number(selectors.len() as u64);
                for selector in selectors {
                    self.parameter(selector.parameter);
                    self.number(selector.fields.len() as u64);
                    for field in &selector.fields {
                        self.number(field.0 as u64);
                    }
                }
            }
        }
        let bs: BTreeSet<_> = brand.alternatives.iter().collect();
        self.number(bs.len() as u64);
        for b in bs {
            self.origin(b);
        }
        self.active.remove(&id);
        Ok(())
    }
    fn arguments(&mut self, a: &[GenericArgument], s: &TypeInterner) -> Result<(), EncodingError> {
        self.number(a.len() as u64);
        for a in a {
            match a {
                GenericArgument::Type(t) => {
                    self.tag("type");
                    self.ty(*t, s)?;
                }
                GenericArgument::Size(Size::Constant(n)) => {
                    self.tag("constant");
                    self.number(*n);
                }
                GenericArgument::Size(Size::Parameter(p)) => {
                    self.tag("size-parameter");
                    self.generic_reference(*p);
                }
            }
        }
        Ok(())
    }
    fn optional_type(&mut self, t: Option<TypeId>, s: &TypeInterner) -> Result<(), EncodingError> {
        self.boolean(t.is_some());
        if let Some(t) = t {
            self.ty(t, s)?;
        }
        Ok(())
    }
    fn contract(&mut self, c: &Contract) {
        self.tag(match c.kind {
            ContractKind::Requires => "requires",
            ContractKind::Ensures => "ensures",
        });
        self.number(c.ordinal as u64);
        self.atom(&c.code_hash.0);
    }
    pub fn function(&mut self, f: &FunctionType, s: &TypeInterner) -> Result<(), EncodingError> {
        // No `..`: a new field must be encoded here or excluded with a reason.
        let FunctionType {
            state_promises,
            generics,
            parameters,
            result,
            error,
            allocation,
            authority,
            uses,
            contracts,
            incomplete,
            foreign,
        } = f;
        self.generics(generics);
        self.number(state_promises.len() as u64);
        for StatePromise {
            selector,
            advance_only,
            preserve_captures,
            preserve_unique,
            unretained,
        } in state_promises
        {
            self.parameter(selector.parameter);
            self.number(selector.fields.len() as u64);
            for field in &selector.fields {
                self.number(field.0 as u64);
            }
            self.boolean(*advance_only);
            self.boolean(*preserve_captures);
            self.boolean(*preserve_unique);
            self.number(unretained.len() as u64);
            for parameter in unretained.iter() {
                self.number(parameter.0 as u64);
            }
        }
        self.number(parameters.len() as u64);
        for Parameter {
            name,
            label,
            passing,
            ty,
        } in parameters
        {
            self.option(name.as_deref(), |e, n| e.tag(n));
            self.option(label.as_deref(), |e, n| e.tag(n));
            self.tag(match passing {
                PassingMode::Borrow => "borrow",
                PassingMode::Take => "take",
                PassingMode::Mut => "mut",
            });
            self.ty(*ty, s)?;
        }
        self.ty(*result, s)?;
        self.optional_type(*error, s)?;
        self.tag(match allocation {
            Allocation::MayAllocate => "may-allocate",
            Allocation::NoAlloc => "noalloc",
        });
        self.authority(authority);
        match uses {
            Uses::Unrestricted => self.tag("unrestricted"),
            Uses::Only(a) => {
                self.tag("only");
                self.authority(a);
            }
        }
        self.number(contracts.len() as u64);
        for c in contracts {
            self.contract(c);
        }
        self.boolean(*incomplete);
        self.boolean(foreign.is_some());
        if let Some(ForeignContract {
            abi,
            blocking,
            aliasing_contract,
        }) = foreign
        {
            self.tag(abi);
            self.boolean(*blocking);
            self.tag(aliasing_contract);
        }
        Ok(())
    }
}

/// Canonical rooted type bytes, independent of its arena and declaration wrapper.
pub fn canonical_type_encoding(
    store: &TypeInterner,
    root: TypeId,
) -> Result<Vec<u8>, EncodingError> {
    let mut encoder = Encoder::default();
    encoder.tag("hirsel.type.encoding.v2");
    encoder.ty(root, store)?;
    if let Some(error) = encoder.error {
        return Err(error);
    }
    Ok(encoder.bytes)
}

/// The original substituted source TYPE identity used by closed storage.
/// Native function types erase axes this retains. This identifies a rooted
/// type, not its expanded payload graph or the contents of an environment.
pub fn closed_storage_source_type(
    store: &TypeInterner,
    root: TypeId,
) -> Result<ContentHash, EncodingError> {
    let bytes = canonical_type_encoding(store, root)?;
    Ok(ContentHash::of_parts(
        "hirsel.closed-storage-source-type.v1",
        [bytes.as_slice()],
    ))
}

/// Canonical specialization arguments use the same rooted type renderer.
pub fn canonical_argument_encoding(
    store: &TypeInterner,
    argument: &GenericArgument,
) -> Result<Vec<u8>, EncodingError> {
    let mut encoder = Encoder::default();
    encoder.arguments(std::slice::from_ref(argument), store)?;
    if let Some(error) = encoder.error {
        return Err(error);
    }
    Ok(encoder.bytes)
}
