//! Canonical `hirsel-codec` encodings of TIR, its type graphs and signatures.
//! Every impl is under the export context [`Cx`],
//! which owns the per-definition interned tables; nothing session-local
//! (`FileId`, byte offsets, `TypeId` provenance, file-arena syntax ids) reaches
//! the text.
use crate::Location;
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer, snapshot_enum, snapshot_struct};
use hirsel_diag::RuleId;
use hirsel_span::{FileId, Span};
use hirsel_syntax::{
    BinaryOp, Bound, ContractKind, ExprId, ItemId, PassingMode, PatternId, StmtId, TypeExprId,
    UnaryOp, Visibility,
};
use hirsel_tir::{
    Block, BlockId, Body, BodyId, CallArgument, CallReturn, Callee, Cleanup, CleanupAction,
    CleanupId, CleanupNode, CleanupNodeId, Constant, ConstructField, Constructor, Definition,
    Instruction, InstructionKind, LoanId, Local, LocalId, MatchArm, MatchPattern, MatchPatternId,
    MatchPatternKind, MatchTable, MatchTableId, Materialized, Operand, OriginBindingKind, Place,
    PlaceId, Projection, ProtocolAssociationReceipt, Rvalue, RvalueId, Scope, ScopeId, Storage,
    TailKind, TailSite, Terminator, ValueKind, ValueNode,
};
use hirsel_types::{
    Allocation, AllocationTarget, AuthoritySummary, AuthorityTerm, BinderRef, Brand,
    CapabilityKind, Contract, Destination, DropAllocation, ErasedCaptures, Field, FieldId,
    ForeignContract, ForeignWrapper, FunctionType, GenericArgument, GenericParameter, HeapRole,
    Layout, LoopSourceDeclaration, Modes, Multiplicity, NameSite, NominalRoles, Origin, OriginRoot,
    ParamId, Parameter, ProtocolAssociation, Reach, RegionId, Relation, RelationPlace,
    ScalarDomain, SchemaConstraint, Sendability, SendabilityRequirement, SharedPath, SharedSlice,
    Signature, SignatureDependency, SignatureKind, SignatureMetadata, Size, SizeParamId,
    StatePromise, Type, TypeDefinition, TypeId, TypeKind, TypeParamId, TypeStore, Uniqueness,
    UseKind, Uses, Variant, VariantId, WrapperKind,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

/// Maps a session span to its portable source site.
pub type Locator = Rc<dyn Fn(Span) -> Location>;

/// Encoder/decoder state. A definition section is written twice: the collect
/// pass interns its locations and syntax ids, the emit pass writes indices into
/// the finished tables, so tables precede the TIR that refers to them.
pub struct Cx {
    pub(crate) locator: Locator,
    pub(crate) collecting: bool,
    pub(crate) locations: Vec<Location>,
    location_ids: HashMap<Location, u32>,
    /// Per syntax-id kind: file-arena id -> order-preserving per-section rank.
    syntax_seen: [BTreeSet<u32>; SYNTAX_KINDS],
    syntax_rank: [BTreeMap<u32, u32>; SYNTAX_KINDS],
    /// Ids of the type store being decoded.
    type_ids: Vec<TypeId>,
    /// Decode: the definition section ordinal, carried as the decoded span file.
    pub(crate) section: u32,
    /// Collect: every definition path the section names (PS-6 references).
    pub(crate) references: BTreeSet<hirsel_syntax::DefPath>,
}

const SYNTAX_KINDS: usize = 5;
const EXPR: usize = 0;
const PATTERN: usize = 1;
const TYPE_EXPR: usize = 2;
const STMT: usize = 3;
const ITEM: usize = 4;

impl Cx {
    pub fn new(locator: Locator) -> Self {
        Self {
            locator,
            collecting: false,
            locations: vec![],
            location_ids: HashMap::new(),
            syntax_seen: Default::default(),
            syntax_rank: Default::default(),
            type_ids: vec![],
            section: 0,
            references: BTreeSet::new(),
        }
    }
    /// Start a definition section: empty tables, collect pass.
    pub(crate) fn begin_collect(&mut self) {
        self.collecting = true;
        self.references.clear();
        self.locations.clear();
        self.location_ids.clear();
        self.syntax_seen = Default::default();
        self.syntax_rank = Default::default();
    }
    /// Freeze the collected tables for the emit pass.
    pub(crate) fn begin_emit(&mut self) {
        self.collecting = false;
        for (kind, seen) in self.syntax_seen.iter().enumerate() {
            self.syntax_rank[kind] = seen
                .iter()
                .enumerate()
                .map(|(rank, id)| (*id, rank as u32))
                .collect();
        }
    }
    /// Leave a section: later values use no section tables.
    pub(crate) fn end_section(&mut self) {
        self.collecting = false;
        self.locations.clear();
        self.location_ids.clear();
        self.syntax_seen = Default::default();
        self.syntax_rank = Default::default();
    }
    fn location(&mut self, span: Span) -> u32 {
        let location = (self.locator)(span);
        if let Some(id) = self.location_ids.get(&location) {
            return *id;
        }
        let id = self.locations.len() as u32;
        self.location_ids.insert(location.clone(), id);
        self.locations.push(location);
        id
    }
    fn syntax(&mut self, kind: usize, id: u32) -> u32 {
        if self.collecting {
            self.syntax_seen[kind].insert(id);
            0
        } else {
            *self.syntax_rank[kind]
                .get(&id)
                .expect("syntax id outside its section's collect pass")
        }
    }
}

snapshot_struct!(Location {
    module,
    line,
    column,
    end_line,
    end_column
} in Cx);

impl Snapshot<Cx> for Span {
    fn write(&self, w: &mut Writer<Cx>) {
        let id = w.context.location(*self);
        w.uint(id.into());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let id = u32::read(r)?;
        if id as usize >= r.context.locations.len() {
            return Err(r.error(format!("location {id} is outside the location table")));
        }
        Ok(Span {
            file: FileId(r.context.section),
            start: id,
            end: id,
        })
    }
}

impl Snapshot<Cx> for RuleId {
    fn write(&self, w: &mut Writer<Cx>) {
        w.text(self.as_str());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let text = r.text()?;
        RuleId::new(text).map_err(|_| r.error("invalid rule id"))
    }
}

macro_rules! index {
    ($($ty:ident),* $(,)?) => {$(
        impl Snapshot<Cx> for $ty {
            fn write(&self, w: &mut Writer<Cx>) {
                w.uint(self.0.into());
            }
            fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
                Ok(Self(u32::read(r)?))
            }
        }
    )*};
}
index!(
    LocalId,
    BlockId,
    PlaceId,
    ScopeId,
    CleanupId,
    CleanupNodeId,
    BodyId,
    LoanId,
    RvalueId,
    MatchTableId,
    MatchPatternId,
    TypeParamId,
    SizeParamId,
    ParamId,
    RegionId,
    FieldId,
    VariantId,
);
impl Snapshot<Cx> for hirsel_types::LocalId {
    fn write(&self, w: &mut Writer<Cx>) {
        w.uint(self.0.into());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        Ok(Self(u32::read(r)?))
    }
}

macro_rules! syntax_index {
    ($($ty:ident => $kind:ident),* $(,)?) => {$(
        impl Snapshot<Cx> for $ty {
            fn write(&self, w: &mut Writer<Cx>) {
                let rank = w.context.syntax($kind, self.0);
                w.uint(rank.into());
            }
            fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
                Ok(Self(u32::read(r)?))
            }
        }
    )*};
}
syntax_index!(
    ExprId => EXPR,
    PatternId => PATTERN,
    TypeExprId => TYPE_EXPR,
    StmtId => STMT,
    ItemId => ITEM,
);

impl Snapshot<Cx> for TypeId {
    fn write(&self, w: &mut Writer<Cx>) {
        w.uint(self.index() as u128);
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let index = u32::read(r)?;
        r.context
            .type_ids
            .get(index as usize)
            .copied()
            .ok_or_else(|| r.error(format!("type {index} is outside its type table")))
    }
}

impl Snapshot<Cx> for TypeStore {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("TypeStore");
        w.uint(self.len() as u128);
        for (_, row) in self.iter() {
            row.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("TypeStore")?;
        let count = u32::read(r)? as usize;
        // Every row takes at least six bytes of text: refuse forged counts
        // before allocating.
        if count > r.remaining() / 6 {
            return Err(r.error("type table is longer than its input"));
        }
        let mut failure = None;
        let store = TypeStore::from_graph(count, |ids| {
            r.context.type_ids = ids.to_vec();
            let mut rows = Vec::with_capacity(count);
            for _ in 0..count {
                match Type::read(r) {
                    Ok(row) => rows.push(row),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            rows.resize_with(count, Type::unit);
            rows
        });
        if let Some(error) = failure {
            return Err(error);
        }
        r.close()?;
        Ok(store)
    }
}

impl<T: Snapshot<Cx>> Snapshot<Cx> for SharedSlice<T> {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("list");
        for item in self.iter() {
            item.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        Ok(Vec::<T>::read(r)?.into())
    }
}

impl<T: Snapshot<Cx> + Clone> Snapshot<Cx> for SharedPath<T> {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("list");
        for item in self.to_vec() {
            item.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        Ok(Vec::<T>::read(r)?.into())
    }
}

impl<P: Snapshot<Cx>> Snapshot<Cx> for BinderRef<P> {
    fn write(&self, w: &mut Writer<Cx>) {
        match self {
            Self::Bound { depth, parameter } => {
                w.open("Bound");
                depth.write(w);
                parameter.write(w);
            }
            Self::Body { scope, parameter } => {
                w.open("Body");
                scope.write(w);
                parameter.write(w);
            }
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let value = match r.open_any()? {
            "Bound" => Self::Bound {
                depth: u32::read(r)?,
                parameter: P::read(r)?,
            },
            "Body" => Self::Body {
                scope: NameSite::read(r)?,
                parameter: P::read(r)?,
            },
            other => return Err(r.error(format!("unknown BinderRef variant {other}"))),
        };
        r.close()?;
        Ok(value)
    }
}

impl Snapshot<Cx> for Relation {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("Relation");
        self.reaches().to_vec().write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("Relation")?;
        let reaches = Vec::<Reach>::read(r)?;
        r.close()?;
        Ok(Relation::new(reaches))
    }
}

// --- shared syntax enums -------------------------------------------------
snapshot_enum!(Bound {
    Plain,
    Flat,
    Data,
    Key,
    Sendable,
    Inert,
    Copyable,
    NoallocDrop,
    Serializable
} in Cx);
snapshot_enum!(ContractKind { Requires, Ensures } in Cx);
snapshot_enum!(PassingMode { Borrow, Take, Mut } in Cx);
snapshot_enum!(Visibility {
    Private,
    Public,
    PublicOpaque
} in Cx);
snapshot_enum!(BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Range,
    RangeInclusive
} in Cx);
snapshot_enum!(UnaryOp {
    Neg,
    Not,
    BitNot,
    Take,
    Copy,
    Mut,
    Unique,
    Box
} in Cx);

// --- semantic types -------------------------------------------------------
snapshot_enum!(NameSite {
    Expression(e),
    Pattern(p),
    Type(t),
    Statement(s),
    Item(i)
} in Cx);
snapshot_enum!(Size { Constant(n), Parameter(p) } in Cx);
snapshot_enum!(GenericArgument { Type(t), Size(s) } in Cx);
snapshot_enum!(OriginRoot {
    ArgumentBrand(p),
    Expression(e),
    Parameter(p),
    Local(l),
    Definition(d),
    Sibling(f)
} in Cx);
snapshot_struct!(Origin { root, fields } in Cx);
snapshot_struct!(Brand { alternatives } in Cx);
snapshot_enum!(Uniqueness { Aliased, Unique } in Cx);
snapshot_enum!(Multiplicity { Many, Once } in Cx);
snapshot_enum!(Sendability {
    Sendable,
    NotSendable,
    DependsOn(parameters)
} in Cx);
snapshot_enum!(Reach {
    Place(origin),
    Environment,
    Region(region)
} in Cx);
snapshot_struct!(Modes {
    relation,
    uniqueness,
    multiplicity,
    sendability
} in Cx);
snapshot_enum!(SendabilityRequirement { None, Sendable } in Cx);
snapshot_enum!(AllocationTarget { Heap } in Cx);
snapshot_enum!(Allocation { MayAllocate, NoAlloc } in Cx);
snapshot_enum!(DropAllocation {
    NoAlloc,
    MayAllocate,
    DependsOn(parameters)
} in Cx);
snapshot_enum!(AuthorityTerm {
    Capability(path),
    Parameter(parameter),
    TypeParameter(parameter)
} in Cx);
snapshot_enum!(AuthoritySummary { Known(terms), Unknown } in Cx);
snapshot_enum!(Uses { Unrestricted, Only(summary) } in Cx);
snapshot_enum!(CapabilityKind { Global, Scoped } in Cx);
snapshot_struct!(RelationPlace { parameter, fields } in Cx);
snapshot_enum!(ErasedCaptures {
    FromArguments(places),
    Known(summary)
} in Cx);
snapshot_struct!(Type {
    sendability_requirement,
    allocation_target,
    drop_allocation,
    kind,
    modes,
    erased_captures,
    brand
} in Cx);
snapshot_struct!(Field { growable, name, ty } in Cx);
snapshot_struct!(Variant { name, fields } in Cx);
snapshot_enum!(GenericParameter {
    Type { name, bounds },
    Size { name }
} in Cx);
snapshot_struct!(Parameter {
    name,
    label,
    passing,
    ty
} in Cx);
snapshot_struct!(Contract {
    kind,
    ordinal,
    code_hash
} in Cx);
snapshot_struct!(ForeignContract {
    abi,
    blocking,
    aliasing_contract
} in Cx);
snapshot_enum!(WrapperKind { Pure, Handle, Effect } in Cx);
snapshot_struct!(ForeignWrapper { kind, deterministic } in Cx);
snapshot_struct!(StatePromise {
    selector,
    advance_only,
    preserve_captures,
    preserve_unique,
    unretained
} in Cx);
snapshot_struct!(FunctionType {
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
    foreign
} in Cx);
snapshot_enum!(TypeKind {
    Primitive(p),
    Tuple(items),
    Parameter(p),
    Named { definition, arguments },
    Record(fields),
    Variant(variants),
    Function(f),
    Box(inner),
    Builtin { kind, arguments },
    Error
} in Cx);
snapshot_struct!(Layout {
    c,
    packed,
    align,
    soa,
    flat
} in Cx);
snapshot_struct!(LoopSourceDeclaration { open, next, option } in Cx);
snapshot_struct!(TypeDefinition {
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
    layout,
    distinct,
    resource,
    drop,
    fact
} in Cx);
snapshot_enum!(SignatureKind {
    Function(f),
    Type(t),
    Const(c)
} in Cx);
snapshot_struct!(HeapRole { arena, operation } in Cx);
snapshot_struct!(NominalRoles {
    unsafe_capability,
    result_type,
    tasks_capability,
    scope_capability,
    heap
} in Cx);
snapshot_enum!(ScalarDomain { Integer, Float, Scalar } in Cx);
snapshot_enum!(SchemaConstraint {
    Bound { parameter, bound, rule },
    Domain { parameter, domain, rule },
    CompleteRangeFits { source, destination },
    DestinationNoWider { source, destination },
    PositiveSize(p),
    NoallocDrop(p)
} in Cx);
snapshot_struct!(SignatureMetadata {
    track_caller,
    visibility,
    alias,
    unsafe_ffi,
    wrapper,
    roles,
    schema
} in Cx);
snapshot_struct!(Signature {
    metadata,
    definition,
    types,
    kind,
    fingerprint
} in Cx);
snapshot_struct!(SignatureDependency { definition, fingerprint } in Cx);
snapshot_enum!(UseKind { Borrow, Take, Copy, Mut } in Cx);
snapshot_enum!(Destination {
    Heap,
    Arena(origin),
    Caller(parameters),
    Region(region),
    Inferred
} in Cx);
snapshot_struct!(ProtocolAssociation { source, fingerprint } in Cx);

// --- TIR -------------------------------------------------------------------
snapshot_enum!(Storage {
    Parameter,
    Local,
    Temporary,
    Capture,
    ReturnSlot
} in Cx);
snapshot_struct!(Local {
    ty,
    storage,
    scope,
    span
} in Cx);
snapshot_enum!(Projection {
    Field(f),
    Index(l),
    VariantField { variant, field }
} in Cx);
snapshot_struct!(Place {
    loan,
    binding,
    root,
    projection,
    ty
} in Cx);
snapshot_struct!(Operand { place, usage } in Cx);
snapshot_enum!(Constant {
    Unit,
    Bool(b),
    Integer(n),
    F32(bits),
    F64(bits),
    String(s),
    Bytes(b)
} in Cx);
snapshot_enum!(Constructor {
    Tuple,
    Record(definition),
    Variant { definition, variant },
    Sequence,
    Map,
    Set,
    Box
} in Cx);
snapshot_struct!(ConstructField { field, value } in Cx);
snapshot_struct!(CallArgument { parameter, value } in Cx);
snapshot_enum!(Rvalue {
    Constant(c),
    ReadConstant { definition },
    FunctionReference { definition, arguments },
    Size(s),
    Use(o),
    Unary { op, value },
    Binary { op, left, right },
    Construct { constructor, fields, destination },
    Closure { body, captures, destination },
    Shape(t)
} in Cx);
snapshot_enum!(OriginBindingKind { Alias, Fresh } in Cx);
snapshot_enum!(InstructionKind {
    Defuse { place },
    BindOrigin { source, place, kind },
    BindFields { source, fields },
    EndOrigin(p),
    StorageLive(l),
    StorageDead { place, if_live },
    EndScope(s),
    Assign { destination, value },
    Replace { destination, value, displaced, if_live },
    BeginLoan { id, place, mutable, if_inactive },
    EndLoan { loan, if_active },
    Drop { place, if_live },
    RunCleanup(c),
    Check { condition, rule },
    Snapshot { destination, value }
} in Cx);
snapshot_struct!(Instruction {
    kind,
    span,
    on_failure
} in Cx);
snapshot_enum!(Callee {
    Definition { path, arguments },
    Indirect { forward_caller, value, arguments }
} in Cx);
snapshot_struct!(CallReturn { destination, next } in Cx);
snapshot_enum!(Terminator {
    Goto(b),
    Match { table, from },
    Branch { condition, yes, no },
    Switch { value, arms, otherwise },
    Call { callee, arguments, returns, on_failure },
    Return { value, cleanup },
    Fail { rule, cleanup },
    Unreachable
} in Cx);
snapshot_struct!(Block {
    instructions,
    terminator,
    span
} in Cx);
snapshot_struct!(Scope { parent, span } in Cx);
snapshot_enum!(CleanupAction {
    Drop { place, if_live },
    EndLoan { loan, if_active },
    ReleaseRegion(r),
    CloseArena(p),
    JoinTasks { scope, cancel }
} in Cx);
snapshot_struct!(Cleanup { head } in Cx);
snapshot_struct!(CleanupNode { action, parent } in Cx);
snapshot_enum!(TailKind {
    SelfTail,
    ConstructorHole(path)
} in Cx);
snapshot_struct!(TailSite {
    owner_body,
    call_block,
    kind,
    owned_loop_arguments
} in Cx);
snapshot_struct!(Body {
    parameter_scope,
    locals,
    places,
    scopes,
    blocks,
    rvalues,
    cleanups,
    cleanup_nodes,
    entry,
    parameters,
    result,
    tail_sites,
    matches
} in Cx);
snapshot_struct!(MatchTable {
    scrutinee,
    patterns,
    arms
} in Cx);
snapshot_struct!(MatchArm {
    pattern,
    guard,
    target
} in Cx);
snapshot_struct!(MatchPattern { place, kind } in Cx);
snapshot_enum!(MatchPatternKind {
    Wildcard,
    Binding { usage, guard_usage },
    Literal(c),
    Range { start, end },
    Tuple(fields),
    Record { definition, fields },
    Variant { definition, variant, fields }
} in Cx);
snapshot_struct!(ProtocolAssociationReceipt {
    association,
    body,
    open,
    next
} in Cx);
snapshot_struct!(Definition {
    caller_bodies,
    protocol_associations,
    types,
    path,
    signature_hash,
    bodies,
    entry,
    contracts
} in Cx);
snapshot_struct!(Materialized { types, root } in Cx);
snapshot_struct!(ValueNode { ty, kind } in Cx);
snapshot_enum!(ValueKind {
    Constant(c),
    FunctionReference(path),
    Construct { constructor, fields }
} in Cx);
