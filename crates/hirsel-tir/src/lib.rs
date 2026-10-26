//! Explicit ordered ownership IR, before Perceus dup/drop optimization (LANG-3, MEM-1).

#[cfg(test)]
hirsel_guard::install!();

use hirsel_diag::RuleId;
use hirsel_span::Span;
use hirsel_syntax::DefId;
use hirsel_syntax::{BinaryOp, ContentHash, DefPath, UnaryOp};
use hirsel_types::QueryError;
use hirsel_types::{
    Destination, GeneratedActualKey, GenericArgument, TypeId, TypeInterner, UseKind,
};

/// Borrowed-association receipts revised the formerly implicit v1 TIR schema;
/// v3 adds diverging calls; v4 retains named-entry caller attribution;
/// v5 adds the order-bearing SortedMap constructor;
/// v6 adds size actuals to constant reads;
/// v7 qualifies body binders and origins with their owning definition occurrence;
/// v8 retains ordinary generic actuals in declared dictionary constant graphs;
/// v9 transports source scoped selectors and closure classification.
/// v10 adds independently checked fixed-origin borrow cursor claims.
/// v11 adds contextual consumable arena scope entries (MEM-4).
/// v12 binds the combined grant and logical invocation semantics.
/// v13 adds compile-time source-event charges (CT-4).
/// v14 publishes inferred private-helper uniqueness evidence.
pub const TIR_VERSION: u32 = 15;

pub mod representation;

pub use representation::RepresentationRecord;

macro_rules! ids { ($($name:ident),* $(,)?) => {$(
    /// Vec-arena index local to its owning TIR body/program (LANG-3, PROG-5).
    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
    pub struct $name(pub u32);
)*}; }
ids!(
    LocalId,
    BlockId,
    PlaceId,
    ScopeId,
    CleanupId,
    CleanupNodeId,
    BodyId,
    LoanId,
    FailureId,
    RvalueId,
    MatchTableId,
    MatchPatternId
);
/// Logical storage class; evaluated temporaries unwind before named locals (LANG-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Storage {
    Parameter,
    Local,
    Temporary,
    Capture,
    ReturnSlot,
}
/// A declared storage slot with the order needed for resource scope exit (DROP-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Local {
    pub ty: TypeId,
    pub storage: Storage,
    pub scope: ScopeId,
    pub span: Span,
}
/// Addressable path components; indices have already been evaluated into a local (OWN-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Projection {
    Field(u32),
    Index(LocalId),
    VariantField { variant: u32, field: u32 },
}
/// Root and projections form a place; never a user-visible reference (LANG-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Place {
    /// Access through this active loan; projections preserve it and reborrows name their parent.
    pub loan: Option<LoanId>,
    /// Source binding and relative field path used for this access. Storage
    /// aliases may share root/projection while retaining different identities.
    pub binding: Option<hirsel_types::Origin>,
    pub root: LocalId,
    pub projection: hirsel_types::SharedPath<Projection>,
    pub ty: TypeId,
}
/// Every operand states ownership explicitly; borrow/mut require an enclosing loan (OWN-1/2/3/4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Operand {
    pub place: PlaceId,
    pub usage: UseKind,
}
/// Typed literal bits; floats retain bit patterns rather than host arithmetic (SEM-1/2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Constant {
    Unit,
    Bool(bool),
    Integer(u128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
}
/// Static constructor identity; every field operand is already in source evaluation order (LANG-3/12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Constructor {
    Tuple,
    Record(DefPath),
    Variant {
        definition: DefPath,
        variant: u32,
    },
    Sequence,
    /// Materialized entries in insertion order, each a (key, value) tuple.
    Map,
    /// Materialized elements in insertion order.
    Set,
    /// Empty ordered map; the borrowed dictionary selects its brand, not stored content.
    SortedMap {
        by: Operand,
    },
    Box,
}
/// A constructor input maps source evaluation order to declaration layout (LANG-3/12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ConstructField {
    pub field: u32,
    pub value: Operand,
}
/// A call input maps source order to the declared parameter ordinal (LANG-3/12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CallArgument {
    pub parameter: hirsel_types::ParamId,
    pub value: Operand,
}
/// No nested effectful expressions: instructions evaluate in Vec order (LANG-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Rvalue {
    Constant(Constant),
    /// Read the program-lifetime value of a named constant (CT-2).
    /// Its signature supplies the type and declaration identity.
    ReadConstant {
        definition: DefPath,
        arguments: Vec<hirsel_types::Size>,
    },
    /// Named callable without an environment; its signature proves its value type.
    FunctionReference {
        definition: DefPath,
        arguments: Vec<GenericArgument>,
    },
    /// Read the current generic size binding as u64 (LANG-2).
    Size(hirsel_types::SizeParameterRef),
    Use(Operand),
    Unary {
        op: UnaryOp,
        value: Operand,
    },
    Binary {
        op: BinaryOp,
        left: Operand,
        right: Operand,
    },
    Construct {
        constructor: Constructor,
        fields: Vec<ConstructField>,
        destination: Destination,
    },
    Closure {
        /// Checked source creation context; backing storage is still heap-owned.
        context: hirsel_types::CaptureContext,
        body: BodyId,
        captures: Vec<Operand>,
        destination: Destination,
    },
    Shape(TypeId),
}
/// Binding introduction is distinct from moving storage between compiler slots.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginBindingKind {
    /// A copied/borrowed name retains the source value identity.
    Alias,
    /// An owner introduction starts a new incarnation of this source binder.
    Fresh,
}
/// Borrow boundaries are emitted by mode checking/lowering, not rederived by the interpreter (OWN-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum InstructionKind {
    /// Commit a whole resource destructure in its defining module (#97).
    /// Suppress its finalizer; ordinary field transfers and drops follow.
    Defuse {
        place: PlaceId,
    },
    /// Establish the name in Place.binding from this initialized value. Fresh
    /// introductions expire earlier incarnations; Alias reads the value identity.
    BindOrigin {
        source: PlaceId,
        place: PlaceId,
        kind: OriginBindingKind,
    },
    /// Introduce all direct field binders of a staging aggregate together.
    /// Declared sibling selectors are replayed against these names before the
    /// ordinary field transfers. This has no runtime storage effect.
    BindFields {
        source: PlaceId,
        fields: Vec<PlaceId>,
    },
    /// Written take ends a binding identity; administrative Use(Take) only moves
    /// storage. This operation has no runtime value or allocation effect.
    EndOrigin(PlaceId),
    StorageLive(LocalId),
    /// Retire proven Plain storage without a release or allocation effect.
    StorageDead {
        place: PlaceId,
        if_live: bool,
    },
    /// End a lexical storage lifetime after its observable cleanup has run.
    EndScope(ScopeId),
    Assign {
        destination: PlaceId,
        value: RvalueId,
    },
    /// Atomic installation in writable storage of a prepared owned value.
    /// A fresh temporary receives any old content; its explicit Drop follows.
    /// Mixed live/dead storage uses if_live and a tied conditional Drop.
    Replace {
        destination: PlaceId,
        value: Operand,
        displaced: PlaceId,
        /// Mixed incoming storage: exchange live content or initialize dead storage.
        if_live: bool,
    },
    BeginLoan {
        id: LoanId,
        place: PlaceId,
        mutable: bool,
        /// Mixed incoming read permission; strict begins require inactivity.
        if_inactive: bool,
    },
    /// Conditional branch alternatives end only on paths where they began.
    EndLoan {
        loan: LoanId,
        if_active: bool,
    },
    Drop {
        place: PlaceId,
        if_live: bool,
    },
    /// Execute explicit cleanup; Assign is only for initialization.
    RunCleanup(CleanupId),
    Check {
        condition: Operand,
        rule: RuleId,
    },
    /// Saves a plain before(e) result at entry for the exit contract (FACT-1).
    Snapshot {
        destination: PlaceId,
        value: Operand,
    },
    /// One unit of a compile-time evaluation's source budget (CT-4): the last
    /// instruction before a source call's transfer, or the first of a loop
    /// body iteration. It has no run-time effect and is not an execution step.
    Charge(SourceEvent),
}
/// The stable identity of a charged source event within its definition: the
/// call or loop as written, independent of lowering and optimization (CT-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SourceEvent {
    pub kind: SourceEventKind,
    /// The call or `?` mapper expression, or the `while`/`for` statement.
    pub source: u32,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SourceEventKind {
    /// Before a source call enters its callee, after its arguments.
    Call,
    /// On an entered loop body iteration, before body work.
    Iteration,
}
/// The charging rules' version: a cache keys evaluations by it (CT-4).
pub const CHARGING_VERSION: u32 = 1;
/// Failure edge is explicit even for checked arithmetic/constructors (FAIL-1/3, DET-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Instruction {
    pub kind: InstructionKind,
    pub span: Span,
    pub on_failure: CleanupId,
}
/// Calls name a definition or an already-evaluated closure operand (LANG-3, CLO-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Callee {
    Definition {
        path: DefPath,
        arguments: Vec<GenericArgument>,
    },
    Indirect {
        /// Compiler-owned contract calls inherit attribution; source function values reset it.
        forward_caller: bool,
        value: Operand,
        arguments: Vec<GenericArgument>,
    },
}
/// A returning call's result slot, initialized only on its normal successor.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallReturn {
    pub destination: PlaceId,
    pub next: BlockId,
}
/// Control edges preserve source evaluation, contracts, short circuiting and cleanup (LANG-3/8/11).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Terminator {
    Goto(BlockId),
    /// Select the first matching arm from this index without consuming storage.
    /// Nonzero starts are exclusively failed-guard continuations.
    Match {
        table: MatchTableId,
        from: u32,
    },
    Branch {
        condition: Operand,
        yes: BlockId,
        no: BlockId,
    },
    Switch {
        value: Operand,
        arms: Vec<(u32, BlockId)>,
        otherwise: BlockId,
    },
    /// On entry, take arguments transfer simultaneously; before entry they remain caller temporaries.
    /// `returns` is None exactly when the callee's result type is Never: a
    /// diverging call has no result and no normal successor, only `on_failure`.
    Call {
        callee: Callee,
        arguments: Vec<CallArgument>,
        returns: Option<CallReturn>,
        on_failure: CleanupId,
    },
    Return {
        value: Operand,
        cleanup: CleanupId,
    },
    /// Result error propagation is normal control flow; this is fail-stop failure only (FAIL-1).
    Fail {
        rule: RuleId,
        cleanup: CleanupId,
    },
    Unreachable,
}
/// An ordered instruction stream followed by exactly one terminator (LANG-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub span: Span,
}
/// Scope nesting only; cleanup order is stored, not inferred from nesting (DROP-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub span: Span,
}
/// Compiler-emitted cleanup action on all exits, including cancellation and failed drops (FAIL-3, DROP-2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum CleanupAction {
    Drop {
        place: PlaceId,
        if_live: bool,
    },
    /// Conditional branch alternatives end only on paths where they began.
    EndLoan {
        loan: LoanId,
        if_active: bool,
    },
    ReleaseRegion(hirsel_types::RegionId),
    CloseArena(PlaceId),
    /// Scope error exit cancels first, then joins; normal exit joins only (CONC-2).
    JoinTasks {
        scope: PlaceId,
        cancel: bool,
    },
}
/// Actions are executed in listed order, temporaries before reverse-declaration locals (LANG-3, DROP-2).
/// Drop failures continue remaining cleanup; user finalizers run shielded (FAIL-3, DROP-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct Cleanup {
    pub head: Option<CleanupNodeId>,
}
/// One immutable step; following parents executes the remainder of the cleanup.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct CleanupNode {
    pub action: CleanupAction,
    pub parent: Option<CleanupNodeId>,
}
impl Body {
    /// Normal successors in terminator order. A match names each remaining
    /// candidate arm's guard or target; an invalid match table names none.
    pub fn successors(&self, terminator: &Terminator) -> Vec<BlockId> {
        match terminator {
            Terminator::Goto(b) => vec![*b],
            Terminator::Match { table, from } => self
                .matches
                .get(table.0 as usize)
                .map(|t| {
                    t.arms
                        .iter()
                        .skip(*from as usize)
                        .map(|a| a.guard.unwrap_or(a.target))
                        .collect()
                })
                .unwrap_or_default(),
            Terminator::Branch { yes, no, .. } => vec![*yes, *no],
            Terminator::Switch {
                arms, otherwise, ..
            } => arms.iter().map(|a| a.1).chain([*otherwise]).collect(),
            Terminator::Call { returns, .. } => returns.iter().map(|r| r.next).collect(),
            Terminator::Return { .. } | Terminator::Fail { .. } | Terminator::Unreachable => vec![],
        }
    }

    /// Walk a cleanup without allocating or expanding its shared suffix.
    pub fn cleanup_actions(&self, id: CleanupId) -> impl Iterator<Item = &CleanupAction> {
        let mut next = self.cleanups[id.0 as usize].head;
        std::iter::from_fn(move || {
            let node = self.cleanup_nodes.get(next?.0 as usize)?;
            next = node.parent;
            Some(&node.action)
        })
    }
    /// Construct a chain from execution-ordered actions. Producers that repeatedly
    /// extend shared scopes should intern action/parent pairs instead.
    pub fn add_cleanup(&mut self, actions: Vec<CleanupAction>) -> CleanupId {
        let id = CleanupId(self.cleanups.len() as u32);
        self.cleanups.push(Cleanup::default());
        self.set_cleanup(id, actions);
        id
    }
    /// Replace a root while preserving references to all other immutable nodes.
    pub fn set_cleanup(&mut self, id: CleanupId, actions: Vec<CleanupAction>) {
        let mut head = None;
        for action in actions.into_iter().rev() {
            let node = CleanupNodeId(self.cleanup_nodes.len() as u32);
            self.cleanup_nodes.push(CleanupNode {
                action,
                parent: head,
            });
            head = Some(node);
        }
        self.cleanups[id.0 as usize].head = head;
    }
}
/// Source-authored normalization input; the IR checker independently checks it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TailKind {
    SelfTail,
    /// Reserved for the settled source contexts of spec question #46.
    ConstructorHole(Vec<u32>),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TailSite {
    pub owner_body: BodyId,
    pub call_block: BlockId,
    pub kind: TailKind,
    /// Whole counted temporaries whose ownership moves into loop slots.
    pub owned_loop_arguments: Vec<(hirsel_types::ParamId, PlaceId)>,
}
/// A producer claim for a fixed-origin read-only cursor. Validation re-derives
/// the borrowed declaration and checks every install independently.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct BorrowCursor {
    pub carrier: LocalId,
    pub initializer: RvalueId,
    /// None infers anchors from the initial Borrow; Some names earlier values.
    pub cover: Option<Vec<PlaceId>>,
}
/// A declaration-derived scoped callable path, independent of its physical passing class.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScopedParameter {
    pub parameter: u32,
    pub fields: Vec<u32>,
}

/// Entry requirement of a contextual core arena scope callback. V reconstructs
/// every executable producer/use; this receipt alone grants no permission.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ArenaScopeGrant {
    pub intrinsic: DefPath,
    pub callback_parameter: hirsel_types::ParamId,
    pub arena_parameter: hirsel_types::ParamId,
}

/// A monomorphic or generic body whose TypeIds refer to Definition.types (GEN-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Body {
    pub arena_scope_grant: Option<ArenaScopeGrant>,
    /// Declaring source scope for value-parameter references, separate from storage scopes.
    pub parameter_scope: hirsel_types::SiteRef,
    pub locals: Vec<Local>,
    pub places: Vec<Place>,
    pub scopes: Vec<Scope>,
    pub blocks: Vec<Block>,
    pub rvalues: Vec<Rvalue>,
    pub cleanups: Vec<Cleanup>,
    pub cleanup_nodes: Vec<CleanupNode>,
    pub entry: BlockId,
    pub parameters: Vec<LocalId>,
    pub result: TypeId,
    pub borrow_cursors: Vec<BorrowCursor>,
    pub tail_sites: Vec<TailSite>,
    pub matches: Vec<MatchTable>,
    pub scoped_parameters: Vec<ScopedParameter>,
}
/// Ordered source arms. Patterns are an acyclic arena owned by this table.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct MatchTable {
    pub scrutinee: PlaceId,
    pub patterns: Vec<MatchPattern>,
    pub arms: Vec<MatchArm>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct MatchArm {
    pub pattern: MatchPatternId,
    /// Non-consuming binding views are established here before the guard.
    pub guard: Option<BlockId>,
    /// Ordinary explicit binding instructions commit only after guard success.
    pub target: BlockId,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct MatchPattern {
    /// The projection this pattern observes or binds, rooted at the scrutinee.
    pub place: PlaceId,
    pub kind: MatchPatternKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum MatchPatternKind {
    Wildcard,
    Binding {
        usage: UseKind,
        guard_usage: UseKind,
    },
    Literal(Constant),
    /// A closed scalar range: both bounds are `Constant::Integer` bits of the
    /// place's integer or char type, and `start` orders at or before `end` (LANG-13).
    Range {
        start: Constant,
        end: Constant,
    },
    Tuple(Vec<(u32, MatchPatternId)>),
    Record {
        definition: DefPath,
        fields: Vec<(u32, MatchPatternId)>,
    },
    /// Packed carriers use the same pattern; their type determines payload layout.
    Variant {
        definition: DefPath,
        variant: u32,
        fields: Vec<(u32, MatchPatternId)>,
    },
}
impl MatchPatternKind {
    pub fn fields(&self) -> &[(u32, MatchPatternId)] {
        match self {
            Self::Tuple(fields) | Self::Record { fields, .. } | Self::Variant { fields, .. } => {
                fields
            }
            _ => &[],
        }
    }
}
/// One definition's lowered bodies, including closures and contract bodies (FACT-1, CT-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ProtocolAssociationReceipt {
    pub association: hirsel_types::ProtocolAssociation,
    pub body: BodyId,
    pub open: BlockId,
    pub next: BlockId,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    /// Body-checked private evidence, usable only at same-module direct calls.
    pub inferred_unique: hirsel_types::UniqueSummary,
    /// Sorted bodies that inherit attribution: the marked entry and its own contract bodies.
    /// Ordinary closures and their contracts remain separate invocation boundaries.
    pub caller_bodies: Vec<BodyId>,
    pub protocol_associations: Vec<ProtocolAssociationReceipt>,
    /// The canonical closure of every type its entry, closure and contract
    /// bodies reach, in first-encounter order (`hirsel_types::closure`): what
    /// V checks and the definition's persisted type table.
    pub types: std::sync::Arc<[TypeId]>,
    pub path: DefId,
    pub signature_hash: ContentHash,
    pub bodies: Vec<Body>,
    pub entry: BodyId,
    pub contracts: Vec<(u32, BodyId)>,
}
impl Definition {
    /// Whether a validated body inherits the declaration's caller attribution.
    pub fn tracks_caller(&self, body: BodyId) -> bool {
        self.caller_bodies
            .binary_search_by_key(&body.0, |id| id.0)
            .is_ok()
    }
}
/// `Definition.types` for these bodies: the closure (`hirsel_types::closure`)
/// of every TypeId they name, rooted per body in a fixed order: the result,
/// local, place and rvalue types, then call-site type actuals in block order.
/// Lowering and every TIR rewrite compute `types` with this one walk.
pub fn definition_types(t: &TypeInterner, definition: &Definition) -> std::sync::Arc<[TypeId]> {
    fn actuals(arguments: &[GenericArgument], roots: &mut Vec<TypeId>) {
        roots.extend(arguments.iter().filter_map(|a| match a {
            GenericArgument::Type(t) => Some(*t),
            GenericArgument::Size(_) => None,
        }));
    }
    let mut roots = vec![];
    for body in &definition.bodies {
        roots.push(body.result);
        roots.extend(body.locals.iter().map(|l| l.ty));
        roots.extend(body.places.iter().map(|p| p.ty));
        for rvalue in &body.rvalues {
            match rvalue {
                Rvalue::Shape(t) => roots.push(*t),
                Rvalue::FunctionReference { arguments, .. } => actuals(arguments, &mut roots),
                _ => (),
            }
        }
        for block in &body.blocks {
            if let Terminator::Call { callee, .. } = &block.terminator {
                match callee {
                    Callee::Definition { arguments, .. } | Callee::Indirect { arguments, .. } => {
                        actuals(arguments, &mut roots)
                    }
                }
            }
        }
    }
    hirsel_types::closure(t, roots)
}
/// Observable fail-stop data uses an indexed cause chain (FAIL-2); no Rc trees.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub kind: String,
    pub rule: RuleId,
    pub site: Span,
    pub spawn_path: Vec<u32>,
    pub cause: Option<FailureId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_types::{Type, TypeKind};
    #[test]
    fn hand_built_unit_body_has_explicit_return_and_cleanup() {
        let types = TypeInterner::new(None);
        let unit = types.intern(Type {
            drop_allocation: hirsel_types::DropAllocation::MayAllocate,
            ..Type::with_kind(TypeKind::Tuple(vec![].into()))
        });
        let span = Span::default();
        let body = Body {
            arena_scope_grant: None,
            scoped_parameters: Vec::new(),
            parameter_scope: hirsel_types::LexicalOwner::first(&hirsel_syntax::DefPath::core(
                "main",
            ))
            .site(hirsel_types::NameSite::Item),
            locals: vec![Local {
                ty: unit,
                storage: Storage::ReturnSlot,
                scope: ScopeId(0),
                span,
            }],
            places: vec![Place {
                loan: None,
                binding: None,
                root: LocalId(0),
                projection: vec![].into(),
                ty: unit,
            }],
            scopes: vec![Scope { parent: None, span }],
            blocks: vec![Block {
                instructions: vec![Instruction {
                    kind: InstructionKind::Assign {
                        destination: PlaceId(0),
                        value: RvalueId(0),
                    },
                    span,
                    on_failure: CleanupId(0),
                }],
                terminator: Terminator::Return {
                    value: Operand {
                        place: PlaceId(0),
                        usage: UseKind::Take,
                    },
                    cleanup: CleanupId(0),
                },
                span,
            }],
            rvalues: vec![Rvalue::Constant(Constant::Unit)],
            cleanup_nodes: vec![],
            cleanups: vec![Cleanup::default()],
            entry: BlockId(0),
            parameters: vec![],
            result: unit,
            borrow_cursors: vec![],
            tail_sites: vec![],
            matches: vec![],
        };
        assert!(matches!(
            body.blocks[body.entry.0 as usize].terminator,
            Terminator::Return { .. }
        ));
        assert!(body.cleanup_actions(CleanupId(0)).next().is_none());
        // The table roots at the result, then locals, places and rvalues, and
        // reaches each root's parts in first-encounter order.
        let byte = types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U8,
        )));
        let pair = types.intern(Type::with_kind(TypeKind::Tuple(vec![byte, unit].into())));
        let mut shaped = body.clone();
        shaped.rvalues.push(Rvalue::Shape(pair));
        let definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: vec![].into(),
            path: DefId::intern(&hirsel_syntax::DefPath::core("main")),
            inferred_unique: Default::default(),
            signature_hash: ContentHash::default(),
            bodies: vec![body, shaped],
            entry: BodyId(0),
            contracts: vec![],
        };
        assert_eq!(&*definition_types(&types, &definition), &[unit, pair, byte]);
        // Equal offsets from a declaration and generated text stay distinct,
        // including sources that first occur in a nested body. Unmapped spans
        // never acquire the declaration's identity.
        let mut definition = definition;
        let declaration = hirsel_span::Anchor::fresh();
        let generated = hirsel_span::Anchor::fresh();
        let nested = hirsel_span::Anchor::fresh();
        definition.bodies[0].locals[0].span = Span::new(declaration, 0, 1).unwrap();
        definition.bodies[0].blocks[0].span = Span::new(generated, 0, 1).unwrap();
        definition.bodies[1].blocks[0].instructions[0].span = Span::new(nested, 0, 1).unwrap();
        assert_eq!(
            source_anchors(&definition),
            [declaration, generated, nested]
        );
        // Anchor allocation and unrelated source registration do not alter the
        // selector of any cited source.
        let _unrelated = hirsel_span::Anchor::fresh();
        assert_eq!(
            source_anchors(&definition),
            [declaration, generated, nested]
        );
    }
}

/// Cheap immutable lowered definition snapshot.
pub type DefinitionRef = std::sync::Arc<Definition>;

/// Evaluated data; its TypeIds are rows of the session interner. No evaluator is linked here.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Materialized {
    pub root: ValueNode,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ValueNode {
    pub ty: TypeId,
    pub kind: ValueKind,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Constant(Constant),
    FunctionReference {
        definition: DefPath,
        arguments: Vec<hirsel_types::GenericArgument>,
    },
    Construct {
        constructor: Constructor,
        fields: Vec<ValueNode>,
    },
}
/// Immutable materialized constant snapshot.
pub type MaterializedRef = std::sync::Arc<Materialized>;

/// CT-5 instance bodies. A generic generated declaration has no definition of
/// its own: each selected instance at closed generic arguments is a monomorphic
/// definition, and a call of the declaration at those arguments runs it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct InstanceBodies {
    /// Every generic generated declaration of the program, selected or not.
    pub declarations: std::collections::BTreeSet<DefId>,
    /// Selected instances by declaration and their actuals' CT-5 key.
    #[serde(with = "sorted_instances")]
    pub instances: std::collections::BTreeMap<
        DefId,
        std::collections::HashMap<GeneratedActualKey, InstanceBody>,
    >,
}
/// One selected instance: its closed lossless actuals (the first selected in
/// key order represents the key) and the monomorphic definition that is the
/// declaration's body at them.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct InstanceBody {
    pub arguments: Vec<GenericArgument>,
    pub definition: DefId,
}
impl InstanceBodies {
    /// The CT-5 key of closed `arguments`; None when one still names a parameter.
    pub fn key(types: &TypeInterner, arguments: &[GenericArgument]) -> Option<GeneratedActualKey> {
        hirsel_types::generated_actual_key(types, arguments)
    }
    /// The definition that runs a call of `path` at closed `arguments`: None
    /// when `path` is not a generic generated declaration, and an error when
    /// it is but that instance was never selected.
    pub fn resolve(
        &self,
        path: DefId,
        types: &TypeInterner,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefId>, QueryError> {
        if !self.declarations.contains(&path) {
            return Ok(None);
        }
        let unselected = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "generated instance".into(),
                message: format!("{path} has no selected instance at these arguments"),
            })
        };
        let key = Self::key(types, arguments).ok_or_else(unselected)?;
        self.instances
            .get(&path)
            .and_then(|instances| instances.get(&key))
            .map(|instance| Some(instance.definition))
            .ok_or_else(unselected)
    }
}
/// Instances persist in key order, never hash order.
mod sorted_instances {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    type Instances = BTreeMap<DefId, HashMap<GeneratedActualKey, InstanceBody>>;
    pub fn serialize<S: serde::Serializer>(map: &Instances, s: S) -> Result<S::Ok, S::Error> {
        let sorted: BTreeMap<&DefId, BTreeMap<&GeneratedActualKey, &InstanceBody>> = map
            .iter()
            .map(|(d, instances)| (d, instances.iter().collect()))
            .collect();
        serde::Serialize::serialize(&sorted, s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Instances, D::Error> {
        let sorted: BTreeMap<DefId, BTreeMap<GeneratedActualKey, InstanceBody>> =
            serde::Deserialize::deserialize(d)?;
        Ok(sorted
            .into_iter()
            .map(|(d, instances)| (d, instances.into_iter().collect()))
            .collect())
    }
}
/// Execution requests other definitions lazily, permitting any topology (PROG-5).
pub trait TirProvider {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError>;
    /// Compiled immutable data; this never executes initializer code.
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError>;
    /// Data of a selected size-generic constant. Ordinary constants have no actuals.
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        if arguments.is_empty() {
            self.constant(path)
        } else {
            Err(QueryError::NotImplemented)
        }
    }
    /// Program constants in dependency order, with source-order ties (#63).
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError>;
    /// CT-5: the definition that runs a call of `path` at closed `arguments`
    /// (`InstanceBodies::resolve`); None for a declaration with its own body.
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        let _ = (path, arguments);
        Ok(None)
    }
}
/// Simulation input events are ordered and explicit; real external effects are not ambient (DET-1, CAP-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CapabilityEvent {
    pub capability: DefPath,
    pub operation: String,
    pub input: Vec<u8>,
    pub output: Vec<u8>,
}
/// Explicit deterministic execution configuration (DET-1, PROG-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct RunConfiguration {
    pub entry: DefPath,
    pub hash_seed: u64,
    pub inputs: Vec<CapabilityEvent>,
}

/// Artificial evaluator guard, not a language failure or an execution observation.
/// Accounting, site selection and cleanup policy remain issue #34.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolingLimitKind {
    Steps,
    CallDepth,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ToolingLimit {
    pub kind: ToolingLimitKind,
    pub limit: u64,
    pub site: Option<Span>,
}

/// Link identity of one immutable constant instance; no actuals preserves the
/// ordinary constant identity. Decimal components are unambiguous and ordered.
pub fn constant_instance_path(definition: &DefPath, arguments: &[u64]) -> DefPath {
    let mut path = definition.clone();
    if !arguments.is_empty() {
        path.0.name = format!(
            "$constant:{}:{}",
            definition.0.name,
            arguments
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(":")
        );
    }
    path
}

/// Existing source anchors in canonical first-encounter order, independent of
/// process allocation, file placement and worker scheduling. Includes every
/// nested body. The no-source anchor is never assigned a durable selector.
///
/// As in rustc's stable source-file index (`rustc_span/src/source_map.rs:171`),
/// this indexes existing sources; it does not allocate a second source map.
pub fn source_anchors(definition: &Definition) -> Vec<hirsel_span::Anchor> {
    let mut anchors = Vec::new();
    let mut add = |span: Span| {
        if span.anchor != hirsel_span::Anchor(0) && !anchors.contains(&span.anchor) {
            anchors.push(span.anchor);
        }
    };
    for body in &definition.bodies {
        for local in &body.locals {
            add(local.span);
        }
        for scope in &body.scopes {
            add(scope.span);
        }
        for block in &body.blocks {
            add(block.span);
            for instruction in &block.instructions {
                add(instruction.span);
            }
        }
    }
    anchors
}
