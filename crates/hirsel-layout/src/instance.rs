//! Specialized TIR instance identity and views for native import.
use hirsel_core_contract::Passing;
use hirsel_lir::ir::{CaptureUse, SchemaError};
use hirsel_syntax::{ContentHash, DefPath};

/// The binder of one generic actual: its declaring scope and ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ActualBinder {
    Type(hirsel_types::TypeParameterRef),
    Size(hirsel_types::SizeParameterRef),
}

/// The declared callable convention of an instance (borrow/take/mut, once/many).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallableConvention {
    pub parameters: Vec<Passing>,
    pub multiplicity: hirsel_lir::abi_types::CallMultiplicity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceKeyError {
    /// A type actual whose kind does not match its binder, or vice versa.
    KindMismatch(u32),
    /// A body-scoped parameter or origin: the actual is not complete.
    Incomplete,
    MissingType,
    StructuralCycle,
    RecoveryType,
    /// Malformed framing or an encoding for another definition/body.
    Encoding,
}

/// Complete identity of one native instance.
///
/// Canonicalization keeps the definition, the body's index within it, each
/// actual's binder and complete type structure, size actuals, the declared
/// callable convention and the environment layout. It erases every actual's
/// relation, brand, uniqueness and sendability at every depth. The
/// definition's implementation revision is not identity: it travels as
/// `UnitHeader::implementation_revision`, so a body edit leaves every
/// caller's call and symbol unchanged. Equality compares the full encoding;
/// hashing uses the digest.
#[derive(Clone, Debug)]
pub struct InstanceKey {
    definition: DefPath,
    body: u32,
    encoding: Vec<u8>,
    digest: ContentHash,
}

struct InstanceAtoms<'a>(&'a [u8]);
impl<'a> InstanceAtoms<'a> {
    fn next(&mut self) -> Result<&'a [u8], InstanceKeyError> {
        let (prefix, rest) = self
            .0
            .split_at_checked(8)
            .ok_or(InstanceKeyError::Encoding)?;
        let len = usize::try_from(u64::from_le_bytes(prefix.try_into().unwrap()))
            .map_err(|_| InstanceKeyError::Encoding)?;
        let (atom, rest) = rest
            .split_at_checked(len)
            .ok_or(InstanceKeyError::Encoding)?;
        self.0 = rest;
        Ok(atom)
    }
    fn number(&mut self) -> Result<u64, InstanceKeyError> {
        Ok(u64::from_le_bytes(
            self.next()?
                .try_into()
                .map_err(|_| InstanceKeyError::Encoding)?,
        ))
    }
}

impl PartialEq for InstanceKey {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest && self.encoding == other.encoding
    }
}
impl Eq for InstanceKey {}
impl std::hash::Hash for InstanceKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.digest.hash(state);
    }
}

impl InstanceKey {
    /// Canonicalize an instance. `actuals` pairs each binder with its actual,
    /// whose TypeIds are rows of `types`. `environment` is the digest of the
    /// closure environment layout for a concrete closure specialization.
    pub fn new(
        definition: &DefPath,
        body: u32,
        actuals: &[(ActualBinder, hirsel_types::GenericArgument)],
        types: &hirsel_types::TypeInterner,
        convention: Option<&CallableConvention>,
        environment: Option<ContentHash>,
    ) -> Result<Self, InstanceKeyError> {
        use hirsel_types::{GenericArgument, Size};
        let mut e = KeyEncoder {
            bytes: Vec::new(),
            types,
            active: Vec::new(),
        };
        e.tag("hirsel.instance-key.v3");
        e.path(definition);
        e.number(u64::from(body));
        e.number(actuals.len() as u64);
        for (ordinal, (binder, actual)) in actuals.iter().enumerate() {
            match (binder, actual) {
                (ActualBinder::Type(binder), GenericArgument::Type(ty)) => {
                    e.tag("type");
                    e.binder(*binder)?;
                    e.ty(*ty)?;
                }
                (ActualBinder::Size(binder), GenericArgument::Size(size)) => {
                    e.tag("size");
                    e.binder(*binder)?;
                    match size {
                        Size::Constant(n) => e.number(*n),
                        Size::Parameter(parameter) => {
                            e.tag("size-parameter");
                            e.binder(*parameter)?;
                        }
                    }
                }
                _ => return Err(InstanceKeyError::KindMismatch(ordinal as u32)),
            }
        }
        e.boolean(convention.is_some());
        if let Some(convention) = convention {
            e.number(convention.parameters.len() as u64);
            for passing in &convention.parameters {
                e.tag(match passing {
                    Passing::Borrow => "borrow",
                    Passing::Take => "take",
                    Passing::Mut => "mut",
                });
            }
            e.tag(match convention.multiplicity {
                hirsel_lir::abi_types::CallMultiplicity::Many => "many",
                hirsel_lir::abi_types::CallMultiplicity::Once => "once",
            });
        }
        e.boolean(environment.is_some());
        if let Some(environment) = environment {
            e.atom(&environment.0);
        }
        let digest = ContentHash::of_parts("hirsel.instance-key.sha256.v3", [e.bytes.as_slice()]);
        Ok(Self {
            definition: definition.clone(),
            body,
            encoding: e.bytes,
            digest,
        })
    }
    pub fn definition(&self) -> &DefPath {
        &self.definition
    }
    pub fn body(&self) -> u32 {
        self.body
    }
    pub fn encoding(&self) -> &[u8] {
        &self.encoding
    }
    /// Owned encoding buffer retained by this key, excluding its inline fields
    /// and the definition path. The caller accounts for those separately.
    pub fn retained_encoding_bytes(&self) -> usize {
        self.encoding.capacity()
    }
    pub fn digest(&self) -> ContentHash {
        self.digest
    }
    /// The original lexical key's convention and environment identity. These
    /// are key data, not an ABI shape or a physical environment layout key:
    /// direct nested functions and scoped captures keep their own identities.
    pub fn identity_shape(
        &self,
    ) -> Result<(Option<CallableConvention>, Option<ContentHash>), InstanceKeyError> {
        let mut current = self.encoding();
        loop {
            let mut atoms = InstanceAtoms(current);
            match atoms.next()? {
                b"hirsel.derived-instance.v1" => current = atoms.next()?,
                b"hirsel.instance-key.v3" => {
                    let mut parts = Vec::new();
                    while !atoms.0.is_empty() {
                        parts.push(atoms.next()?);
                    }
                    let environment = match parts.pop() {
                        Some([0]) => None,
                        Some(hash) if hash.len() == 32 && parts.pop() == Some(&[1][..]) => {
                            Some(ContentHash(hash.try_into().unwrap()))
                        }
                        _ => return Err(InstanceKeyError::Encoding),
                    };
                    let convention = match parts.pop() {
                        Some([0]) => None,
                        Some(multiplicity @ (b"many" | b"once")) => {
                            let mut parameters = Vec::new();
                            loop {
                                let passing = match parts.last().copied() {
                                    Some(b"borrow") => Passing::Borrow,
                                    Some(b"take") => Passing::Take,
                                    Some(b"mut") => Passing::Mut,
                                    _ => break,
                                };
                                parts.pop();
                                parameters.push(passing);
                            }
                            let count = parts.pop().ok_or(InstanceKeyError::Encoding)?;
                            if u64::from_le_bytes(
                                count.try_into().map_err(|_| InstanceKeyError::Encoding)?,
                            ) != parameters.len() as u64
                                || parts.pop() != Some(&[1][..])
                            {
                                return Err(InstanceKeyError::Encoding);
                            }
                            parameters.reverse();
                            Some(CallableConvention {
                                parameters,
                                multiplicity: if multiplicity == b"many" {
                                    hirsel_lir::abi_types::CallMultiplicity::Many
                                } else {
                                    hirsel_lir::abi_types::CallMultiplicity::Once
                                },
                            })
                        }
                        _ => return Err(InstanceKeyError::Encoding),
                    };
                    return Ok((convention, environment));
                }
                _ => return Err(InstanceKeyError::Encoding),
            }
        }
    }
    /// Whether this selected implementation retains the exact complete origin
    /// instance, including its type/size actuals and lexical body. This checks
    /// ordinary identity framing and grants no validation authority.
    pub fn is_same_or_derived_from(&self, origin: &Self) -> bool {
        let mut encoding = self.encoding();
        loop {
            if encoding == origin.encoding() {
                return true;
            }
            let mut atoms = InstanceAtoms(encoding);
            if atoms.next().ok() != Some(b"hirsel.derived-instance.v1".as_slice()) {
                return false;
            }
            let Ok(parent) = atoms.next() else {
                return false;
            };
            encoding = parent;
        }
    }

    /// Restore ordinary address metadata using the existing length-framed
    /// encoding. Recompute symbol identity rather than accepting a saved digest.
    /// This checks framing and the lexical owner, not source/native authority.
    pub fn from_encoding(
        definition: &DefPath,
        body: u32,
        encoding: Vec<u8>,
    ) -> Result<Self, InstanceKeyError> {
        let mut current = encoding.as_slice();
        let mut derived = Vec::new();
        let mut digest;
        loop {
            let mut atoms = InstanceAtoms(current);
            match atoms.next()? {
                b"hirsel.derived-instance.v1" => {
                    let parent = atoms.next()?;
                    let domain = std::str::from_utf8(atoms.next()?)
                        .map_err(|_| InstanceKeyError::Encoding)?;
                    let mut inputs = Vec::new();
                    while !atoms.0.is_empty() {
                        inputs.push(atoms.next()?);
                    }
                    derived.push((domain, inputs));
                    current = parent;
                }
                b"hirsel.instance-key.v3" => {
                    if atoms.next()? != definition.0.package.name.as_bytes()
                        || atoms.next()? != definition.0.package.version.as_bytes()
                        || atoms.number()? != definition.0.modules.len() as u64
                    {
                        return Err(InstanceKeyError::Encoding);
                    }
                    for module in &definition.0.modules {
                        if atoms.next()? != module.as_bytes() {
                            return Err(InstanceKeyError::Encoding);
                        }
                    }
                    if atoms.next()? != definition.0.name.as_bytes()
                        || atoms.number()? != u64::from(body)
                    {
                        return Err(InstanceKeyError::Encoding);
                    }
                    // Remaining flattened type/size/convention atoms retain their
                    // exact canonical bytes. Their semantic interpretation stays
                    // with instantiation, not this ordinary descriptor decoder.
                    while !atoms.0.is_empty() {
                        atoms.next()?;
                    }
                    digest = ContentHash::of_parts("hirsel.instance-key.sha256.v3", [current]);
                    break;
                }
                _ => return Err(InstanceKeyError::Encoding),
            }
        }
        for (domain, inputs) in derived.into_iter().rev() {
            digest =
                ContentHash::of_parts(domain, std::iter::once(digest.0.as_slice()).chain(inputs));
        }
        Ok(Self {
            definition: definition.clone(),
            body,
            encoding,
            digest,
        })
    }
    /// A selected native implementation retains the complete lexical instance
    /// and transformation inputs. Preserve existing symbol digest domains,
    /// while equality and durable addresses use the full canonical encoding.
    pub fn derived(&self, domain: &str, inputs: &[&[u8]]) -> Self {
        let mut encoding = Vec::new();
        for part in std::iter::once(b"hirsel.derived-instance.v1".as_slice())
            .chain([self.encoding(), domain.as_bytes()])
            .chain(inputs.iter().copied())
        {
            encoding.extend_from_slice(&(part.len() as u64).to_le_bytes());
            encoding.extend_from_slice(part);
        }
        Self {
            definition: self.definition.clone(),
            body: self.body,
            encoding,
            digest: ContentHash::of_parts(
                domain,
                std::iter::once(self.digest.0.as_slice()).chain(inputs.iter().copied()),
            ),
        }
    }
}

#[cfg(test)]
mod derived_tests;

/// Declaration ordinal of a binder's parameter.
trait Ordinal: Copy {
    fn ordinal(self) -> u32;
}
impl Ordinal for hirsel_types::TypeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}
impl Ordinal for hirsel_types::SizeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}
impl Ordinal for hirsel_types::ParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}

/// Length-framed canonical bytes, as in `hirsel_types::encoding`, with the
/// four erased mode axes never written.
struct KeyEncoder<'a> {
    bytes: Vec<u8>,
    types: &'a hirsel_types::TypeInterner,
    active: Vec<hirsel_types::TypeId>,
}

impl KeyEncoder<'_> {
    fn atom(&mut self, b: &[u8]) {
        self.bytes
            .extend_from_slice(&(b.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(b);
    }
    fn tag(&mut self, s: &str) {
        self.atom(s.as_bytes());
    }
    fn number(&mut self, n: u64) {
        self.atom(&n.to_le_bytes());
    }
    fn boolean(&mut self, b: bool) {
        self.atom(&[u8::from(b)]);
    }
    fn path(&mut self, p: &DefPath) {
        self.tag(&p.0.package.name);
        self.tag(&p.0.package.version);
        self.number(p.0.modules.len() as u64);
        for m in &p.0.modules {
            self.tag(m);
        }
        self.tag(&p.0.name);
    }
    fn name_site(&mut self, site: hirsel_types::NameSite) {
        use hirsel_types::NameSite;
        let (kind, index) = match site {
            NameSite::Expression(id) => ("expression", id.0),
            NameSite::Pattern(id) => ("pattern", id.0),
            NameSite::Type(id) => ("type", id.0),
            NameSite::Statement(id) => ("statement", id.0),
            NameSite::Item => ("item", 0),
        };
        self.tag(kind);
        self.number(u64::from(index));
    }
    /// A binder names its declaring scope (body site or relative depth) and ordinal.
    fn binder<P: Ordinal>(
        &mut self,
        reference: hirsel_types::BinderRef<P>,
    ) -> Result<(), InstanceKeyError> {
        match reference {
            hirsel_types::BinderRef::Bound { depth, parameter } => {
                self.tag("bound");
                self.number(u64::from(depth));
                self.number(u64::from(parameter.ordinal()));
            }
            hirsel_types::BinderRef::Body { scope, parameter } => {
                self.tag("body");
                self.path(scope.owner.def.path());
                self.number(u64::from(scope.owner.occurrence));
                self.name_site(scope.site);
                self.number(u64::from(parameter.ordinal()));
            }
        }
        Ok(())
    }
    /// A type parameter inside an actual is complete only if a binder within
    /// the actual (a generic function type) declares it.
    fn inner_parameter<P: Ordinal>(
        &mut self,
        reference: hirsel_types::BinderRef<P>,
    ) -> Result<(), InstanceKeyError> {
        match reference {
            hirsel_types::BinderRef::Bound { .. } => self.binder(reference),
            hirsel_types::BinderRef::Body { .. } => Err(InstanceKeyError::Incomplete),
        }
    }
    fn arguments(
        &mut self,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<(), InstanceKeyError> {
        use hirsel_types::{GenericArgument, Size};
        self.number(arguments.len() as u64);
        for argument in arguments {
            match argument {
                GenericArgument::Type(ty) => {
                    self.tag("type");
                    self.ty(*ty)?;
                }
                GenericArgument::Size(Size::Constant(n)) => {
                    self.tag("constant");
                    self.number(*n);
                }
                GenericArgument::Size(Size::Parameter(p)) => {
                    self.tag("size-parameter");
                    self.inner_parameter(*p)?;
                }
            }
        }
        Ok(())
    }
    fn authority(&mut self, a: &hirsel_types::AuthoritySummary) -> Result<(), InstanceKeyError> {
        use hirsel_types::{AuthoritySummary, AuthorityTerm};
        match a {
            AuthoritySummary::Unknown => self.tag("unknown"),
            AuthoritySummary::Known(terms) => {
                self.tag("known");
                let mut terms: Vec<_> = terms.iter().collect();
                terms.sort();
                terms.dedup();
                self.number(terms.len() as u64);
                for term in terms {
                    match term {
                        AuthorityTerm::Capability(p) => {
                            self.tag("capability");
                            self.path(p);
                        }
                        AuthorityTerm::Parameter(p) => {
                            self.tag("parameter");
                            self.inner_parameter(*p)?;
                        }
                        AuthorityTerm::TypeParameter(p) => {
                            self.tag("type-parameter");
                            self.inner_parameter(*p)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn type_parameters(
        &mut self,
        parameters: &[hirsel_types::TypeParameterRef],
    ) -> Result<(), InstanceKeyError> {
        let mut parameters = parameters.to_vec();
        parameters.sort();
        parameters.dedup();
        self.number(parameters.len() as u64);
        for p in parameters {
            self.inner_parameter(p)?;
        }
        Ok(())
    }
    fn ty(&mut self, id: hirsel_types::TypeId) -> Result<(), InstanceKeyError> {
        use hirsel_types::{
            AllocationTarget, DropAllocation, ErasedCaptures, GenericParameter, Multiplicity,
            TypeKind,
        };
        let t = self.types.get(id).ok_or(InstanceKeyError::MissingType)?;
        if self.active.contains(&id) {
            return Err(InstanceKeyError::StructuralCycle);
        }
        self.active.push(id);
        match &t.kind {
            TypeKind::Primitive(p) => {
                use hirsel_types::Primitive as P;
                self.tag("primitive");
                self.tag(match p {
                    P::Bool => "bool",
                    P::U8 => "u8",
                    P::U16 => "u16",
                    P::U32 => "u32",
                    P::U64 => "u64",
                    P::U128 => "u128",
                    P::I8 => "i8",
                    P::I16 => "i16",
                    P::I32 => "i32",
                    P::I64 => "i64",
                    P::I128 => "i128",
                    P::F32 => "f32",
                    P::F64 => "f64",
                    P::Char => "char",
                    P::String => "String",
                    P::Bytes => "Bytes",
                    P::Never => "never",
                });
            }
            TypeKind::Tuple(items) => {
                self.tag("tuple");
                self.number(items.len() as u64);
                for item in items.iter() {
                    self.ty(*item)?;
                }
            }
            TypeKind::Parameter(p) => {
                self.tag("parameter");
                self.inner_parameter(*p)?;
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                self.tag("named");
                self.path(definition.path());
                self.arguments(arguments)?;
            }
            TypeKind::Record(fields) => {
                self.tag("record");
                self.number(fields.len() as u64);
                for field in fields.iter() {
                    self.tag(field.name.as_str());
                    self.boolean(field.growable);
                    self.ty(field.ty)?;
                }
            }
            TypeKind::Variant(variants) => {
                self.tag("variant");
                self.number(variants.len() as u64);
                for variant in variants.iter() {
                    self.tag(variant.name.as_str());
                    self.number(variant.fields.len() as u64);
                    for (name, ty) in variant.fields.iter() {
                        self.boolean(name.is_some());
                        if let Some(name) = name {
                            self.tag(name.as_str());
                        }
                        self.ty(*ty)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                self.tag("function");
                self.number(f.state_promises.len() as u64);
                for promise in &f.state_promises {
                    self.inner_parameter(promise.selector.parameter)?;
                    self.number(promise.selector.fields.len() as u64);
                    for field in &promise.selector.fields {
                        self.number(field.0 as u64);
                    }
                    self.number(u64::from(promise.advance_only));
                    self.number(u64::from(promise.preserve_captures));
                    self.number(u64::from(promise.preserve_unique));
                }
                self.number(f.generics.len() as u64);
                for g in f.generics.iter() {
                    match g {
                        GenericParameter::Type { bounds, .. } => {
                            self.tag("type");
                            use hirsel_types::Bound as Bd;
                            let mut bounds: Vec<_> = bounds
                                .iter()
                                .map(|b| match b {
                                    Bd::Plain => "plain",
                                    Bd::Flat => "flat",
                                    Bd::Data => "data",
                                    Bd::Key => "key",
                                    Bd::Sendable => "sendable",
                                    Bd::Inert => "inert",
                                    Bd::Copyable => "copyable",
                                    Bd::NoallocDrop => "noalloc_drop",
                                    Bd::Serializable => "serializable",
                                })
                                .collect();
                            bounds.sort();
                            bounds.dedup();
                            self.number(bounds.len() as u64);
                            for b in bounds {
                                self.tag(b);
                            }
                        }
                        GenericParameter::Size { .. } => self.tag("size"),
                    }
                }
                self.number(f.parameters.len() as u64);
                for p in f.parameters.iter() {
                    self.tag(match Passing::from(p.passing) {
                        Passing::Borrow => "borrow",
                        Passing::Take => "take",
                        Passing::Mut => "mut",
                    });
                    self.ty(p.ty)?;
                }
                self.ty(f.result)?;
                self.boolean(f.error.is_some());
                if let Some(error) = f.error {
                    self.ty(error)?;
                }
                self.boolean(f.allocation == hirsel_types::Allocation::NoAlloc);
                self.authority(&f.authority)?;
                match &f.uses {
                    hirsel_types::Uses::Unrestricted => self.tag("unrestricted"),
                    hirsel_types::Uses::Only(a) => {
                        self.tag("only");
                        self.authority(a)?;
                    }
                }
                self.number(f.contracts.len() as u64);
                for c in f.contracts.iter() {
                    self.number(u64::from(c.ordinal));
                    self.atom(&c.code_hash.0);
                }
                self.boolean(f.foreign.is_some());
            }
            TypeKind::Box(inner) => {
                self.tag("box");
                self.ty(*inner)?;
            }
            TypeKind::Builtin { kind, arguments } => {
                use hirsel_types::BuiltinType as B;
                self.tag("builtin");
                self.tag(match kind {
                    B::Array => "Array",
                    B::List => "List",
                    B::Map => "Map",
                    B::Set => "Set",
                    B::SortedMap => "SortedMap",
                    B::Inline => "Inline",
                    B::Simd => "Simd",
                    B::Packed => "Packed",
                    B::Shared => "Shared",
                    B::Slots => "Slots",
                    B::SlotId => "SlotId",
                    B::Region => "Region",
                    B::Iter => "Iter",
                    B::Task => "Task",
                    B::Sender => "Sender",
                    B::Receiver => "Receiver",
                    B::Mutex => "Mutex",
                    B::Atomic => "Atomic",
                    B::Ptr => "Ptr",
                    B::FnPtr => "FnPtr",
                    B::TypeShape => "TypeShape",
                });
                self.arguments(arguments)?;
            }
            TypeKind::Error => return Err(InstanceKeyError::RecoveryType),
        }
        // Kept: multiplicity, drop effect, allocation target, erased captures.
        // Erased: relation, brand, uniqueness, sendability.
        self.tag(match t.modes.multiplicity {
            Multiplicity::Many => "many",
            Multiplicity::Once => "once",
        });
        match &t.drop_allocation {
            DropAllocation::NoAlloc => self.tag("drop-noalloc"),
            DropAllocation::MayAllocate => self.tag("drop-may-allocate"),
            DropAllocation::DependsOn(ps) => {
                self.tag("drop-depends");
                self.type_parameters(ps)?;
            }
        }
        self.boolean(t.allocation_target == Some(AllocationTarget::Heap));
        match &t.erased_captures {
            None => self.tag("captures-none"),
            Some(ErasedCaptures::Known(a)) => {
                self.tag("captures-known");
                self.authority(a)?;
            }
            Some(ErasedCaptures::FromArguments(ps)) => {
                self.tag("captures-from");
                let mut ps = ps.to_vec();
                ps.sort();
                ps.dedup();
                self.number(ps.len() as u64);
                for p in ps {
                    self.inner_parameter(p.parameter)?;
                    self.number(p.fields.len() as u64);
                    for field in &p.fields {
                        self.number(field.0 as u64);
                    }
                }
            }
        }
        self.active.pop();
        Ok(())
    }
}

/// A parameter of the instance: source ordinal, instance ordinal, convention.
/// The remapping must be a bijection on ordinals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ParameterView {
    pub source: u32,
    pub instance: u32,
    pub passing: Passing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CaptureView {
    pub scoped: bool,
    pub ordinal: u32,
    pub usage: CaptureUse,
}

/// Owned substitution of one instance; TypeIds are session interner rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Substitution {
    pub types: Vec<(hirsel_types::TypeParameterRef, hirsel_types::TypeId)>,
    pub sizes: Vec<(hirsel_types::SizeParameterRef, u64)>,
}

/// A shared generic TIR body viewed as one instance. The body is
/// never duplicated or edited; owner, capture, result and parameter
/// conventions are imported together.
#[derive(Clone, Debug)]
pub struct InstanceView {
    pub key: InstanceKey,
    definition: Option<hirsel_tir::DefinitionRef>,
    declaration: std::sync::Arc<InstanceDeclaration>,
    pub body: hirsel_tir::BodyId,
    pub owner: hirsel_lir::abi_types::CallableShape,
    pub substitution: Substitution,
    pub parameters: Vec<ParameterView>,
    pub captures: Vec<CaptureView>,
    /// The result is written to the hidden caller destination.
    pub caller_destination: bool,
}

/// Immutable declaration, caller and scoped-capture contract observations,
/// retained independently of the source implementation body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceDeclaration {
    pub definition: DefPath,
    pub signature: ContentHash,
    pub body_count: u32,
    pub caller_bodies: Vec<hirsel_tir::BodyId>,
    pub scoped_parameters: Vec<Vec<hirsel_lir::abi_types::ParameterPath>>,
}
impl InstanceDeclaration {
    pub fn of(definition: &hirsel_tir::Definition) -> Self {
        Self {
            definition: definition.path.path().clone(),
            signature: definition.signature_hash,
            body_count: definition.bodies.len() as u32,
            caller_bodies: definition.caller_bodies.clone(),
            scoped_parameters: definition
                .bodies
                .iter()
                .map(|b| {
                    b.scoped_parameters
                        .iter()
                        .map(|p| hirsel_lir::abi_types::ParameterPath {
                            parameter: p.parameter,
                            fields: p.fields.clone(),
                        })
                        .collect()
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceBodyError {
    Missing { definition: DefPath, body: u32 },
    Mismatch { definition: DefPath, body: u32 },
}
impl std::fmt::Display for InstanceBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { definition, body } => write!(
                f,
                "source body {body} of {definition} has not been demanded"
            ),
            Self::Mismatch { definition, body } => write!(
                f,
                "source body {body} of {definition} differs from its declaration descriptor"
            ),
        }
    }
}
impl std::error::Error for InstanceBodyError {}

impl InstanceView {
    #[allow(clippy::too_many_arguments)]
    pub fn from_definition(
        key: InstanceKey,
        definition: hirsel_tir::DefinitionRef,
        body: hirsel_tir::BodyId,
        owner: hirsel_lir::abi_types::CallableShape,
        substitution: Substitution,
        parameters: Vec<ParameterView>,
        captures: Vec<CaptureView>,
        caller_destination: bool,
    ) -> Self {
        let declaration = std::sync::Arc::new(InstanceDeclaration::of(&definition));
        Self {
            key,
            definition: Some(definition),
            declaration,
            body,
            owner,
            substitution,
            parameters,
            captures,
            caller_destination,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn from_declaration(
        key: InstanceKey,
        declaration: std::sync::Arc<InstanceDeclaration>,
        body: hirsel_tir::BodyId,
        owner: hirsel_lir::abi_types::CallableShape,
        substitution: Substitution,
        parameters: Vec<ParameterView>,
        captures: Vec<CaptureView>,
        caller_destination: bool,
    ) -> Self {
        Self {
            key,
            definition: None,
            declaration,
            body,
            owner,
            substitution,
            parameters,
            captures,
            caller_destination,
        }
    }
    pub fn declaration(&self) -> &std::sync::Arc<InstanceDeclaration> {
        &self.declaration
    }
    pub fn tracks_caller(&self) -> bool {
        self.declaration.caller_bodies.contains(&self.body)
    }
    pub fn definition(&self) -> Result<&hirsel_tir::DefinitionRef, InstanceBodyError> {
        self.definition
            .as_ref()
            .ok_or_else(|| InstanceBodyError::Missing {
                definition: self.key.definition().clone(),
                body: self.body.0,
            })
    }
    /// Drop retention only; query/dependency identity is unchanged.
    pub fn release_definition(&mut self) {
        self.definition = None;
    }
    /// Attach an actually demanded source product. This is referential checking,
    /// not source V authority; the caller must obtain V acceptance independently.
    pub fn attach_definition(
        &mut self,
        definition: hirsel_tir::DefinitionRef,
    ) -> Result<(), InstanceBodyError> {
        if InstanceDeclaration::of(&definition) != *self.declaration
            || definition.path.path() != self.key.definition()
            || self.body.0 >= self.declaration.body_count
        {
            return Err(InstanceBodyError::Mismatch {
                definition: self.key.definition().clone(),
                body: self.body.0,
            });
        }
        self.definition = Some(definition);
        Ok(())
    }

    /// Referential integrity: the body exists, every substituted TypeId is a
    /// row of `types`, and parameter remapping is a bijection.
    pub fn check_identity(&self, types: &hirsel_types::TypeInterner) -> Result<(), SchemaError> {
        let definition = self
            .definition()
            .map_err(|_| SchemaError::Stale("instance source body has not been demanded"))?;
        let bodies = definition.bodies.len();
        if self.body.0 as usize >= bodies {
            return Err(SchemaError::ForeignId {
                kind: "TIR body",
                index: self.body.0,
                bound: bodies,
            });
        }
        if definition.path.path() != self.key.definition() || self.key.body() != self.body.0 {
            return Err(SchemaError::Stale("instance key names another body"));
        }
        for (ordinal, (_, ty)) in self.substitution.types.iter().enumerate() {
            if !types.contains(*ty) {
                return Err(SchemaError::ForeignId {
                    kind: "TypeId",
                    index: ordinal as u32,
                    bound: types.len(),
                });
            }
        }
        let n = self.parameters.len();
        let mut sources = vec![false; n];
        let mut targets = vec![false; n];
        for p in &self.parameters {
            let (s, t) = (p.source as usize, p.instance as usize);
            if s >= n || t >= n || std::mem::replace(&mut sources[s], true) {
                return Err(SchemaError::Malformed(
                    "parameter remapping is not a bijection",
                ));
            }
            if std::mem::replace(&mut targets[t], true) {
                return Err(SchemaError::Malformed(
                    "parameter remapping is not a bijection",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod encoding_tests;
