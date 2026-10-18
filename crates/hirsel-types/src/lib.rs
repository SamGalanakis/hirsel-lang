//! Semantic types and complete separately checked signatures (LANG-6, PROG-5).
pub mod core_roles;
pub mod encoding;
pub mod rows;
pub mod serialization;
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
use hirsel_intern::{Interner, Symbol};
use hirsel_syntax::FileContext;
pub use hirsel_syntax::{Bound, PassingMode};
use hirsel_syntax::{ContentHash, ContractKind, DefPath, ExprId, PatternId, TypeExprId};
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
/// Equality-only generic sizes (LANG-7); arithmetic sizes are not a type language.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Size {
    Constant(u64),
    Parameter(SizeParameterRef),
}
/// Type or size instantiation argument (GEN-4/5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum GenericArgument {
    Type(TypeId),
    Size(Size),
}
/// A parameter reference names its declaring binder, independently of storage.
/// Every callable introduces a binder, including nongeneric callables. Signature
/// depth zero is the nearest binder; checked free references name a source scope.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum BinderRef<P> {
    Bound { depth: u32, parameter: P },
    Body { scope: NameSite, parameter: P },
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
    pub const fn body(scope: NameSite, parameter: P) -> Self {
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
    Body(NameSite),
}
/// Origin root; parameter references survive separate checking (PROV-1/2).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum OriginRoot {
    /// A declared operation selects the retained brand of this argument, rather
    /// than the identity of the wrapper value itself (for example PositionView).
    ArgumentBrand(ParameterRef),
    /// Identity of a materialized source expression within its retained syntax snapshot.
    Expression(ExprId),
    Parameter(ParameterRef),
    Local(LocalId),
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct Brand {
    pub alternatives: SharedSlice<Origin>,
}
/// Fully deep uniqueness, bounded by Shared roots (MODE-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Uniqueness {
    Aliased,
    Unique,
}
/// Call multiplicity (MODE-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Multiplicity {
    Many,
    Once,
}
/// Explicit sendability proof state, including generic obligations (MODE-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Sendability {
    Sendable,
    NotSendable,
    DependsOn(SharedSlice<TypeParameterRef>),
}
/// Orthogonal value modes; defaults are resolved by parameter/result position (MODE-7).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SendabilityRequirement {
    #[default]
    None,
    Sendable,
}
/// A known ambient allocator value (MEM-4). Ordinary Arena values leave this absent.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationTarget {
    Heap,
}
/// A type edge carries modes (its relation included) and brands, including nested generic arguments (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TypeKind {
    Primitive(Primitive),
    Tuple(SharedSlice<TypeId>),
    Parameter(TypeParameterRef),
    Named {
        definition: DefPath,
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub growable: bool,
    pub name: String,
    pub ty: TypeId,
}
/// Positional payloads have None names; records use Some (LANG-12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: SharedSlice<(Option<String>, TypeId)>,
}
/// Declared invocation or resource-finalizer allocation promise (MEM-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Allocation {
    MayAllocate,
    NoAlloc,
}
/// Cleanup conjunction; an unknown/erased proof conservatively permits allocation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Default)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
            (Self::Known(mut a), Self::Known(b)) => {
                a.extend(b);
                a.sort();
                a.dedup();
                Self::Known(a)
            }
            _ => Self::Unknown,
        }
    }
    pub fn inert() -> Self {
        Self::Known(vec![].into())
    }
}
/// An omitted uses clause is not an empty restriction (CAP-3, CLO-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Uses {
    Unrestricted,
    Only(AuthoritySummary),
}
/// Scope lifetime of library-declared capabilities (MODE-6, CAP-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityKind {
    Global,
    Scoped,
}
/// Generic declaration with stable ordinal IDs, not symbols from another session (GEN-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub name: Option<String>,
    pub label: Option<String>,
    pub passing: PassingMode,
    pub ty: TypeId,
}
/// Signature-owned code identity. Contract code is queried with its owner and ordinal (FACT-1, CT-2).
/// Hash includes resolved names and referenced signatures, excludes locations and arena IDs.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Contract {
    pub kind: ContractKind,
    pub ordinal: u32,
    pub code_hash: ContentHash,
}
/// FFI declarations are trusted but explicit and monomorphic (PROG-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ForeignContract {
    pub abi: String,
    pub blocking: bool,
    /// The extern's doc comment.
    pub aliasing_contract: String,
}
/// A safe wrapper's trusted kind and model (PROG-4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignWrapper {
    pub kind: WrapperKind,
    /// Its results and effects are a function of its arguments, so replay
    /// may run it again.
    pub deterministic: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WrapperKind {
    /// The result depends only on the arguments.
    Pure,
    /// It acts on foreign state reachable from the handles it is given.
    Handle,
    /// It acts on the host outside Hirsel's capabilities.
    Effect,
}
/// Complete callable signature. Error is None for non-Result results, otherwise matches Result's E (LANG-6/8).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
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
    store: &mut TypeStore,
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
        parameter.ty = store.push(row);
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
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Layout {
    pub c: bool,
    pub packed: bool,
    pub align: Option<u32>,
    pub soa: bool,
    pub flat: bool,
}
/// Public type declaration signature, including opaque guarantees and destruction (GEN-1, DROP-2).
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub layout: Layout,
    pub distinct: bool,
    pub resource: bool,
    pub drop: Option<DefPath>,
    pub fact: Option<Contract>,
}
/// Every definition exposes one signature category (LANG-6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignatureKind {
    Function(FunctionType),
    Type(TypeDefinition),
    Const(TypeId),
}
mod parameters;
pub use parameters::*;
mod graph;
pub use graph::*;
mod store;
pub use store::*;
/// Self-contained cross-query signature. Importers remap TypeIds into their own store (LANG-6).
/// Fingerprints are of canonical semantic encoding, never Rust Debug or local IDs (PROG-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    pub metadata: SignatureMetadata,
    pub definition: DefPath,
    pub types: TypeStore,
    pub kind: SignatureKind,
    pub fingerprint: ContentHash,
}
/// Cheap immutable signature snapshot across provider consumers.
pub type SignatureRef = std::sync::Arc<Signature>;

/// A resolved name is a binding, primitive, nominal definition or contextual variant (LANG-4).
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    pub reads: Vec<DependencyRead>,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UniqueAction {
    AlreadyUnique,
    DeepCopy,
}
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardedPatternUse {
    pub pattern: PatternId,
    pub test: UseKind,
    pub commit: UseKind,
}
/// Capture creation context shared by authority, ownership and lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureContext {
    Scoped,
    OwningScoped,
    Escaping,
    Task,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureMode {
    pub expression: ExprId,
    pub ordinal: u32,
    pub owner: NameSite,
    pub usage: UseKind,
    pub context: CaptureContext,
}
/// Complete static decisions; cleanup lowering still determines ordered drops.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnershipFacts {
    /// Plain rows in the final checked type store; K omits their cleanup.
    pub plain_types: std::collections::BTreeSet<TypeId>,
    /// Rows whose values have no user-visible destruction (DROP-1): copyable
    /// and not once. Destroying a holder of any other value, including rows
    /// appended after this pass, is a use of its loans (LANG-3).
    pub silent_drop_types: std::collections::BTreeSet<TypeId>,
    /// Borrowed call arguments whose result relation names only a field path
    /// `F` of their parameter, with the type of the argument place extended
    /// by `F` (#77). Lowering alone reads it, to loan that field place for
    /// the result instead of the whole argument.
    pub field_loans: Vec<(ExprId, TypeId)>,
    pub captures: Vec<CaptureMode>,
    /// Callee consumption, independent of evaluating the source operand.
    pub call_arguments: Vec<(ExprId, ExprId, UseKind)>,
    /// J introductions that I proves can read initialized storage on every incoming path.
    pub identity_joins: Vec<(NameSite, NameSite, Origin)>,
    pub identity_fields: Vec<(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)>,
    /// Declaration-owned physical projections, separate from expression receipt modes.
    pub physical_projections: Vec<(ExprId, TypeId)>,
    pub unique: Vec<UniqueDecision>,
    pub guarded_patterns: Vec<GuardedPatternUse>,
    /// Assignment sites that detach receiver storage into allocation.
    pub allocating_mutations: Vec<ExprId>,
    /// Receiving content contract and I-proven storage at the write: Some(true) live, Some(false) dead, None mixed.
    pub replacements: Vec<(ExprId, TypeId, Option<bool>)>,
}

/// Lowering must obey checker decisions instead of inferring ownership again (LANG-3, OWN-8).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Elaboration {
    pub expression: ExprId,
    pub ty: TypeId,
    pub usage: UseKind,
    pub instantiated_arguments: Vec<GenericArgument>,
}
/// Recorded generic call; TypeIds refer to the enclosing CheckedBody.types (GEN-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instantiation {
    pub expression: ExprId,
    pub callee: DefPath,
    pub arguments: Vec<GenericArgument>,
}
/// Explicit capture selected at closure construction, independent of uses inside its body (CLO-1/3/4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub binding: ResolvedName,
    pub usage: UseKind,
    pub ty: TypeId,
}
/// Mode-checked environment layout in capture evaluation order (MODE-5, LANG-3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureInfo {
    /// Complete body result, independently of the contextual callable promise.
    pub result_evidence: Option<TypeId>,
    pub expression: ExprId,
    pub captures: Vec<Capture>,
    pub ty: TypeId,
}
/// Binding identity is scoped by its declaring function; resolver locals are not TIR slots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingDecision {
    pub owner: NameSite,
    pub binding: ResolvedName,
    pub site: NameSite,
    pub ty: TypeId,
    pub usage: UseKind,
    pub mutable: bool,
}
/// Explicit pattern decision; children map source patterns to declaration-order fields.
#[derive(Clone, Debug, Eq, PartialEq)]
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
/// The mode axes a `let` annotation writes (MODE-1/2/3). A written axis is the
/// receiving contract; an unwritten one follows the received value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternDecisionKind {
    Wildcard,
    Binding(LocalId),
    Literal,
    Tuple,
    Constructor(ContextualName),
    Error,
}
/// A checked source storage path. Value roots cover computed receivers as well
/// as expressions which do not designate a binding's storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePlace {
    pub expression: ExprId,
    pub root: SourceRoot,
    pub projections: SharedPath<SourceProjection>,
    pub access: SourceAccess,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceRoot {
    Binding {
        owner: NameSite,
        binding: ResolvedName,
    },
    Definition(DefPath),
    Value(ExprId),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceProjection {
    /// The source node performing this step; dotted path steps share a node.
    pub expression: ExprId,
    pub kind: SourceProjectionKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceProjectionKind {
    Field(FieldId),
    VariantField { variant: VariantId, field: FieldId },
    Index(ExprId),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceAccess {
    Read,
    Copy,
    Take,
    Mut,
    Unique,
}
/// One H-checked call, keyed by its retained source expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedCall {
    pub expression: ExprId,
    pub target: CallTarget,
    /// Written evaluation order, independently of declaration order.
    pub arguments: Vec<CheckedArgument>,
    pub generic_arguments: Vec<GenericArgument>,
    /// Declared result requirement before actual value-flow refinement.
    pub result: TypeId,
    pub state: CallState,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedArgument {
    pub expression: ExprId,
    /// Missing only for an unmapped recovery operand; never a positional fallback.
    pub parameter: Option<(u32, Parameter)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallTarget {
    Definition(DefPath),
    Value(ExprId),
    Constructor(ContextualName),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallState {
    Complete,
    MissingCapabilities,
    Recovery,
}
/// Source-authorized loop conversion, independently checked after lowering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckedTailKind {
    SelfTail,
    ConstructorHole(Vec<u32>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedTail {
    pub call: ExprId,
    pub kind: CheckedTailKind,
    /// Declaration ordinal and source expression of a single-use counted owner.
    pub owned_loop_arguments: Vec<(ParamId, ExprId)>,
}
/// Typechecked body tables keyed by source expression; mode checking enriches these tables (MODE-7).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedBody {
    pub places: Vec<SourcePlace>,
    pub calls: Vec<CheckedCall>,
    pub tails: Vec<CheckedTail>,
    /// Every declared binding, including unused locals/parameters and closure binders.
    pub bindings: Vec<BindingDecision>,
    /// Every pattern, including wildcards and nested unused patterns.
    pub patterns: Vec<PatternDecision>,
    pub types: TypeStore,
    pub resolution: Resolution,
    pub expressions: Vec<Elaboration>,
    pub instantiations: Vec<Instantiation>,
    pub closures: Vec<ClosureInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_result_and_authority_remain_in_signature() {
        let mut store = TypeStore::default();
        let ty = store.push(Type {
            drop_allocation: DropAllocation::MayAllocate,
            modes: Modes {
                relation: Relation::parameters([ParameterRef::bound(ParamId(0))]),
                ..Modes::global()
            },
            ..Type::with_kind(TypeKind::Primitive(Primitive::Bytes))
        });
        let f = FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: ty,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::inert(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        };
        assert!(matches!(
            destination(&store.get(f.result).unwrap().modes.relation, |_| false),
            Destination::Caller(_)
        ));
    }
}

/// Query failures distinguish unavailable implementation from language diagnostics (LANG-6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryError {
    /// Transient host/provider failure, never a deterministic rejection cache entry.
    Operational(OperationalFailure),
    NotImplemented,
    Diagnostics(Vec<Diagnostic>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug)]
pub struct QueryOutput<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
    pub dependencies: Vec<SignatureDependency>,
}
/// A used signature, even for a rejected body; sorted by DefPath in cache keys (LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SignatureDependency {
    pub definition: DefPath,
    pub fingerprint: ContentHash,
}

/// One lookup records a manifest dependency even when resolution failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LookupOutput {
    pub manifest: ContentHash,
    pub result: Result<DefPath, QueryError>,
}
/// Name lookup reads manifest identities, never another body's inferred facts (LANG-4/6).
/// Failed lookups must record the manifest hash, so adding a name invalidates failures.
pub trait SignatureProvider {
    fn signature(&mut self, definition: &DefPath) -> Result<SignatureRef, QueryError>;
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
        symbols: &Interner,
    ) -> LookupOutput;
}

#[cfg(test)]
mod property_tests;
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

pub mod logical;

/// Shared fixture builders, available only with the `testing` feature.
#[cfg(feature = "testing")]
pub mod testing;

mod snapshot;
