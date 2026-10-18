//! Explicit ordered ownership IR, before Perceus dup/drop optimization (LANG-3, MEM-1).
use hirsel_diag::RuleId;
use hirsel_span::Span;
use hirsel_syntax::{BinaryOp, ContentHash, DefPath, UnaryOp};
use hirsel_types::QueryError;
use hirsel_types::{Destination, GenericArgument, TypeId, TypeStore, UseKind};

/// Borrowed-association receipts revised the formerly implicit v1 TIR schema;
/// v3 adds diverging calls (`Terminator::Call { returns: None, .. }`).
pub const TIR_VERSION: u32 = 3;

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
}
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
/// A monomorphic or generic body whose TypeIds refer to Definition.types (GEN-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Body {
    /// Declaring source scope for value-parameter references, separate from storage scopes.
    pub parameter_scope: hirsel_types::NameSite,
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
    pub tail_sites: Vec<TailSite>,
    pub matches: Vec<MatchTable>,
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
    pub protocol_associations: Vec<ProtocolAssociationReceipt>,
    /// The sole published type graph for all entry, closure and contract bodies.
    pub types: TypeStore,
    pub path: DefPath,
    pub signature_hash: ContentHash,
    pub bodies: Vec<Body>,
    pub entry: BodyId,
    pub contracts: Vec<(u32, BodyId)>,
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
        let mut types = TypeStore::default();
        let unit = types.push(Type {
            drop_allocation: hirsel_types::DropAllocation::MayAllocate,
            ..Type::with_kind(TypeKind::Tuple(vec![].into()))
        });
        let span = Span::default();
        let body = Body {
            parameter_scope: hirsel_types::NameSite::Item(hirsel_syntax::ItemId(0)),
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
            tail_sites: vec![],
            matches: vec![],
        };
        assert!(matches!(
            body.blocks[body.entry.0 as usize].terminator,
            Terminator::Return { .. }
        ));
        assert!(body.cleanup_actions(CleanupId(0)).next().is_none());
    }
}

/// Cheap immutable lowered definition snapshot.
pub type DefinitionRef = std::sync::Arc<Definition>;

/// Evaluated data and its producer-owned type graph. No evaluator is linked here.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Materialized {
    pub types: TypeStore,
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
    FunctionReference(DefPath),
    Construct {
        constructor: Constructor,
        fields: Vec<ValueNode>,
    },
}
/// Immutable materialized constant snapshot.
pub type MaterializedRef = std::sync::Arc<Materialized>;

/// Execution requests other definitions lazily, permitting any topology (PROG-5).
pub trait TirProvider {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError>;
    /// Compiled immutable data; this never executes initializer code.
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError>;
    /// Program constants in dependency order, with source-order ties (#63).
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError>;
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
