//! Canonical `hirsel-codec` encodings of TIR, its type graphs and signatures.
//! Every impl is under the export context [`Cx`],
//! which owns the per-definition interned tables; nothing session-local
//! (anchors, byte offsets, interner `TypeId`s, raw syntax ids, `DefId`s)
//! reaches the text; lexical owners are written as (DefPath, occurrence).
//! A definition, signature or materialized value writes its canonical type
//! table first: the closure of the TypeIds it names, in the order its fields
//! name them, each TypeId after it written as a position in that table.
use crate::Location;
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer, snapshot_enum, snapshot_struct};
use hirsel_diag::RuleId;
use hirsel_span::{Anchor, Span};
use hirsel_syntax::{
    BinaryOp, Bound, ContentHash, ContractKind, DefPath, ExprId, LexicalOwner, PassingMode,
    PatternId, SiteRef, StmtId, TypeExprId, UnaryOp, Visibility,
};
use hirsel_tir::{
    ArenaScopeGrant, Block, BlockId, Body, BodyId, BorrowCursor, CallArgument, CallReturn, Callee,
    Cleanup, CleanupAction, CleanupId, CleanupNode, CleanupNodeId, Constant, ConstructField,
    Constructor, Definition, Instruction, InstructionKind, LoanId, Local, LocalId, MatchArm,
    MatchPattern, MatchPatternId, MatchPatternKind, MatchTable, MatchTableId, Materialized,
    Operand, OriginBindingKind, Place, PlaceId, Projection, ProtocolAssociationReceipt, Rvalue,
    RvalueId, Scope, ScopeId, ScopedParameter, SourceEvent, SourceEventKind, Storage, TailKind,
    TailSite, Terminator, ValueKind, ValueNode,
};
use hirsel_types::{
    Allocation, AllocationTarget, AuthoritySummary, AuthorityTerm, BinderRef, Brand,
    CapabilityKind, CaptureContext, Contract, Destination, DropAllocation, ErasedCaptures,
    FamilyMember, FamilyMemberKind, FamilySignature, Field, FieldId, ForeignContract,
    ForeignWrapper, FunctionType, GenericArgument, GenericParameter, HeapRole,
    ImplementationBinder, ImplementationSignature, Layout, LoopSourceDeclaration, MemberActual,
    Modes, Multiplicity, NameSite, NominalRoles, Origin, OriginRoot, ParamId, Parameter,
    ProtocolAssociation, Reach, RegionId, Relation, RelationPlace, ScalarDomain, SchemaConstraint,
    Sendability, SendabilityRequirement, SerializableCheck, SharedPath, SharedSlice, Signature,
    SignatureDependency, SignatureKind, SignatureMetadata, Size, SizeParamId, StatePromise, Type,
    TypeDefinition, TypeId, TypeInterner, TypeKind, TypeParamId, UniqueResultRow, UniqueSummary,
    Uniqueness, UseKind, Uses, Variant, VariantId, WrapperKind,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

/// Maps a session span to its portable source site.
pub type Locator = Rc<dyn Fn(Span) -> Location>;

/// Encoder/decoder state. A definition section is written twice: the collect
/// pass interns its locations and syntax ids, the emit pass writes indices into
/// the finished tables, so tables precede the TIR that refers to them.
pub struct Cx {
    pub(crate) locator: Locator,
    pub(crate) collecting: bool,
    pub(crate) sizing: Option<crate::admission::WireShape>,
    pub(crate) capture_roots: Option<Vec<TypeId>>,
    pub(crate) captured_types: Option<Arc<std::collections::HashSet<TypeId>>>,
    pub(crate) section_types: Option<Arc<[TypeId]>>,
    pub(crate) locations: Vec<Location>,
    location_ids: HashMap<Location, u32>,
    /// Per syntax-id kind: file-arena id -> order-preserving per-section rank.
    syntax_seen: [BTreeSet<u32>; SYNTAX_KINDS],
    syntax_rank: [BTreeMap<u32, u32>; SYNTAX_KINDS],
    /// The session interner: rows are read from and decoded into it.
    pub(crate) types: Arc<TypeInterner>,
    /// Encode: the current type table's position of each TypeId.
    type_index: HashMap<TypeId, u32>,
    /// Decode: the session id of each position of the current type table.
    type_ids: Vec<TypeId>,
    /// A probe pass: the TypeIds written, in order, instead of positions.
    probe: Option<Vec<TypeId>>,
    /// Decode: the definition section ordinal, carried as the decoded span file.
    pub(crate) section: u32,
    /// Collect: every definition path the section names (PS-6 references).
    pub(crate) references: BTreeSet<hirsel_syntax::DefPath>,
    /// A definition content hash encoding (LANG-6): references are written
    /// by hash, and spans and local names are erased.
    pub(crate) content: Option<crate::content::Naming>,
}

const SYNTAX_KINDS: usize = 4;
const EXPR: usize = 0;
const PATTERN: usize = 1;
const TYPE_EXPR: usize = 2;
const STMT: usize = 3;

impl Cx {
    pub fn new(locator: Locator, types: Arc<TypeInterner>) -> Self {
        Self {
            locator,
            collecting: false,
            sizing: None,
            capture_roots: None,
            captured_types: None,
            section_types: None,
            locations: vec![],
            location_ids: HashMap::new(),
            syntax_seen: Default::default(),
            syntax_rank: Default::default(),
            types,
            type_index: HashMap::new(),
            type_ids: vec![],
            probe: None,
            section: 0,
            references: BTreeSet::new(),
            content: None,
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
        if self.capture_roots.is_some() {
            return 0;
        }
        if let Some(shape) = &mut self.sizing {
            shape.spans = shape.spans.checked_add(1).unwrap_or(usize::MAX);
            return 0;
        }
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
        if self.capture_roots.is_some() {
            return 0;
        }
        if let Some(shape) = &mut self.sizing {
            shape.syntax[kind] = shape.syntax[kind].checked_add(1).unwrap_or(usize::MAX);
            return 0;
        }
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

/// `snapshot_struct!` under [`Cx`], except that each `field => value` writes
/// `value` instead under a content hash encoding: the field is a name or a
/// name-derived fingerprint, never meaning.
macro_rules! erasing_struct {
    ($ty:ident { $($field:ident $(=> $erased:expr)?),* $(,)? }) => {
        impl Snapshot<Cx> for $ty {
            fn write(&self, w: &mut Writer<Cx>) {
                w.open(stringify!($ty));
                let erase = w.context.content.is_some();
                $(erasing_struct!(@field self, w, erase, $field $(, $erased)?);)*
                w.close();
            }
            fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
                r.open(stringify!($ty))?;
                let value = $ty { $($field: Snapshot::<Cx>::read(r)?,)* };
                r.close()?;
                Ok(value)
            }
        }
    };
    (@field $self:ident, $w:ident, $erase:ident, $field:ident) => {
        Snapshot::<Cx>::write(&$self.$field, $w);
    };
    (@field $self:ident, $w:ident, $erase:ident, $field:ident, $erased:expr) => {
        if $erase {
            Snapshot::<Cx>::write(&$erased, $w);
        } else {
            Snapshot::<Cx>::write(&$self.$field, $w);
        }
    };
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
        if w.context.content.is_some() {
            w.uint(0);
            return;
        }
        let id = w.context.location(*self);
        w.uint(id.into());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let id = u32::read(r)?;
        if id as usize >= r.context.locations.len() {
            return Err(r.error(format!("location {id} is outside the location table")));
        }
        Ok(Span {
            anchor: Anchor(r.context.section),
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
);

/// A TypeId no table row has, as written; reading it is refused.
const MISSING: u32 = u32::MAX;

impl Snapshot<Cx> for TypeId {
    fn write(&self, w: &mut Writer<Cx>) {
        if let Some(roots) = &mut w.context.capture_roots {
            roots.push(*self);
            w.uint(0);
            return;
        }
        if let Some(shape) = &mut w.context.sizing {
            shape.unloaded |= *self != TypeId::DANGLING
                && (w.context.types.get_if_loaded(*self).is_none()
                    || w.context
                        .captured_types
                        .as_ref()
                        .is_some_and(|rows| !rows.contains(self))
                    || w.context
                        .section_types
                        .as_ref()
                        .is_some_and(|rows| !rows.contains(self)));
            shape.types = shape.types.checked_add(1).unwrap_or(usize::MAX);
            w.uint(0);
            return;
        }
        if let Some(probe) = &mut w.context.probe {
            probe.push(*self);
            w.uint(0);
            return;
        }
        let position = w.context.type_index.get(self).copied().unwrap_or(MISSING);
        w.uint(position as u128);
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

/// The TypeIds `write` names, in the order it names them: a scratch pass
/// in the same (content or wire) mode that leaves `w` untouched.
fn probe(w: &Writer<Cx>, write: impl FnOnce(&mut Writer<Cx>)) -> Vec<TypeId> {
    let mut cx = Cx::new(w.context.locator.clone(), w.context.types.clone());
    cx.collecting = true;
    cx.probe = Some(vec![]);
    if w.context.content.is_some() {
        cx.content = Some(Default::default());
    }
    let mut scratch = Writer::discarding(cx);
    write(&mut scratch);
    scratch.finish().1.probe.unwrap_or_default()
}

/// Write the canonical table of everything `write` names, then `write` with
/// its TypeIds as positions in that table. The table stays current for the
/// rest of the section.
pub(crate) fn tabled(w: &mut Writer<Cx>, write: impl Fn(&mut Writer<Cx>)) {
    if w.context.sizing.is_some() || w.context.capture_roots.is_some() {
        write(w);
        return;
    }
    let types = w.context.types.clone();
    let order = hirsel_types::closure(&types, probe(w, &write));
    w.context.type_index = order
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect();
    w.open("TypeTable");
    w.uint(order.len() as u128);
    for id in order.iter() {
        types.row(*id).write(w);
    }
    w.close();
    write(w);
}

/// Read a canonical type table into the session interner; later positions
/// read as its rows' session ids. Rows equal to existing rows get their ids.
pub(crate) fn read_table(r: &mut Reader<'_, Cx>) -> Result<(), DecodeError> {
    r.open("TypeTable")?;
    let count = u32::read(r)? as usize;
    // Every row takes at least six bytes of text: refuse forged counts
    // before allocating.
    if count > r.remaining() / 6 {
        return Err(r.error("type table is longer than its input"));
    }
    let scratch = TypeInterner::new(None);
    let mut failure = None;
    let ids = scratch.intern_graph(count, |ids| {
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
    let (table, positions) = hirsel_types::encode_table(&scratch, &ids);
    let decoded = hirsel_types::decode_table(&r.context.types, &table);
    r.context.type_ids = positions
        .iter()
        .map(|p| {
            decoded
                .get(*p as usize)
                .copied()
                .unwrap_or(TypeId::DANGLING)
        })
        .collect();
    Ok(())
}

/// A definition is written as its DefPath, never its session id.
impl Snapshot<Cx> for hirsel_syntax::DefId {
    fn write(&self, w: &mut Writer<Cx>) {
        self.path().write(w);
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        Ok(hirsel_syntax::DefId::intern(&DefPath::read(r)?))
    }
}
/// A name is written as its text.
impl Snapshot<Cx> for hirsel_intern::Symbol {
    fn write(&self, w: &mut Writer<Cx>) {
        w.text(self.as_str());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        Ok(hirsel_intern::Symbol::intern(&String::read(r)?))
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
        if w.context.capture_roots.is_some() {
            for index in 0..self.len() {
                self.get(index).expect("path index").write(w);
            }
        } else if let Some(shape) = &mut w.context.sizing {
            shape.path_iter_bytes = self
                .len()
                .checked_mul(size_of::<&T>())
                .and_then(|bytes| shape.path_iter_bytes.checked_add(bytes))
                .unwrap_or(usize::MAX);
            // SharedPath::iter materializes a reverse traversal buffer. Sizing
            // uses indexed access so it needs no temporary allocation.
            for index in 0..self.len() {
                self.get(index).expect("path index").write(w);
            }
        } else {
            for item in self.iter() {
                item.write(w);
            }
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
                scope: SiteRef::read(r)?,
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
        w.open("list");
        for reach in self.reaches() {
            reach.write(w);
        }
        w.close();
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
    Item
} in Cx);
/// An owner is written as its DefPath and occurrence, never its session id.
impl Snapshot<Cx> for LexicalOwner {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("LexicalOwner");
        self.def.path().write(w);
        self.occurrence.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("LexicalOwner")?;
        let path = DefPath::read(r)?;
        let occurrence = u32::read(r)?;
        r.close()?;
        Ok(LexicalOwner {
            def: hirsel_syntax::DefId::intern(&path),
            occurrence,
        })
    }
}
snapshot_struct!(SiteRef { owner, site } in Cx);
snapshot_enum!(Size { Constant(n), Parameter(p) } in Cx);
snapshot_enum!(GenericArgument { Type(t), Size(s) } in Cx);
snapshot_enum!(OriginRoot {
    ArgumentBrand(p),
    Expression(o, e),
    Parameter(p),
    Local(o, l),
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
impl Snapshot<Cx> for GenericParameter {
    fn write(&self, w: &mut Writer<Cx>) {
        // Generic parameters are referenced by position; their names are local.
        let erase = w.context.content.is_some();
        match self {
            Self::Type { name: n, bounds } => {
                w.open("Type");
                w.text(if erase { "" } else { n.as_str() });
                bounds.write(w);
            }
            Self::Size { name: n } => {
                w.open("Size");
                w.text(if erase { "" } else { n.as_str() });
            }
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let value = match r.open_any()? {
            "Type" => Self::Type {
                name: String::read(r)?,
                bounds: SharedSlice::read(r)?,
            },
            "Size" => Self::Size {
                name: String::read(r)?,
            },
            other => return Err(r.error(format!("unknown GenericParameter variant {other}"))),
        };
        r.close()?;
        Ok(value)
    }
}
erasing_struct!(Parameter {
    // A parameter's name is local to its body; its label is its call interface.
    name => None::<String>,
    label,
    passing,
    ty
});
erasing_struct!(Contract {
    kind,
    ordinal,
    // Derived from resolved names; the contract code is in its owner's TIR.
    code_hash => ContentHash::default()
});
erasing_struct!(ForeignContract {
    abi,
    blocking,
    // The extern's doc comment.
    aliasing_contract => String::new()
});
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
    serializable_check,
    layout,
    distinct,
    resource,
    drop,
    fact,
    captures
} in Cx);
snapshot_struct!(SerializableCheck {
    function,
    representation
} in Cx);
snapshot_enum!(FamilyMemberKind { Type { bounds }, Size, Operation } in Cx);
snapshot_struct!(FamilyMember { name, kind } in Cx);
snapshot_struct!(FamilySignature { members } in Cx);
snapshot_enum!(MemberActual { Type(t), Size(n), Operation(p) } in Cx);
snapshot_struct!(ImplementationSignature { family, members } in Cx);
snapshot_struct!(ImplementationBinder { name, family, members } in Cx);
snapshot_enum!(SignatureKind {
    Function(f),
    Type(t),
    Const { ty, generics },
    Family(f),
    Implementation(i)
} in Cx);
snapshot_struct!(HeapRole { arena, operation } in Cx);
/// Under a content hash encoding each role is written as its role name, not its
/// definition: the roles are one environment binding every signature carries,
/// and a body's real use of a role type is a hashed reference in its TIR.
impl Snapshot<Cx> for NominalRoles {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("NominalRoles");
        if w.context.content.is_some() {
            for (role, present) in [
                ("unsafe_capability", self.unsafe_capability.is_some()),
                ("result_type", self.result_type.is_some()),
                ("tasks_capability", self.tasks_capability.is_some()),
                ("scope_capability", self.scope_capability.is_some()),
                ("heap", self.heap.is_some()),
            ] {
                present.then(|| role.to_string()).write(w);
            }
        } else {
            self.unsafe_capability.write(w);
            self.result_type.write(w);
            self.tasks_capability.write(w);
            self.scope_capability.write(w);
            self.heap.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("NominalRoles")?;
        let value = NominalRoles {
            unsafe_capability: Snapshot::<Cx>::read(r)?,
            result_type: Snapshot::<Cx>::read(r)?,
            tasks_capability: Snapshot::<Cx>::read(r)?,
            scope_capability: Snapshot::<Cx>::read(r)?,
            heap: Snapshot::<Cx>::read(r)?,
        };
        r.close()?;
        Ok(value)
    }
}
snapshot_enum!(ScalarDomain { Integer, Float, Scalar } in Cx);
snapshot_enum!(SchemaConstraint {
    Bound { parameter, bound, rule },
    Domain { parameter, domain, rule },
    CompleteRangeFits { source, destination },
    DestinationNoWider { source, destination },
    PositiveSize(p),
    NoallocDrop(p)
} in Cx);
/// A recursive type group member's key follows the schema only when present,
/// so every other signature encodes as before the field existed. A content
/// hash encoding never writes it: the key is over the members' paths and
/// source text, and LANG-6 hashes a recursive group together instead.
impl Snapshot<Cx> for SignatureMetadata {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("SignatureMetadata");
        self.inferred_unique.write(w);
        self.track_caller.write(w);
        self.visibility.write(w);
        self.alias.write(w);
        self.constant_alias.write(w);
        self.unsafe_ffi.write(w);
        self.wrapper.write(w);
        self.roles.write(w);
        self.schema.write(w);
        if let Some(group) = &self.recursive_group
            && w.context.content.is_none()
        {
            group.write(w);
        }
        // Binders follow only when present, so other signatures encode as before;
        // a list opens a node where a group key is bytes.
        if !self.implementations.is_empty() {
            self.implementations.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("SignatureMetadata")?;
        let value = SignatureMetadata {
            inferred_unique: Snapshot::read(r)?,
            track_caller: Snapshot::read(r)?,
            visibility: Snapshot::read(r)?,
            alias: Snapshot::read(r)?,
            constant_alias: Snapshot::read(r)?,
            unsafe_ffi: Snapshot::read(r)?,
            wrapper: Snapshot::read(r)?,
            roles: NominalRoles::read(r)?.into(),
            schema: Snapshot::read(r)?,
            recursive_group: if r.next_kind() == hirsel_codec::Next::Bytes {
                Some(Snapshot::read(r)?)
            } else {
                None
            },
            implementations: if r.at_close() {
                vec![]
            } else {
                Snapshot::read(r)?
            },
        };
        r.close()?;
        Ok(value)
    }
}
impl Snapshot<Cx> for Signature {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("Signature");
        tabled(w, |w| {
            self.metadata.write(w);
            self.definition.write(w);
            self.kind.write(w);
            // Name-sensitive; the content hash covers the signature itself.
            if w.context.content.is_some() {
                ContentHash::default().write(w);
            } else {
                self.fingerprint.write(w);
            }
        });
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("Signature")?;
        read_table(r)?;
        let value = Signature {
            metadata: Snapshot::read(r)?,
            definition: Snapshot::read(r)?,
            kind: Snapshot::read(r)?,
            fingerprint: Snapshot::read(r)?,
        };
        r.close()?;
        Ok(value)
    }
}
snapshot_struct!(SignatureDependency { definition, fingerprint } in Cx);
snapshot_enum!(UseKind { Borrow, Take, Copy, Mut } in Cx);
snapshot_enum!(Destination {
    Heap,
    Arena(origin),
    Caller(parameters),
    Region(region),
    Inferred
} in Cx);
erasing_struct!(ProtocolAssociation {
    source,
    fingerprint => ContentHash::default()
});

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
    SortedMap { by },
    Box
} in Cx);
snapshot_struct!(ConstructField { field, value } in Cx);
snapshot_struct!(CallArgument { parameter, value } in Cx);
snapshot_enum!(Rvalue {
    Constant(c),
    ReadConstant { definition, arguments },
    FunctionReference { definition, arguments },
    Size(s),
    Use(o),
    Unary { op, value },
    Binary { op, left, right },
    Construct { constructor, fields, destination },
    Closure { context, body, captures, destination },
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
    Snapshot { destination, value },
    Charge(e)
} in Cx);
snapshot_enum!(SourceEventKind { Call, Iteration } in Cx);
snapshot_struct!(SourceEvent { kind, source } in Cx);
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
snapshot_struct!(ScopedParameter { parameter, fields } in Cx);
snapshot_struct!(ArenaScopeGrant { intrinsic, callback_parameter, arena_parameter } in Cx);
snapshot_enum!(CaptureContext { Scoped, OwningScoped, Escaping, Task } in Cx);
snapshot_struct!(BorrowCursor { carrier, initializer, cover } in Cx);
snapshot_struct!(Body {
    arena_scope_grant,
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
    borrow_cursors,
    matches,
    scoped_parameters
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
snapshot_enum!(UniqueResultRow {
    Root,
    Tuple(field),
    Payload { variant, field }
} in Cx);
snapshot_struct!(UniqueSummary { preserves_unique, results, nonretaining } in Cx);
/// `types` is not written: it is the definition's closure, recomputed on read.
impl Snapshot<Cx> for Definition {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("Definition");
        tabled(w, |w| {
            self.inferred_unique.write(w);
            self.caller_bodies.write(w);
            self.protocol_associations.write(w);
            self.path.write(w);
            if w.context.content.is_some() {
                ContentHash::default().write(w);
            } else {
                self.signature_hash.write(w);
            }
            self.bodies.write(w);
            self.entry.write(w);
            self.contracts.write(w);
        });
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("Definition")?;
        read_table(r)?;
        let mut value = Definition {
            inferred_unique: Snapshot::read(r)?,
            caller_bodies: Snapshot::read(r)?,
            protocol_associations: Snapshot::read(r)?,
            types: Arc::from([]),
            path: Snapshot::read(r)?,
            signature_hash: Snapshot::read(r)?,
            bodies: Snapshot::read(r)?,
            entry: Snapshot::read(r)?,
            contracts: Snapshot::read(r)?,
        };
        r.close()?;
        value.types = hirsel_tir::definition_types(&r.context.types, &value);
        Ok(value)
    }
}
impl Snapshot<Cx> for Materialized {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("Materialized");
        tabled(w, |w| self.root.write(w));
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("Materialized")?;
        read_table(r)?;
        let root = Snapshot::read(r)?;
        r.close()?;
        Ok(Materialized { root })
    }
}
snapshot_struct!(ValueNode { ty, kind } in Cx);
snapshot_enum!(ValueKind {
    Constant(c),
    FunctionReference { definition, arguments },
    Construct { constructor, fields }
} in Cx);

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_types::{GenericArgument, Primitive, TypeKind, Variant};

    /// A type table's text for a recursive group, a named instance and
    /// repeated roots, after a round trip through a second interner.
    fn tables() -> String {
        let locator: Locator = Rc::new(|_: Span| -> Location { unreachable!("no spans") });
        let t = TypeInterner::new(None);
        let u64 = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let list = t.intern_graph(2, |ids| {
            let field = |name: &str, ty| (Some(hirsel_intern::Symbol::intern(name)), ty);
            vec![
                Type::with_kind(TypeKind::Variant(
                    vec![
                        Variant {
                            name: hirsel_intern::Symbol::intern("Nil"),
                            fields: vec![].into(),
                        },
                        Variant {
                            name: hirsel_intern::Symbol::intern("Cons"),
                            fields: vec![field("head", u64), field("tail", ids[1])].into(),
                        },
                    ]
                    .into(),
                )),
                Type::with_kind(TypeKind::Box(ids[0])),
            ]
        })[0];
        let named = t.intern(Type::with_kind(TypeKind::Named {
            definition: hirsel_syntax::DefId::intern(&DefPath::core("Option")),
            arguments: vec![GenericArgument::Type(list)].into(),
        }));
        let boolean = t.intern(Type::boolean());
        let tuple = t.intern(Type::with_kind(TypeKind::Tuple(
            vec![named, boolean, u64].into(),
        )));
        let roots = [tuple, list, u64, named, tuple];
        let write = |cx: Cx, roots: &[TypeId]| {
            let mut w = Writer::with_context(cx);
            tabled(&mut w, |w| {
                w.uint(roots.len() as u128);
                for root in roots {
                    root.write(w);
                }
            });
            w.finish().0
        };
        let text = write(Cx::new(locator.clone(), t), &roots);
        let other = TypeInterner::new(None);
        let mut r = Reader::with_context(&text, Cx::new(locator.clone(), other.clone()));
        read_table(&mut r).unwrap();
        let count = u32::read(&mut r).unwrap();
        let read: Vec<TypeId> = (0..count).map(|_| TypeId::read(&mut r).unwrap()).collect();
        r.finish().unwrap();
        assert_eq!(read[0], read[4], "a repeated root reads as one id");
        assert_eq!(write(Cx::new(locator, other), &read), text);
        text
    }

    #[test]
    fn type_tables_do_not_depend_on_id_assignment() {
        let Some(runs) = hirsel_intern::under_salts(
            "wire::tests::type_tables_do_not_depend_on_id_assignment",
            &[1, 2, 0x5eed],
            tables,
        ) else {
            return;
        };
        assert!(runs.iter().all(|run| *run == runs[0]), "{runs:#?}");
        assert_eq!(runs[0], tables(), "salted runs agree with the unsalted one");
    }
}

#[cfg(test)]
#[test]
fn definition_header_summary_round_trips_in_written_field_order() {
    let types = TypeInterner::new(None);
    let locator: Locator = Rc::new(|_| Location::unknown());
    for inferred_unique in [
        UniqueSummary::default(),
        UniqueSummary {
            nonretaining: vec![hirsel_types::ParamId(0)],
            preserves_unique: vec![hirsel_types::ParamId(2)],
            results: vec![
                UniqueResultRow::Root,
                UniqueResultRow::Payload {
                    variant: 1,
                    field: 0,
                },
            ],
        },
    ] {
        let definition = Definition {
            inferred_unique,
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: hirsel_syntax::DefId::intern(&DefPath::core("round_trip")),
            signature_hash: ContentHash::default(),
            bodies: vec![],
            entry: BodyId(0),
            contracts: vec![],
        };
        let mut writer = Writer::with_context(Cx::new(locator.clone(), types.clone()));
        definition.write(&mut writer);
        let (text, _) = writer.finish();
        let mut reader = Reader::with_context(&text, Cx::new(locator.clone(), types.clone()));
        let decoded = Definition::read(&mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(decoded, definition);
    }
}
