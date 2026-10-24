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
    pub fn digest(&self) -> ContentHash {
        self.digest
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
    pub definition: hirsel_tir::DefinitionRef,
    pub body: hirsel_tir::BodyId,
    pub owner: hirsel_lir::abi_types::CallableShape,
    pub substitution: Substitution,
    pub parameters: Vec<ParameterView>,
    pub captures: Vec<CaptureView>,
    /// The result is written to the hidden caller destination.
    pub caller_destination: bool,
}

impl InstanceView {
    /// Referential integrity: the body exists, every substituted TypeId is a
    /// row of `types`, and parameter remapping is a bijection.
    pub fn check_identity(&self, types: &hirsel_types::TypeInterner) -> Result<(), SchemaError> {
        let bodies = self.definition.bodies.len();
        if self.body.0 as usize >= bodies {
            return Err(SchemaError::ForeignId {
                kind: "TIR body",
                index: self.body.0,
                bound: bodies,
            });
        }
        if self.definition.path.path() != self.key.definition() || self.key.body() != self.body.0 {
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
