//! Semantic types and complete separately checked signatures (LANG-6, PROG-5).

#[cfg(test)]
hirsel_guard::install!();

pub mod unique_summary;
pub use unique_summary::{UniqueResultRow, UniqueSummary};
pub mod core_roles;
pub mod encoding;
pub mod entry;
pub use encoding::signature_types;
pub mod rows;
pub mod serialization;
pub mod storage;
pub mod wire;
pub use serialization::{serializable_schema, structural_serializable};
mod slice;
pub use slice::SharedSlice;
mod path;
pub use path::{PathInterner, SharedPath};

use hirsel_diag::Diagnostic;
mod crossing;
pub use crossing::*;
mod generics;
pub use generics::*;
mod resolution;
pub use resolution::*;
mod authority;
mod iteration;
mod metadata;
pub use iteration::*;
mod properties;
pub use authority::*;
use hirsel_intern::Symbol;
use hirsel_syntax::FileContext;
pub use hirsel_syntax::{Bound, LexicalOwner, NameSite, PassingMode, SiteRef};
use hirsel_syntax::{ContentHash, ContractKind, DefId, DefPath, ExprId, PatternId, TypeExprId};
pub use metadata::*;
pub use properties::*;

macro_rules! ids { ($($name:ident),* $(,)?) => {$(
    /// Index in its owning semantic arena, never a cross-query identity (LANG-6).
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
    pub struct $name(pub u32);
)*}; }
ids!(
    TypeParamId,
    SizeParamId,
    ParamId,
    LocalId,
    RegionId,
    FieldId,
    VariantId
);
/// Sized scalar set; literals without expected types never default (LANG-2, SEM-1/2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// A Unicode scalar value: 0..=0x10FFFF excluding surrogates (LANG-2, SEM-3).
    Char,
    String,
    Bytes,
    Never,
}
impl Primitive {
    /// Bit width and signedness of an integer primitive.
    pub fn integer_width(self) -> Option<(u32, bool)> {
        use Primitive::*;
        Some(match self {
            U8 => (8, false),
            U16 => (16, false),
            U32 => (32, false),
            U64 => (64, false),
            U128 => (128, false),
            I8 => (8, true),
            I16 => (16, true),
            I32 => (32, true),
            I64 => (64, true),
            I128 => (128, true),
            _ => return None,
        })
    }
    /// The values a range pattern of this type ranges over, as ascending
    /// inclusive intervals of `pattern_key`s: every integer, and every Unicode
    /// scalar for `char` (LANG-13). None for every other primitive.
    pub fn pattern_domain(self) -> Option<Vec<(u128, u128)>> {
        if self == Primitive::Char {
            return Some(vec![(0, 0xD7FF), (0xE000, 0x10FFFF)]);
        }
        let (width, _) = self.integer_width()?;
        Some(vec![(0, u128::MAX >> (128 - width))])
    }
    /// The order-preserving key of a value's two's-complement bits: a signed
    /// integer flips its sign bit, so keys compare as the values do.
    pub fn pattern_key(self, bits: u128) -> Option<u128> {
        if self == Primitive::Char {
            return Some(bits);
        }
        let (width, signed) = self.integer_width()?;
        Some(if signed {
            bits ^ (1 << (width - 1))
        } else {
            bits
        })
    }
}
/// Equality-only generic sizes (LANG-7); arithmetic sizes are not a type language.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Size {
    Constant(u64),
    Parameter(SizeParameterRef),
}
/// Type or size instantiation argument (GEN-4/5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum GenericArgument {
    Type(TypeId),
    Size(Size),
}
/// A parameter reference names its declaring binder, independently of storage.
/// Every callable introduces a binder, including nongeneric callables. Signature
/// depth zero is the nearest binder; checked free references name a source scope
/// qualified by its owning definition occurrence, so a row means the same thing
/// in every definition that holds it.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum BinderRef<P> {
    Bound { depth: u32, parameter: P },
    Body { scope: SiteRef, parameter: P },
}
pub type ParameterRef = BinderRef<ParamId>;
pub type TypeParameterRef = BinderRef<TypeParamId>;
pub type SizeParameterRef = BinderRef<SizeParamId>;
impl<P: Copy> BinderRef<P> {
    pub const fn bound(parameter: P) -> Self {
        Self::Bound {
            depth: 0,
            parameter,
        }
    }
    pub const fn body(scope: SiteRef, parameter: P) -> Self {
        Self::Body { scope, parameter }
    }
    pub const fn parameter(self) -> P {
        match self {
            Self::Bound { parameter, .. } | Self::Body { parameter, .. } => parameter,
        }
    }
    pub const fn binder(self) -> GenericBinder {
        match self {
            Self::Bound { depth, .. } => GenericBinder::Bound(depth),
            Self::Body { scope, .. } => GenericBinder::Body(scope),
        }
    }
}
/// The binder removed by generic instantiation. Bound depths are relative to the
/// graph root; Body identities are invariant under graph transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum GenericBinder {
    Bound(u32),
    Body(SiteRef),
}
/// Origin root; parameter references survive separate checking (PROV-1/2).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum OriginRoot {
    /// A declared operation selects the retained brand of this argument, rather
    /// than the identity of the wrapper value itself (for example PositionView).
    ArgumentBrand(ParameterRef),
    /// Identity of a materialized source expression of the owner's syntax.
    Expression(LexicalOwner, ExprId),
    Parameter(ParameterRef),
    Local(LexicalOwner, LocalId),
    Definition(DefPath),
    Sibling(FieldId),
}
/// Identity path; projecting fields preserves the named origin (PROV-1/5).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct Origin {
    pub root: OriginRoot,
    pub fields: SharedPath<FieldId>,
}
/// Canonical sorted, deduplicated union; >1 alternative fails at branded use (PROV-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Brand {
    pub alternatives: SharedSlice<Origin>,
}
/// Fully deep uniqueness, bounded by Shared roots (MODE-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Uniqueness {
    Aliased,
    Unique,
}
/// Call multiplicity (MODE-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Multiplicity {
    Many,
    Once,
}
/// Explicit sendability proof state, including generic obligations (MODE-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Sendability {
    Sendable,
    NotSendable,
    DependsOn(SharedSlice<TypeParameterRef>),
}
/// Orthogonal value modes; defaults are resolved by parameter/result position (MODE-7).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Modes {
    pub relation: Relation,
    pub uniqueness: Uniqueness,
    pub multiplicity: Multiplicity,
    pub sendability: Sendability,
}
impl Modes {
    pub fn global() -> Self {
        Self {
            relation: Relation::global(),
            uniqueness: Uniqueness::Aliased,
            multiplicity: Multiplicity::Many,
            sendability: Sendability::NotSendable,
        }
    }
}
/// A receiving crossing contract; it never supplies current value evidence.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq, Hash,
)]
pub enum SendabilityRequirement {
    #[default]
    None,
    Sendable,
}
/// A known ambient allocator value (MEM-4). Ordinary Arena values leave this absent.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AllocationTarget {
    Heap,
}
/// A type edge carries modes (its relation included) and brands, including nested generic arguments (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Type {
    /// Fixed crossing requirement of this receiving occurrence, independent of current evidence.
    pub sendability_requirement: SendabilityRequirement,
    /// The allocator denoted by this value, independent of its storage relation.
    pub allocation_target: Option<AllocationTarget>,
    /// Destruction effect, independent of function invocation (MEM-5).
    pub drop_allocation: DropAllocation,
    pub kind: TypeKind,
    pub modes: Modes,
    /// Held authority of erased Iter state and callbacks. None is unknown;
    /// other type kinds derive authority structurally and must leave this absent.
    pub erased_captures: Option<ErasedCaptures>,
    pub brand: Brand,
}
impl Type {
    /// An unqualified global row with no crossing requirement, allocator refinement,
    /// erased capture evidence or brand, and nonallocating cleanup.
    /// Callers supply any derived modes and cleanup obligations explicitly.
    pub fn with_kind(kind: TypeKind) -> Self {
        Self {
            sendability_requirement: SendabilityRequirement::None,
            allocation_target: None,
            drop_allocation: DropAllocation::NoAlloc,
            kind,
            modes: Modes::global(),
            erased_captures: None,
            brand: Brand::default(),
        }
    }
    /// Native unit needs no declaration or provider read.
    pub fn unit() -> Self {
        Self::native_control(TypeKind::Tuple(vec![].into()))
    }
    /// Native branch condition needs no declaration or provider read.
    pub fn boolean() -> Self {
        Self::native_control(TypeKind::Primitive(Primitive::Bool))
    }
    fn native_control(kind: TypeKind) -> Self {
        Self {
            modes: Modes {
                sendability: Sendability::Sendable,
                ..Modes::global()
            },
            ..Self::with_kind(kind)
        }
    }
}
/// Erased iterator environments retain their complete authority. A declaration
/// can select its captured value parameters; calls resolve this against actual
/// authority and sendability before publishing the resulting value type.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ErasedCaptures {
    FromArguments(SharedSlice<RelationPlace>),
    Known(AuthoritySummary),
}
impl ErasedCaptures {
    pub fn authority(&self) -> AuthoritySummary {
        match self {
            Self::Known(a) => a.clone(),
            Self::FromArguments(ps) => {
                if ps.iter().any(|p| !p.fields.is_empty()) {
                    return AuthoritySummary::Unknown;
                }
                AuthoritySummary::Known(
                    ps.iter()
                        .map(|p| AuthorityTerm::Parameter(p.parameter))
                        .collect(),
                )
            }
        }
    }
}
/// Complete actual evidence used when instantiating erased capture contracts.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CaptureProperties {
    pub authority: AuthoritySummary,
    pub sendability: Sendability,
    pub source_bounds: Relation,
    pub brands: Brand,
    pub multiplicity: Multiplicity,
    pub drop_allocation: DropAllocation,
}
impl CaptureProperties {
    /// The caller supplies independently derived authority/crossing evidence;
    /// the remaining axes retain the selected actual's complete occurrence.
    pub fn from_type(row: &Type, authority: AuthoritySummary, sendability: Sendability) -> Self {
        Self {
            authority,
            sendability,
            source_bounds: row.modes.relation.clone(),
            brands: row.brand.clone(),
            multiplicity: row.modes.multiplicity,
            drop_allocation: row.drop_allocation.clone(),
        }
    }
}
/// Semantic layouts are inline unless a block is explicitly required (MEM-7, GEN-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum TypeKind {
    Primitive(Primitive),
    Tuple(SharedSlice<TypeId>),
    Parameter(TypeParameterRef),
    Named {
        definition: DefId,
        arguments: SharedSlice<GenericArgument>,
    },
    Record(SharedSlice<Field>),
    Variant(SharedSlice<Variant>),
    Function(FunctionType),
    Box(TypeId),
    Builtin {
        kind: BuiltinType,
        arguments: SharedSlice<GenericArgument>,
    },
    /// Recovery-only; a successful public signature must not contain this (LANG-6).
    Error,
}
/// Standard type constructors with special checker/runtime rules (MEM-4/7/8/11/13/14, CONC-2/6/7).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BuiltinType {
    Array,
    List,
    Map,
    Set,
    SortedMap,
    Inline,
    Simd,
    Packed,
    Shared,
    Slots,
    SlotId,
    Region,
    Iter,
    Task,
    Sender,
    Receiver,
    Mutex,
    Atomic,
    Ptr,
    FnPtr,
    TypeShape,
}
/// Semantic field; dependent sibling brands refer to its index (PROV-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Field {
    pub growable: bool,
    pub name: Symbol,
    pub ty: TypeId,
}
/// Positional payloads have None names; records use Some (LANG-12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Variant {
    pub name: Symbol,
    pub fields: SharedSlice<(Option<Symbol>, TypeId)>,
}
/// Declared invocation or resource-finalizer allocation promise (MEM-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Allocation {
    MayAllocate,
    NoAlloc,
}
/// Cleanup conjunction; an unknown/erased proof conservatively permits allocation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub enum DropAllocation {
    NoAlloc,
    #[default]
    MayAllocate,
    DependsOn(SharedSlice<TypeParameterRef>),
}
impl DropAllocation {
    /// Join owned component cleanup effects; generic obligations form a set.
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::MayAllocate, _) | (_, Self::MayAllocate) => Self::MayAllocate,
            (Self::NoAlloc, Self::NoAlloc) => Self::NoAlloc,
            (Self::NoAlloc, Self::DependsOn(a)) | (Self::DependsOn(a), Self::NoAlloc) => {
                Self::DependsOn(a).join(Self::DependsOn(vec![].into()))
            }
            (Self::DependsOn(mut a), Self::DependsOn(b)) => {
                a.extend(b);
                a.sort();
                a.dedup();
                if a.is_empty() {
                    Self::NoAlloc
                } else {
                    Self::DependsOn(a)
                }
            }
        }
    }
}
/// Summary terms can refer to parameters and type arguments without inspecting bodies (CAP-2, CLO-4).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum AuthorityTerm {
    Capability(DefPath),
    Parameter(ParameterRef),
    TypeParameter(TypeParameterRef),
}
/// Sorted, deduplicated terms; Unknown is allowed only for transparent scoped calls (CLO-2, CAP-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum AuthoritySummary {
    Known(SharedSlice<AuthorityTerm>),
    Unknown,
}
impl AuthoritySummary {
    /// Held authority can weaken to a broader promise; unknown actual evidence
    /// cannot satisfy a known restriction.
    pub fn is_subset_of(&self, expected: &Self) -> bool {
        match (self, expected) {
            (_, Self::Unknown) => true,
            (Self::Known(actual), Self::Known(expected)) => {
                actual.iter().all(|t| expected.contains(t))
            }
            _ => false,
        }
    }

    /// Join declared field summaries and explicit uses; Unknown cannot prove inert.
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Known(a), Self::Known(b)) => {
                // Rust alloc/src/vec/mod.rs:3631-3633,3790-3792: edit the
                // owned buffer, then freeze once instead of copying each edit.
                let mut terms = Vec::with_capacity(a.len() + b.len());
                terms.extend_from_slice(&a);
                terms.extend_from_slice(&b);
                terms.sort();
                terms.dedup();
                Self::Known(terms.into())
            }
            _ => Self::Unknown,
        }
    }
    pub fn inert() -> Self {
        Self::Known(vec![].into())
    }
}
/// An omitted uses clause is not an empty restriction (CAP-3, CLO-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Uses {
    Unrestricted,
    Only(AuthoritySummary),
}
/// Scope lifetime of library-declared capabilities (MODE-6, CAP-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CapabilityKind {
    Global,
    Scoped,
}
/// Generic declaration with stable ordinal IDs, not symbols from another session (GEN-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum GenericParameter {
    Type {
        name: String,
        bounds: SharedSlice<Bound>,
    },
    Size {
        name: String,
    },
}
/// Public parameter convention, label and type (LANG-6/12, OWN-1/2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Parameter {
    pub name: Option<String>,
    pub label: Option<String>,
    pub passing: PassingMode,
    pub ty: TypeId,
}
/// Signature-owned code identity. Contract code is queried with its owner and ordinal (FACT-1, CT-2).
/// Hash includes resolved names and referenced signatures, excludes locations and arena IDs.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Contract {
    pub kind: ContractKind,
    pub ordinal: u32,
    pub code_hash: ContentHash,
}
/// FFI declarations are trusted but explicit and monomorphic (PROG-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ForeignContract {
    pub abi: String,
    pub blocking: bool,
    /// The extern's doc comment.
    pub aliasing_contract: String,
}
/// A safe wrapper's trusted kind and model (PROG-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ForeignWrapper {
    pub kind: WrapperKind,
    /// Its results and effects are a function of its arguments, so replay
    /// may run it again.
    pub deterministic: bool,
}
/// Where an `unsafe ffi` package's externs run (S-13): its manifest's trusted `threads`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ForeignThreads {
    /// The run's admission thread (the default).
    #[default]
    Home,
    /// Any thread, one call chain at a time per package; a callback's nested call to the
    /// same package re-enters.
    Serial,
    /// Any thread, concurrently.
    Any,
}
impl ForeignThreads {
    /// Its manifest spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Serial => "serial",
            Self::Any => "any",
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum WrapperKind {
    /// The result depends only on the arguments.
    Pure,
    /// It acts on foreign state reachable from the handles it is given.
    Handle,
    /// It acts on the host outside Hirsel's capabilities.
    Effect,
}
/// Complete callable signature. Error is None for non-Result results, otherwise matches Result's E (LANG-6/8).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FunctionType {
    /// Mutable state promises are declared on this callable, including indirect calls.
    pub state_promises: SharedSlice<StatePromise>,
    pub generics: SharedSlice<GenericParameter>,
    pub parameters: SharedSlice<Parameter>,
    pub result: TypeId,
    pub error: Option<TypeId>,
    pub allocation: Allocation,
    pub authority: AuthoritySummary,
    pub uses: Uses,
    pub contracts: SharedSlice<Contract>,
    pub incomplete: bool,
    pub foreign: Option<ForeignContract>,
}
/// A field-scoped selector inside a callable's own parameter binder.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct RelationPlace {
    pub parameter: ParameterRef,
    pub fields: SharedPath<FieldId>,
}
impl From<ParameterRef> for RelationPlace {
    fn from(parameter: ParameterRef) -> Self {
        Self {
            parameter,
            fields: SharedPath::default(),
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct StatePromise {
    pub selector: RelationPlace,
    pub advance_only: bool,
    pub preserve_captures: bool,
    /// MODE-2: a unique selected argument comes back unique when every other
    /// non-plain taken or `mut` argument is unique too.
    pub preserve_unique: bool,
    /// MODE-2: borrowed parameters a bodyless core operation reads and stores
    /// nothing derived from, which its borrowed-argument clause exempts.
    pub unretained: SharedSlice<ParamId>,
}
/// Preserve the selected incoming erased environment in the callable interface.
/// Field selectors remain on the header: a nominal's representation is immutable.
pub fn apply_state_promises(
    store: &TypeInterner,
    parameters: &mut [Parameter],
    promises: &[StatePromise],
) {
    for promise in promises {
        if !promise.selector.fields.is_empty() {
            continue;
        }
        let ParameterRef::Bound {
            depth: 0,
            parameter,
        } = promise.selector.parameter
        else {
            continue;
        };
        let Some(parameter) = parameters.get_mut(parameter.0 as usize) else {
            continue;
        };
        let Some(row) = store.get(parameter.ty) else {
            continue;
        };
        if !matches!(
            row.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) {
            continue;
        }
        let mut row = row.clone();
        row.erased_captures = Some(ErasedCaptures::FromArguments(
            vec![promise.selector.clone()].into(),
        ));
        parameter.ty = store.intern(row);
    }
}
/// Callable reception can forget a stronger preservation guarantee, never invent one.
pub fn state_promises_satisfy(actual: &[StatePromise], expected: &[StatePromise]) -> bool {
    expected.iter().all(|expected| {
        actual.iter().any(|actual| {
            actual.selector == expected.selector
                && (!expected.advance_only || actual.advance_only)
                && (!expected.preserve_captures || actual.preserve_captures)
                && (!expected.preserve_unique || actual.preserve_unique)
                && expected
                    .unretained
                    .iter()
                    .all(|p| actual.unretained.contains(p))
        })
    })
}
/// Explicit native storage promises (MEM-7/12), separate from SER-2 wire schemas.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub struct Layout {
    pub c: bool,
    pub packed: bool,
    pub align: Option<u32>,
    pub soa: bool,
    pub flat: bool,
}
/// Public type declaration signature, including opaque guarantees and destruction (GEN-1, DROP-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TypeDefinition {
    pub loop_source: Option<LoopSourceDeclaration>,
    /// `@format(function)`: the function an f-string hole of this type calls (SEM-3).
    pub format: Option<DefPath>,
    /// Declared resource finalizer promise; MayAllocate unless explicitly noalloc.
    pub destructor_allocation: Allocation,
    pub generics: Vec<GenericParameter>,
    pub representation: Option<TypeId>,
    pub bounds: Vec<Bound>,
    pub authority: AuthoritySummary,
    /// Whole-value nominal seal kind; identity is Signature.definition (CAP-5).
    pub capability: Option<CapabilityKind>,
    /// Owner-only declared ceiling, separate from public whole-value authority.
    /// None on client exports and ordinary non-capability declarations.
    pub implementation_authority: Option<AuthoritySummary>,
    /// Written public schema opt-in. Eligibility is independently checked (SER-1).
    pub serializable: bool,
    /// `@serializable(check = function)` on an opaque type: its public schema
    /// representation and the function that admits a decoded one (SER-1).
    pub serializable_check: Option<SerializableCheck>,
    pub layout: Layout,
    pub distinct: bool,
    pub resource: bool,
    pub drop: Option<DefPath>,
    pub fact: Option<Contract>,
    /// `@captures(fields)`: an erased carrier. Its value authority joins the
    /// declared summary with each value's own erased evidence (CAP-2, CLO-4).
    /// The selected representation field ordinals (a distinct type's field is
    /// 0); empty in a public opaque export, which has no representation.
    pub captures: Option<SharedSlice<u32>>,
}
/// A checked opaque type's serialized form: its own representation, published as its
/// wire contract, and `fn(value: T) -> bool uses {}` in its module, called on every
/// decoded occurrence before the value is exposed (SER-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SerializableCheck {
    pub function: DefPath,
    /// The opaque's representation, indexing the signature's types.
    pub representation: TypeId,
}
/// Every definition exposes one signature category (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum SignatureKind {
    Function(FunctionType),
    Type(TypeDefinition),
    Const {
        ty: TypeId,
        generics: SharedSlice<GenericParameter>,
    },
    /// A `signature` declaration (GEN-6).
    Family(FamilySignature),
    /// An `implementation` declaration (GEN-6).
    Implementation(ImplementationSignature),
}
/// A `signature`: its members in declaration order. Its type and size members,
/// in order, are the generic parameters of every operation's declaration
/// (GEN-6), so instantiating an operation at an implementation's actuals is
/// ordinary substitution.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FamilySignature {
    pub members: Vec<FamilyMember>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FamilyMember {
    pub name: String,
    pub kind: FamilyMemberKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum FamilyMemberKind {
    /// An abstract type with its required bounds.
    Type {
        bounds: SharedSlice<Bound>,
    },
    Size,
    /// A complete operation header: the bodyless declaration
    /// [`family_operation_path`] names, generic over the family's types and sizes.
    Operation,
}
impl FamilySignature {
    /// The generic parameters a family's operations declare: its type and
    /// size members in declaration order, named by the member.
    pub fn parameters(&self) -> Vec<GenericParameter> {
        self.members
            .iter()
            .filter_map(|member| match &member.kind {
                FamilyMemberKind::Type { bounds } => Some(GenericParameter::Type {
                    name: member.name.clone(),
                    bounds: bounds.clone(),
                }),
                FamilyMemberKind::Size => Some(GenericParameter::Size {
                    name: member.name.clone(),
                }),
                FamilyMemberKind::Operation => None,
            })
            .collect()
    }
    /// The parameter ordinal of a type or size member.
    pub fn parameter(&self, name: &str) -> Option<usize> {
        self.members
            .iter()
            .filter(|m| !matches!(m.kind, FamilyMemberKind::Operation))
            .position(|m| m.name == name)
    }
    pub fn member(&self, name: &str) -> Option<&FamilyMember> {
        self.members.iter().find(|m| m.name == name)
    }
}
/// The bodyless declaration that holds a family operation's header (GEN-6).
pub fn family_operation_path(family: &DefPath, operation: &str) -> DefPath {
    let mut path = family.clone();
    path.0.name = format!("{}$member${operation}", family.0.name);
    path
}
/// The specialization of `base` at closed implementation actuals (GEN-6):
/// `f$with$H=pkg@1/m/impl$..`, one entry per binder in declaration order.
pub fn specialization_path(base: &DefPath, bindings: &[(String, DefPath)]) -> DefPath {
    let mut path = base.clone();
    path.0.name = std::iter::once(format!("{}$with", base.0.name))
        .chain(
            bindings
                .iter()
                .map(|(binder, implementation)| format!("{binder}={implementation}")),
        )
        .collect::<Vec<_>>()
        .join("$");
    path
}
/// The base declaration and bindings of a [`specialization_path`].
pub fn specialization_parts(path: &DefPath) -> Option<(DefPath, Vec<(String, DefPath)>)> {
    let (name, rest) = path.0.name.split_once("$with$")?;
    let mut base = path.clone();
    base.0.name = name.to_owned();
    let bindings = rest
        .split('$')
        .map(|binding| {
            let (binder, implementation) = binding.split_once('=')?;
            let mut parts = implementation.split('/');
            let (package, version) = parts.next()?.split_once('@')?;
            let mut rest: Vec<String> = parts.map(str::to_owned).collect();
            let name = rest.pop()?;
            Some((
                binder.to_owned(),
                DefPath(hirsel_syntax::ItemPath {
                    package: hirsel_syntax::PackageId {
                        name: package.to_owned(),
                        version: version.to_owned(),
                    },
                    modules: rest,
                    name,
                }),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    Some((base, bindings))
}
/// The transparent alias that names an implementation's type member (GEN-6).
pub fn implementation_type_path(implementation: &DefPath, member: &str) -> DefPath {
    let mut path = implementation.clone();
    path.0.name = format!("{}$member${member}", implementation.0.name);
    path
}
/// An `implementation`: the family it implements and each member's actual,
/// in the family's member order. Type actuals are the transparent aliases'
/// representations; operations name their exact target (GEN-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ImplementationSignature {
    pub family: DefId,
    pub members: Vec<(String, MemberActual)>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum MemberActual {
    Type(TypeId),
    Size(u64),
    Operation(DefPath),
}
impl ImplementationSignature {
    pub fn member(&self, name: &str) -> Option<&MemberActual> {
        self.members
            .iter()
            .find(|(member, _)| member == name)
            .map(|(_, actual)| actual)
    }
    /// The type actuals' rows, in member order.
    pub fn types(&self) -> Vec<TypeId> {
        self.members
            .iter()
            .filter_map(|(_, actual)| match actual {
                MemberActual::Type(ty) => Some(*ty),
                _ => None,
            })
            .collect()
    }
    /// The family generic actuals: type and size members in order.
    pub fn arguments(&self) -> Vec<GenericArgument> {
        self.members
            .iter()
            .filter_map(|(_, actual)| match actual {
                MemberActual::Type(ty) => Some(GenericArgument::Type(*ty)),
                MemberActual::Size(n) => Some(GenericArgument::Size(Size::Constant(*n))),
                MemberActual::Operation(_) => None,
            })
            .collect()
    }
}
mod parameters;
pub use parameters::*;
mod graph;
pub use graph::*;
mod spelling;
pub use spelling::CanonicalSpelling;
mod interner;
pub use interner::*;
mod memo;
mod normal;
pub use memo::*;
mod table;
pub use table::*;
mod actual_key;
pub use actual_key::*;
#[cfg(test)]
mod arch_tests;
#[cfg(test)]
mod pool_tests;
#[cfg(test)]
mod prove_tests;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod test_api;
#[cfg(test)]
pub(crate) use test_api::*;
/// Self-contained cross-query signature: its TypeIds are rows of the session
/// interner, which every phase shares, so reading one imports nothing (LANG-6).
/// Fingerprints are of canonical semantic encoding, never Rust Debug or ids (PROG-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    pub metadata: SignatureMetadata,
    pub definition: DefId,
    pub kind: SignatureKind,
    pub fingerprint: ContentHash,
}
/// Cheap immutable signature snapshot across provider consumers.
pub type SignatureRef = std::sync::Arc<Signature>;

/// A resolved name is a binding, primitive, nominal definition or contextual variant (LANG-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ResolvedName {
    Local(LocalId),
    Parameter(ParamId),
    Definition(DefPath),
    Variant(VariantId),
    Size(SizeParameterRef),
    TypeParameter(TypeParameterRef),
    Primitive(Primitive),
}
/// Resolver output is shared data, not resolver crate internals (LANG-4, PROG-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    /// Phase snapshots and memo evidence share the completed read log.
    /// A later phase shares the immutable prefix and appends its own read nodes.
    pub reads: std::sync::Arc<SharedPath<DependencyRead>>,
    pub contextual: Vec<(NameSite, ContextualName)>,
    pub paths: Vec<(NameSite, ResolvedName)>,
    pub local_functions: Vec<(hirsel_syntax::StmtId, LocalId)>,
    /// The implicit finalizer binder, allocated by resolution (DROP-3).
    pub drop_binding: Option<LocalId>,
    pub deferred: Vec<DeferredName>,
    pub generics: Vec<GenericBinding>,
    pub generic_declarations: GenericDeclarations,
    pub reference_scopes: Vec<(NameSite, NameSite)>,
    pub expressions: Vec<(ExprId, ResolvedName)>,
    pub types: Vec<(TypeExprId, ResolvedName)>,
    pub bindings: Vec<(PatternId, Symbol, LocalId)>,
    pub dependencies: Vec<DefPath>,
    /// Generic actuals an implementation binder supplies (GEN-6), by the
    /// callee expression (a member path or a `with` clause) and the callee's
    /// generic ordinal; never inferred.
    pub static_arguments: Vec<(ExprId, Vec<(u32, StaticArgument)>)>,
    /// The item's implementation binders (GEN-6), as its signature records them.
    pub implementation_binders: Vec<ImplementationBinder>,
}
/// One generic actual an implementation binder supplies (GEN-6): an
/// enclosing binder's member parameter, or a bound implementation's actual.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StaticArgument {
    Parameter(GenericParameterName),
    Actual(GenericArgument),
}
/// Checker elaboration of one use, consumed by modes and lowering (OWN-1/2/3/4/8).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum UseKind {
    Borrow,
    Take,
    Copy,
    Mut,
}
/// The lowering action selected for a unary `unique` operation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum UniqueAction {
    AlreadyUnique,
    DeepCopy,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct UniqueDecision {
    pub expression: ExprId,
    pub action: UniqueAction,
}
impl UniqueAction {
    /// The source extraction selected by the ownership operation.
    pub fn usage(self) -> UseKind {
        match self {
            Self::AlreadyUnique => UseKind::Take,
            Self::DeepCopy => UseKind::Copy,
        }
    }
}
/// A guard tests borrowed payloads before committing ownership to its arm.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct GuardedPatternUse {
    pub pattern: PatternId,
    pub test: UseKind,
    pub commit: UseKind,
}
/// Capture creation context shared by authority, ownership and lowering.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureContext {
    Scoped,
    OwningScoped,
    Escaping,
    Task,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CaptureMode {
    pub expression: ExprId,
    pub ordinal: u32,
    pub owner: NameSite,
    pub usage: UseKind,
    pub context: CaptureContext,
}
/// Fixed source contract for an independently checked TIR borrow cursor.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct BorrowCursorDecision {
    pub binding: NameSite,
    pub cover: Option<Vec<(Origin, Vec<TypeId>)>>,
}
/// Complete static decisions; cleanup lowering still determines ordered drops.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnershipFacts {
    /// Confined counted-result owners released after a completed statement.
    /// Lowering emits ordinary cleanup; V reconstructs shares independently.
    #[serde(default)]
    pub confined_releases: Vec<(hirsel_syntax::StmtId, usize)>,
    pub borrow_cursors: Vec<BorrowCursorDecision>,
    pub cursor_rebindings: Vec<ExprId>,
    /// Plain rows in the session interner; K omits their cleanup.
    pub plain_types: std::collections::HashSet<TypeId>,
    /// Rows whose values have no user-visible destruction (DROP-1): copyable
    /// and not once. Destroying a holder of any other value, including rows
    /// appended after this pass, is a use of its loans (LANG-3).
    pub silent_drop_types: std::collections::HashSet<TypeId>,
    /// Borrowed call arguments whose result relation names only a field path
    /// `F` of their parameter, with the type of the argument place extended
    /// by `F` (#77). Lowering alone reads it, to loan that field place for
    /// the result instead of the whole argument.
    pub field_loans: Vec<(ExprId, TypeId)>,
    /// Borrowed call arguments naming a local binding of the body whose
    /// current content is local, related to a `data` result: relations
    /// compose (MODE-1), so the result holds the binding's content loans and
    /// no access loan on the binding. Lowering alone reads it.
    pub composed_arguments: Vec<ExprId>,
    pub captures: Vec<CaptureMode>,
    /// Callee consumption, independent of evaluating the source operand.
    pub call_arguments: Vec<(ExprId, ExprId, UseKind)>,
    /// J introductions that I proves can read initialized storage on every incoming path.
    pub identity_joins: Vec<(NameSite, NameSite, Origin)>,
    pub identity_fields: Vec<(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)>,
    /// Declaration-owned physical projections, separate from expression receipt modes.
    pub physical_projections: Vec<(ExprId, TypeId)>,
    /// Declaration-owned pattern payloads before receipt locality is inferred.
    /// Lowering adds the physical owner's bounds at each projection step.
    pub physical_patterns: Vec<(hirsel_syntax::PatternId, TypeId)>,
    pub unique: Vec<UniqueDecision>,
    pub guarded_patterns: Vec<GuardedPatternUse>,
    /// Assignment sites that detach receiver storage into allocation.
    pub allocating_mutations: Vec<ExprId>,
    /// Receiving content contract and I-proven storage at the write: Some(true) live, Some(false) dead, None mixed.
    pub replacements: Vec<(ExprId, TypeId, Option<bool>)>,
}

/// Lowering must obey checker decisions instead of inferring ownership again (LANG-3, OWN-8).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Elaboration {
    pub expression: ExprId,
    /// Fresh storage selected before held-part placement enriches the row (PROV-4).
    #[serde(default)]
    pub construction_destination: Option<Destination>,
    pub ty: TypeId,
    pub usage: UseKind,
    pub instantiated_arguments: Vec<GenericArgument>,
}
/// Recorded generic call; TypeIds refer to the enclosing CheckedBody.types (GEN-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Instantiation {
    pub expression: ExprId,
    pub callee: DefPath,
    pub arguments: Vec<GenericArgument>,
}
/// Explicit capture selected at closure construction, independent of uses inside its body (CLO-1/3/4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub binding: ResolvedName,
    pub usage: UseKind,
    pub ty: TypeId,
}
/// Mode-checked environment layout in capture evaluation order (MODE-5, LANG-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ClosureInfo {
    pub context: CaptureContext,
    /// Complete body result, independently of the contextual callable promise.
    pub result_evidence: Option<TypeId>,
    pub expression: ExprId,
    pub captures: Vec<Capture>,
    pub ty: TypeId,
}
/// Binding identity is scoped by its declaring function; resolver locals are not TIR slots.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct BindingDecision {
    pub owner: NameSite,
    pub binding: ResolvedName,
    pub site: NameSite,
    pub ty: TypeId,
    pub usage: UseKind,
    pub mutable: bool,
}
/// Explicit pattern decision; children map source patterns to declaration-order fields.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PatternDecision {
    pub pattern: PatternId,
    pub ty: TypeId,
    pub usage: UseKind,
    pub kind: PatternDecisionKind,
    pub fields: Vec<(FieldId, PatternId)>,
    /// On the root pattern of an annotated `let`: the mode axes its annotation
    /// writes. `None` for an unannotated let and every other pattern.
    pub annotation: Option<WrittenModes>,
}
impl PatternDecision {
    /// Direct subpatterns: the field patterns, or the alternatives.
    pub fn subpatterns(&self) -> impl Iterator<Item = PatternId> + '_ {
        let alternatives = match &self.kind {
            PatternDecisionKind::Or(alternatives) => alternatives.as_slice(),
            _ => &[],
        };
        self.fields
            .iter()
            .map(|(_, child)| *child)
            .chain(alternatives.iter().copied())
    }
}
/// The mode axes a `let` annotation writes (MODE-1/2/3). A written axis is the
/// receiving contract; an unwritten one follows the received value.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WrittenModes {
    /// `sendable` qualifies the annotation root.
    pub sendable: bool,
    /// A relation (`in S`) qualifies the annotation's root.
    pub relation: bool,
    /// `unique` qualifies the annotation's root.
    pub unique: bool,
    /// `once` qualifies the annotation's root.
    pub once: bool,
    /// The annotation's root is a function type, whose checked relation is
    /// the contract and whose multiplicity is Many unless `once` is written.
    pub function: bool,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum PatternDecisionKind {
    Wildcard,
    Binding(LocalId),
    Literal,
    /// A closed scalar range; its bounds stay in the syntax tree (LANG-13).
    Range,
    Tuple,
    Constructor(ContextualName),
    /// Alternatives, each a pattern of the same place and type (LANG-13).
    Or(Vec<PatternId>),
    Error,
}
/// A checked source storage path. Value roots cover computed receivers as well
/// as expressions which do not designate a binding's storage.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SourcePlace {
    pub expression: ExprId,
    pub root: SourceRoot,
    pub projections: SharedPath<SourceProjection>,
    pub access: SourceAccess,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum SourceRoot {
    Binding {
        owner: NameSite,
        binding: ResolvedName,
    },
    Definition(DefPath),
    Value(ExprId),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SourceProjection {
    /// The source node performing this step; dotted path steps share a node.
    pub expression: ExprId,
    pub kind: SourceProjectionKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum SourceProjectionKind {
    Field(FieldId),
    VariantField { variant: VariantId, field: FieldId },
    Index(ExprId),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceAccess {
    Read,
    Copy,
    Take,
    Mut,
    Unique,
}
/// One H-checked call, keyed by its retained source expression.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CheckedCall {
    pub expression: ExprId,
    pub target: CallTarget,
    /// Written evaluation order, independently of declaration order.
    pub arguments: Vec<CheckedArgument>,
    pub generic_arguments: Vec<GenericArgument>,
    /// Diagnostic provenance: generic ordinals fixed by the expected result
    /// before any operand was checked. These never affect a judgment.
    #[serde(default)]
    pub contextual_contracts: Vec<u32>,
    /// Declared result requirement before actual value-flow refinement.
    pub result: TypeId,
    pub state: CallState,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CheckedArgument {
    pub expression: ExprId,
    /// Missing only for an unmapped recovery operand; never a positional fallback.
    pub parameter: Option<(u32, Parameter)>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum CallTarget {
    Definition(DefPath),
    Value(ExprId),
    Constructor(ContextualName),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallState {
    Complete,
    MissingCapabilities,
    Recovery,
}
/// Source-authorized loop conversion, independently checked after lowering.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum CheckedTailKind {
    SelfTail,
    ConstructorHole(Vec<u32>),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CheckedTail {
    pub call: ExprId,
    pub kind: CheckedTailKind,
    /// Declaration ordinal and source expression of a single-use counted owner.
    pub owned_loop_arguments: Vec<(ParamId, ExprId)>,
}
/// Typechecked body tables keyed by source expression; mode checking enriches these tables (MODE-7).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CheckedBody {
    /// The definition occurrence whose syntax these tables are keyed by; rows
    /// qualify the body's lexical sites with it.
    pub lexical: LexicalOwner,
    pub places: Vec<SourcePlace>,
    pub calls: Vec<CheckedCall>,
    pub tails: Vec<CheckedTail>,
    /// Every declared binding, including unused locals/parameters and closure binders.
    pub bindings: Vec<BindingDecision>,
    /// Every pattern, including wildcards and nested unused patterns.
    pub patterns: Vec<PatternDecision>,
    pub resolution: Resolution,
    pub expressions: Vec<Elaboration>,
    pub instantiations: Vec<Instantiation>,
    pub closures: Vec<ClosureInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
}

/// Query failures distinguish unavailable implementation from language diagnostics (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum QueryError {
    /// Transient host/provider failure, never a deterministic rejection cache entry.
    Operational(OperationalFailure),
    NotImplemented,
    Diagnostics(Vec<Diagnostic>),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct OperationalFailure {
    pub operation: String,
    pub message: String,
}
impl QueryError {
    pub fn cacheable(&self) -> bool {
        matches!(self, Self::Diagnostics(_))
    }
}
/// A query may return recoverable diagnostics together with syntax (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct QueryOutput<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
    pub dependencies: Vec<SignatureDependency>,
}
/// A used signature, even for a rejected body; sorted by DefPath in cache keys (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SignatureDependency {
    // Keep the historical path encoding even inside a numbered std image.
    #[serde(with = "definition_path")]
    pub definition: DefId,
    pub fingerprint: ContentHash,
}

mod definition_path {
    use super::{DefId, DefPath};
    use serde::{Deserialize, Serialize};

    pub fn serialize<S: serde::Serializer>(id: &DefId, serializer: S) -> Result<S::Ok, S::Error> {
        id.path().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DefId, D::Error> {
        DefPath::deserialize(deserializer).map(|path| DefId::intern(&path))
    }
}

/// One lookup records a manifest dependency even when resolution failed.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct LookupOutput {
    pub manifest: ContentHash,
    pub result: Result<DefPath, QueryError>,
}
/// How much of a recursive type group's other members a normalization reads.
/// A member's header (generics, visibility and attributes) needs no other
/// declaration; its representation needs only the others' headers. A full
/// signature reads the others' representations, so no member's signature
/// waits on another's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupStage {
    Header,
    Representation,
}
/// A type declaration's recursive group: the other type declarations that its
/// body reaches and that reach it back, each as a provisional signature at the
/// requested stage, and the declaration's own group key. Empty outside a group.
#[derive(Clone, Debug, Default)]
pub struct RecursiveGroup {
    pub key: Option<ContentHash>,
    pub members: Vec<SignatureRef>,
}
/// Name lookup reads manifest identities, never another body's inferred facts (LANG-4/6).
/// Failed lookups must record the manifest hash, so adding a name invalidates failures.
pub trait SignatureProvider {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError>;
    fn lookup(&mut self, context: &FileContext, path: &hirsel_syntax::Path) -> LookupOutput;
    /// Whether these declarations share an indexed recursive type group.
    /// Header resolution cannot probe a member's unpublished signature. This
    /// query must not normalize signatures; providers cover membership with
    /// declaration lookups and group input reads, just as `recursive_group`.
    fn in_recursive_group(&mut self, _owner: &DefPath, _member: &DefPath) -> bool {
        false
    }
    /// `definition`'s recursive type group, its other members at `stage`.
    /// Providers without one (fixtures over a fixed registry) have none.
    fn recursive_group(
        &mut self,
        _definition: &DefPath,
        _stage: GroupStage,
    ) -> Result<RecursiveGroup, QueryError> {
        Ok(RecursiveGroup::default())
    }
    /// The name site of `definition`'s first declaration in the checked
    /// definition's own source file, for a secondary label. A citation of
    /// another item: the span is relative to that item's anchor, and the
    /// provider records the read so a change to the cited text invalidates it.
    fn cite_name(&mut self, definition: &DefPath) -> Option<hirsel_span::Span> {
        let _ = definition;
        None
    }
}

#[cfg(test)]
mod revision_tests;

mod declarations;
pub use declarations::*;

mod local_results;
pub use local_results::{
    local_result_destination, local_result_generics, local_result_permissions,
};
mod relation;
pub use relation::{
    Destination, Reach, Relation, adopt_local, arena_parameter, destination, is_arena_type,
    keyed_constructor, structurally_global,
};

mod nominal;
pub use nominal::*;

/// Shared fixture builders, available only with the `testing` feature.
#[cfg(feature = "testing")]
pub mod testing;

mod snapshot;

/// Normalize explicit constant aliases before constructing nominal brand evidence.
/// Lookup uses signatures only and detects cycles independently of evaluation.
pub fn constant_identity(
    mut path: DefPath,
    read: &mut impl FnMut(DefId) -> Result<SignatureRef, QueryError>,
) -> Result<DefPath, QueryError> {
    let mut chain = Vec::new();
    loop {
        if chain.contains(&path) {
            chain.push(path);
            return Err(QueryError::Diagnostics(vec![
                hirsel_diag::Diagnostic::error(
                    const { hirsel_diag::RuleId::of("GEN-6") },
                    hirsel_span::Span::default(),
                    format!(
                        "constant alias cycle: {}",
                        chain
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                ),
            ]));
        }
        chain.push(path.clone());
        let signature = read(DefId::intern(&path))?;
        match &signature.metadata.constant_alias {
            Some(target) => path = target.clone(),
            None => return Ok(path),
        }
    }
}
