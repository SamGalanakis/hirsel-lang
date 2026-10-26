//! Causal ownership explanations. A failed ownership judgment
//! states what is provided and why, at the cause's site, and what is required
//! and why, at the requirement's site, then ranked repairs that the checker
//! accepts. Producers fill these records from facts they already hold and
//! only on the error path; this module renders them. The vocabulary is data:
//! a change of locality tracking adapts the producers, and a respelling of the
//! qualifiers changes `qualifier` alone.
use hirsel_diag::Diagnostic;
use hirsel_span::Span;
use hirsel_syntax::{DefPath, ExprId, ExprKind, Literal, SyntaxTree, UnaryOp};
use hirsel_types::{BindingDecision, NameSite, ParamId, ResolvedName};
use std::hash::{Hash, Hasher};

/// A written qualifier.
pub enum Qualifier<'a> {
    /// `in x` or `in {x, ..}`; no places is the closure form, `in env`.
    In(&'a [&'a str]),
    Heap,
    Brand(&'a str),
}
/// The one spelling of every qualifier in diagnostic text.
pub fn qualifier(q: Qualifier<'_>) -> String {
    match q {
        Qualifier::In([]) => "in env".into(),
        Qualifier::In([place]) => qualified_name("in", place),
        Qualifier::In(places) => {
            let descriptive = places.iter().any(|p| p.starts_with(PROSE));
            let text = format!(
                "in {{{}}}",
                places
                    .iter()
                    .map(|p| p.trim_start_matches(PROSE))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if descriptive { prose(&text) } else { text }
        }
        Qualifier::Heap => "in heap".into(),
        Qualifier::Brand(owner) => qualified_name("of", owner),
    }
}
fn qualified_name(prefix: &str, name: &str) -> String {
    match name.strip_prefix(PROSE) {
        Some(description) => prose(&format!("{prefix} {description}")),
        None => format!("{prefix} {name}"),
    }
}

/// Why a value has the property that fails: local, placed, branded, moved,
/// lent and so on. Each kind is a pair of templates over its names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cause {
    BorrowedParameter,
    MutParameter,
    TakenParameter,
    Capture,
    CreationInput,
    Binding,
    Holds,
    LocalResult,
    Temporary,
    FrameStorage,
    Environment,
    Unplaced,
    LocalValue,
    Placed,
    NoPlacement,
    Branded,
    Unbranded,
    IdentityEnded,
    Moved,
    Uninitialised,
    Lent,
    Immutable,
    MutAlias,
    Published,
    Once,
    Aliased,
    Noncopyable,
    ScopedClosure,
    ScopedCapability,
    NotSendable,
    Authority,
    Resource,
    MutableCapture,
    OperandBorrows,
    Part,
    ArgumentLoan,
    Iterated,
    FinalizerResource,
    Packed,
    TakenOwner,
    ClosureOwn,
    SiblingOwner,
    Header,
    MutLend,
    NotMut,
    NotTaken,
    BrandForgotten,
    ScopedPlacement,
    BorrowedCapture,
    ReceiverCapture,
    ReadOnlyParameter,
    AuthorityCapture,
    LastUse,
    BlockBinding,
    Sendable,
    UniquenessLoss,
    UniquenessMerge,
    IterationItem,
    InspectedMatch,
    BorrowedPattern,
}
impl Cause {
    /// The closed docs/spec/diagnostics.md vocabulary entry. Fine wording kinds remain
    /// private; every kind has an entry (#81).
    #[cfg(test)]
    pub fn spec_kind(self) -> &'static str {
        use Cause::*;
        match self {
            BorrowedParameter | ReadOnlyParameter => "borrowed parameter",
            MutParameter => "mut parameter",
            TakenParameter | TakenOwner => "taken parameter",
            LocalResult => "local result",
            MutableCapture => "written relation, brand or modifier",
            Capture | CreationInput | Environment | ScopedClosure | BorrowedCapture => {
                "environment of closure"
            }
            Branded | BrandForgotten => "brand",
            Unbranded => "brand erased",
            IdentityEnded => "identity ended",
            IterationItem | Lent | ArgumentLoan | Iterated | MutLend => "loan on place",
            Aliased | UniquenessLoss | UniquenessMerge => "share or copy that ended uniqueness",
            Placed | ScopedPlacement => "arena placement",
            Temporary => "temporary",
            Holds => "stored at a place",
            Binding | FrameStorage | LocalValue | BlockBinding => "storage of this body",
            Unplaced | NoPlacement => "no placement",
            Moved | Uninitialised => "moved or not initialised",
            Immutable | MutAlias | NotMut | NotTaken | InspectedMatch | BorrowedPattern => "access",
            LastUse => "last use of a binding",
            Published => "published storage",
            Once => "once function value",
            Noncopyable | Resource | FinalizerResource | NotSendable | Sendable
            | ReceiverCapture | Packed => "structural property of a type",
            Authority | AuthorityCapture | ScopedCapability => "authority",
            OperandBorrows => "computed operand that only borrows other storage",
            Part => "of an owned value",
            ClosureOwn | SiblingOwner => "owner that cannot hold a brand or placement",
            Header => "header of a function value",
        }
    }
    /// (headline fragment, label text); `{0}`, `{1}` are the fact's names.
    fn text(self) -> (&'static str, &'static str) {
        use Cause::*;
        match self {
            BorrowedParameter => (
                "`{0}` is local to the caller",
                "`{0}` is a borrowed parameter; it lives in the caller's region",
            ),
            MutParameter => (
                "`{0}` is the caller's storage",
                "`{0}` is a `mut` parameter, local to the caller",
            ),
            TakenParameter => (
                "`{0}` ends with this function",
                "`{0}` is a taken parameter whose storage ends with this function",
            ),
            Capture => (
                "`{0}` is local to the enclosing function",
                "`{0}` is captured by borrowing, so the closure knows it as a borrowed parameter (CLO-1)",
            ),
            CreationInput => (
                "the environment retains an unselected or unproved input",
                "the actual creation operands determine the retained envelope",
            ),
            Binding => (
                "`{0}` is local to this function",
                "`{0}` is storage of this body; it ends with its scope",
            ),
            Holds => (
                "`{0}` holds a local value",
                "`{0}` holds a value local to its source",
            ),
            LocalResult => (
                "the result of `{0}` is local to its arguments",
                "`{0}` returns a `{1}` result, local to its arguments",
            ),
            Temporary => (
                "it depends on a temporary",
                "a temporary created here ends with its enclosing statement or block",
            ),
            FrameStorage => (
                "it depends on storage of this body",
                "this storage ends with the function",
            ),
            BlockBinding => (
                "`{0}` ends with its block",
                "`{0}` is declared in this block, so its storage ends here",
            ),
            Environment => (
                "it is local to a closure environment",
                "a closure over local content returns results local to its environment (CLO-1)",
            ),
            Unplaced => (
                "it has no single placement",
                "a join of differently placed values lives where its sources do (PROV-4)",
            ),
            LocalValue => ("the value is local", "this value is local here"),
            Placed => (
                "`{0}` lives in arena `{1}`",
                "placed `{2}` by its declared type",
            ),
            NoPlacement => (
                "`{0}` has no declared placement",
                "declared without a placement, so it is not known to be on the heap",
            ),
            Branded => ("`{0}` belongs to `{1}`", "branded `{2}` by its type"),
            Unbranded => (
                "`{0}` has no single brand",
                "a computed or joined value does not keep a brand (PROV-2)",
            ),
            IdentityEnded => (
                "its owner's identity ended",
                "the owner's identity ended here",
            ),
            Moved => ("`{0}` was moved", "`{0}` is moved here"),
            Uninitialised => (
                "`{0}` is not initialised on every path",
                "`{0}` has no value on some path to its use",
            ),
            Lent => ("`{0}` is borrowed", "`{1}` borrows `{0}` and is still live"),
            Immutable => ("`{0}` is immutable", "`{0}` is declared without `mut`"),
            MutAlias => (
                "`{0}` is a read-only alias",
                "`{0}` borrows its value; only a `mut` binding grants write access",
            ),
            Published => (
                "`{0}` is published storage",
                "Shared and region storage is immutable once published",
            ),
            Once => (
                "the function value may be called once",
                "a once function: it takes a capture, captures once, or is declared `once`",
            ),
            Aliased => (
                "`{0}` is not proved unique",
                "`{0}` may share counted storage; its current contract does not prove uniqueness",
            ),
            InspectedMatch => (
                "the match borrows the payloads of `{0}`",
                "this match inspects `{0}` in place and borrows its payloads (LANG-13)",
            ),
            BorrowedPattern => (
                "`{0}` is a borrowed pattern binding",
                "this pattern binds `{0}` as a borrowed view, even when the scrutinee is owned",
            ),
            IterationItem => ("loop item `{0}` supplies this input", "{1}"),
            UniquenessLoss => ("`{0}` lost its deep uniqueness", "{1}"),
            UniquenessMerge => ("`{0}` is not proved deeply unique on every path", "{1}"),
            Noncopyable => (
                "`{0}` cannot be copied",
                "`{0}` has a noncopyable type: it holds a resource, handle, once function or arena",
            ),
            ScopedClosure => (
                "the closure borrows its environment",
                "a scoped closure borrows storage of this body",
            ),
            ScopedCapability => (
                "the value is a scoped capability",
                "a scoped capability is valid only inside its callback (MODE-6)",
            ),
            NotSendable => (
                "`{0}` is not proved sendable",
                "`{0}` cannot cross a task boundary under its current type and ownership contract",
            ),
            Sendable => (
                "the storage is `sendable`",
                "its type promises that what it holds may cross a task boundary",
            ),
            Authority => (
                "the function value holds authority",
                "its authority summary is not within the declared `uses`",
            ),
            Resource => (
                "the value holds a resource",
                "a resource must be destroyed exactly once",
            ),
            MutableCapture => ("`{0}` is mutable", "`{0}` is declared `mut`"),
            OperandBorrows => (
                "the operand only borrows other storage",
                "a computed value that names other storage is not a place",
            ),
            Part => (
                "this is a part of `{0}`",
                "a part of an owned value; moving it would leave `{0}` partly moved",
            ),
            ArgumentLoan => (
                "`{0}` is lent to this call",
                "an earlier argument of this call lends `{0}`",
            ),
            Iterated => (
                "`{0}` is being iterated",
                "this loop reads `{0}` at every step until it exits (SEM-4)",
            ),
            FinalizerResource => (
                "`{0}` is the finalizer's own resource",
                "a finalizer only borrows the resource it destroys",
            ),
            Packed => (
                "the sequence is packed",
                "a sequence of Packed values is sequential",
            ),
            TakenOwner => (
                "`{0}` is taken",
                "a taken parameter ends with the function, so it cannot own a brand or placement",
            ),
            ClosureOwn => (
                "the closure result names its own parameters",
                "a closure's parameters end with each call",
            ),
            SiblingOwner => (
                "the owner is a sibling field",
                "a sibling brand owner lives and moves with its record (PROV-5)",
            ),
            Header => (
                "the function value is `{0}`",
                "this function value is `{0}`",
            ),
            MutLend => (
                "the operand is lent `mut`",
                "`mut` lends this place exclusively",
            ),
            NotMut => (
                "the argument is passed without `mut`",
                "without `mut`, this argument is lent read-only",
            ),
            NotTaken => (
                "the argument is a place passed without `take`",
                "without `take`, a place is lent, not moved",
            ),
            BrandForgotten => (
                "the argument carries a brand",
                "this argument's type names a brand",
            ),
            ScopedPlacement => (
                "the capture lives in a scoped placement",
                "this capture's storage is placed in an arena or region",
            ),
            BorrowedCapture => (
                "`{0}` is captured by borrowing",
                "`{0}` is captured by borrowing here; it stays with the source owner (CLO-1)",
            ),
            AuthorityCapture => (
                "`{0}` holds authority",
                "`{0}` is captured from this declaration with authority outside the callback's `uses`",
            ),
            LastUse => (
                "`{0}` is not used again",
                "`{0}` is not used after this call",
            ),
            ReadOnlyParameter => (
                "`{0}` is a borrowed parameter",
                "`{0}` is borrowed: its caller lends it read-only (OWN-1)",
            ),
            ReceiverCapture => (
                "`{0}` is a Receiver",
                "a Receiver crosses into a task only when taken",
            ),
        }
    }
}

/// Why the receiver needs the property that fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requirement {
    ResultGlobal,
    ResultSources,
    ResultNoTemporary,
    ResultTakenSource,
    MutStorage,
    StatePreservation,
    CreationEnvelope,
    GlobalBinding,
    AnnotatedBinding,
    Argument,
    EscapingCapture,
    Outlives,
    FunctionSlot,
    Exclusive,
    Write,
    Owned,
    OwnedResult,
    TakeIntoClosure,
    Unique,
    Many,
    Sendable,
    Brand,
    Placement,
    Heap,
    Read,
    Copy,
    DeepCopy,
    StoreClosure,
    ReturnClosure,
    MutArgument,
    TakeArgument,
    ConstructorOperand,
    Uses,
    NoResources,
    Index,
    Header,
    NoOwnPlacement,
    Arena,
    Mutate,
    Capture,
    OutlivesScope,
    Part,
    NoPlacement,
    BrandedUse,
    OwnerOutlives,
    StoreValue,
    ReturnValue,
    Inert,
    UnbrandedParameter,
    TaskCapture,
    GlobalArgument,
    AnnotatedGlobal,
    Borrows,
    Within,
    OutlivesBlock,
    SendableType,
    NotSendableType,
}
impl Requirement {
    /// The closed docs/spec/diagnostics.md vocabulary entry. Fine wording kinds remain
    /// private; every kind has an entry (#81).
    #[cfg(test)]
    pub fn spec_kind(self) -> &'static str {
        use Requirement::*;
        match self {
            ResultGlobal => "result without a relation",
            ResultSources | ResultTakenSource => "result relation",
            MutStorage | StatePreservation => "caller storage",
            GlobalBinding | AnnotatedBinding | FunctionSlot | Header | Part | AnnotatedGlobal
            | Within => "receiving contract of binding",
            Argument | MutArgument | TakeArgument | GlobalArgument | Borrows => {
                "argument for parameter"
            }
            EscapingCapture | CreationEnvelope | OwnedResult | TakeIntoClosure | StoreClosure
            | ReturnClosure | ConstructorOperand | Capture | StoreValue | ReturnValue
            | TaskCapture => "escaping store",
            ResultNoTemporary | OutlivesScope => "end of a temporary scope",
            Outlives | Arena | OwnerOutlives | OutlivesBlock => "exit of a scope or arena",
            Exclusive | Write | Owned | Mutate => "exclusive action on place",
            Unique => "uniqueness requirement",
            Sendable | SendableType | NotSendableType => "sendability requirement",
            Many => "many function value",
            Brand | Placement | Heap | NoPlacement | BrandedUse | UnbrandedParameter => {
                "brand or placement"
            }
            NoOwnPlacement => "closure result outside its own parameters",
            Read | Copy | DeepCopy | Index => "access requirement",
            Uses | Inert => "authority bound",
            NoResources => "arena contents without resources",
        }
    }
    fn text(self) -> (&'static str, &'static str) {
        use Requirement::*;
        match self {
            ResultGlobal => (
                "the result must be global",
                "the result names no `{1}` source, so it is global (MODE-7)",
            ),
            ResultSources => (
                "the result may depend only on its declared sources",
                "the result is declared `{1}`",
            ),
            ResultNoTemporary => (
                "the result outlives this function",
                "a result may depend on the caller's storage, not on a temporary of this body",
            ),
            ResultTakenSource => (
                "the result outlives the taken parameter",
                "a result local to a taken parameter must name it in `{1}`",
            ),
            StatePreservation => (
                "`{0}` must retain its selected incoming state",
                "the declared state promise preserves `{0}` on every exit",
            ),
            CreationEnvelope => (
                "the creation contract permits only its selected input envelopes",
                "unselected inputs need independent inert, Global, Many, Sendable and noalloc-drop proofs",
            ),
            MutStorage => (
                "`{0}` only receives global values",
                "`{0}` is a `mut` parameter; its storage is the caller's and outlives this call",
            ),
            GlobalBinding => (
                "`{0}` only receives global values",
                "`{0}` is declared without `{1}`, so it receives only global values",
            ),
            AnnotatedBinding => (
                "`{0}` requires a different value",
                "the annotation is the receiving contract of `{0}`",
            ),
            Argument => (
                "`{0}` requires a different argument",
                "a parameter of `{0}` imposes this requirement (MODE-7)",
            ),
            EscapingCapture => (
                "the closure escapes",
                "an escaping closure owns its captures, which must be immutable, owned and global (CLO-4)",
            ),
            Outlives => (
                "`{0}` outlives it",
                "`{0}` lives until the end of its scope",
            ),
            OutlivesBlock => (
                "it is read after the block ends",
                "this reads `{0}` after the block that declares `{1}` has ended",
            ),
            FunctionSlot => (
                "this slot promises a global result",
                "a declared function type promises a global result (CLO-1)",
            ),
            Exclusive => (
                "this needs exclusive access to `{0}`",
                "writing, moving or lending `mut` needs exclusive access (OWN-3)",
            ),
            Write => (
                "this writes `{0}`",
                "writing needs a `mut` binding or a `mut` parameter",
            ),
            Owned => (
                "this needs an owned value",
                "taking moves the value out of its owner",
            ),
            OwnedResult => (
                "the result must be owned",
                "a returned or stored value is moved out of its owner",
            ),
            TakeIntoClosure => (
                "the closure takes it",
                "an environment that takes a capture owns it",
            ),
            Unique => (
                "a unique value is required",
                "this requires a unique value (MODE-2)",
            ),
            Many => (
                "a many function is required",
                "this function type may be called many times (MODE-3)",
            ),
            Sendable => (
                "a sendable value is required",
                "this crosses a task boundary (MODE-4)",
            ),
            SendableType => (
                "{0} requires a `sendable` value",
                "a `sendable` type holds only values that may cross a task boundary (MODE-4)",
            ),
            NotSendableType => (
                "{0} is not `sendable`",
                "storage of its type may come to hold a value that crosses no task (MODE-4)",
            ),
            Brand => ("{2} requires a value `{1}`", "{2} requires `{1}`"),
            Placement => (
                "{2} requires arena `{0}`",
                "{2} requires `{1}`; every part must outlive `{0}`",
            ),
            Heap => ("{2} requires the heap", "{2} requires `{1}`"),
            BrandedUse => (
                "it is used after its owner ended",
                "this use needs the brand's owner to be live (PROV-1)",
            ),
            OwnerOutlives => (
                "a brand or placement must outlive the call",
                "a brand or placement names storage the caller still sees",
            ),
            StoreValue => (
                "it is stored",
                "stored values outlive the callback that grants the capability (MODE-6)",
            ),
            ReturnValue => (
                "it is returned",
                "a result outlives the callback that grants the capability (MODE-6)",
            ),
            Inert => (
                "an inert value is required",
                "this bound requires an empty authority summary (`inert`)",
            ),
            UnbrandedParameter => (
                "the parameter's type names no brand",
                "the parameter could hand this storage back without its brand",
            ),
            Within => (
                "{2} admits only values within `{0}`",
                "{2} is declared `{1}`, so it holds only arenas `{0}` holds (PROV-4)",
            ),
            Borrows => (
                "`{0}` only borrows it",
                "this parameter of `{0}` borrows its argument, and the result keeps nothing of it",
            ),
            GlobalArgument => (
                "`{0}` requires a global argument",
                "a parameter of `{0}` takes a global value (MODE-7)",
            ),
            AnnotatedGlobal => (
                "`{0}` only receives global values",
                "its annotation makes `{0}` receive only global values",
            ),
            TaskCapture => (
                "the closure crosses into a task",
                "task captures must copy or take sendable values (CLO-3)",
            ),
            NoPlacement => (
                "{2} declares no placement",
                "{2} declares no placement, so it promises nothing about an arena (PROV-4)",
            ),
            Read => ("it is read here", "this reads `{0}`"),
            Copy => (
                "a copyable value is required",
                "`copy` and copying uses require a copyable value (OWN-5)",
            ),
            DeepCopy => (
                "a deep copy is required",
                "a deep copy requires a copyable value (OWN-5)",
            ),
            StoreClosure => (
                "it is stored",
                "a stored closure outlives the storage it borrows (CLO-1)",
            ),
            ReturnClosure => (
                "it is returned",
                "a returned closure outlives the storage it borrows (CLO-1)",
            ),
            MutArgument => (
                "the parameter is `mut`",
                "a `mut` parameter requires `mut` at the call site (OWN-3)",
            ),
            TakeArgument => (
                "the parameter is `take`",
                "a `take` parameter requires an owned argument (OWN-2)",
            ),
            ConstructorOperand => (
                "a constructor stores its operands",
                "a constructor has no place to lend exclusively (OWN-3)",
            ),
            Uses => (
                "the declared `uses` restricts authority",
                "the required `uses` restriction",
            ),
            NoResources => (
                "this cannot hold resources",
                "arena storage holds no resources (MEM-4)",
            ),
            Index => ("this indexes it", "indexing needs random access (MEM-13)"),
            Header => (
                "the required function type is `{0}`",
                "the required function type is `{0}`",
            ),
            NoOwnPlacement => (
                "a result cannot live in its own parameters",
                "a closure result outlives each call's parameters (PROV-2)",
            ),
            Arena => (
                "it cannot leave the callback's arena",
                "the arena ends with this callback (PROV-4)",
            ),
            Mutate => ("this mutates it", "this mutates `{0}`"),
            Capture => ("the closure captures it", "this closure captures `{0}`"),
            OutlivesScope => (
                "it is used after that scope ends",
                "this use outlives the temporary's scope",
            ),
            Part => (
                "the aggregate's part requires it",
                "a part's declared modes are contracts of the aggregate (MODE-2, MODE-3)",
            ),
        }
    }
}

/// One side of an explanation, at its own site.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fact<K> {
    pub kind: K,
    pub span: Span,
    pub names: Vec<String>,
}
impl<K> Fact<K> {
    pub fn new(kind: K, span: Span, names: &[&str]) -> Self {
        Self {
            kind,
            span,
            names: names.iter().map(|n| n.to_string()).collect(),
        }
    }
}

/// Repairs, ranked by runtime cost, then by edit radius: a body edit, a
/// signature edit, a deep copy, and last a change of meaning. Only repairs
/// the checker accepts for that cause and requirement are listed.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Repair {
    /// Conditional whole-owner consumption; never an automatic edit.
    ConsumingMatch(String),
    /// A prose recipe, never a machine edit: every field is explicitly bound.
    WholeRecord {
        owner: String,
        statement: String,
    },
    /// Resource destruction changes cleanup semantics (LANG-13/DROP).
    ResourcePart(String),
    /// No concrete recipe is established for this owner/projection.
    WholeOwnerOnly(String),

    /// Checked per-item extraction, or conditional guidance when a concrete
    /// independent copy is not established by the diagnostic producer.
    IterationItem {
        item: String,
        independent_copy: bool,
    },
    /// A declaration edit, conditional on the existing body contract check.
    PreserveUnique {
        helper: String,
        parameter: String,
    },
    /// A Shared root's copy is a counted, global handle (MEM-11).
    CopyShared(String),
    UseOwner {
        owner: String,
    },
    DropBrand(String),
    ReadDirectly(String),
    TakeParameter(String),
    LocalResult {
        parameter: String,
    },
    PlaceParameter {
        parameter: String,
        arena: String,
    },
    PlaceResult {
        arena: String,
    },
    HeapParameter(String),
    DeepCopy(String),
    /// A read-only immutable snapshot, selected from current flow evidence.
    CaptureSnapshot {
        source: String,
        saved: String,
        deep_copy: bool,
    },
    /// No concrete snapshot recipe is established for this mutable capture.
    MutableCaptureLocal {
        source: String,
        evidence: String,
    },
    /// Conditional guidance when independence cannot be proved for a capture.
    KeepCaptureLocal(String),
    TakeChangesMeaning(String),
    /// A redundant `copy` (R6): pass the binding itself.
    PassBorrowed(String),
    /// An owned binding at its last use.
    TakeAtLastUse(String),
    /// A counted part copied out of storage that is about to end.
    CopyPart(String),
    /// A callback result whose counted parts cross when unique.
    UniqueResult {
        ty: String,
        expression: String,
    },
}
impl Repair {
    fn text(&self) -> String {
        match self {
            Repair::ConsumingMatch(owner) => format!(
                "if consuming the owner is intended, use `match take {owner}` at the inspected match; this consumes the whole `{owner}` and gives its payloads to the selected arm. Check later uses of `{owner}` and drop behavior on every arm and early exit; the whole-owner transfer needs no deep copy"
            ),
            Repair::WholeRecord { owner, statement } => format!(
                "replace the enclosing field-extraction statement with `{statement}` and update later field uses to the new bindings; this consumes all of `{owner}`, so `{owner}` cannot be used later. Bind every still-needed field explicitly; this recipe binds every field and omits none"
            ),
            Repair::ResourcePart(owner) => format!(
                "`{owner}` is a resource: whole destructuring is governed by LANG-13/DROP and can suppress its parent finalizer; bound fields move and omitted fields drop in declaration order. Use the resource's checked API or deliberately design its whole destruction; this is not an equivalent field-extraction repair"
            ),
            Repair::WholeOwnerOnly(owner) => format!(
                "moving a part requires an accessible whole owned value and consumes that whole owner; `{owner}` cannot then be used later. Borrowed owners cannot be consumed; indexed elements and nested or boxed projections need their own checked whole-owner pattern. Hidden, external or generic representations need a checked API or ownership contract; no concrete pattern is established here. Bind every still-needed field explicitly, and check LANG-13/DROP for resources before whole destruction"
            ),
            Repair::IterationItem {
                item,
                independent_copy: true,
            } => format!(
                "use `unique {item}` for this append input (deep copy per item: may allocate and costs O(item size)); this copies the item instead of the complete accumulator"
            ),
            Repair::IterationItem {
                item,
                independent_copy: false,
            } => format!(
                "supply an independently unique `{item}` only if its element contract permits it; a per-item deep copy requires concrete copyable data, permitted allocation and compatible placement. Generic or resource elements need a checked ownership contract; arena bounds remain and allocating copies are unavailable under `@noalloc`"
            ),
            Repair::PreserveUnique { helper, parameter } => format!(
                "declare `@preserves_unique({parameter})` on `{helper}` only if its body checks the contract: the selected parameter must stay deeply unique at every exit, including failure handback; other non-plain `take` inputs must also be unique at this call (no cost; checked exported signature promise)"
            ),
            Repair::UniqueResult { ty, expression } => format!(
                "return a `unique {ty}` value with `unique {expression}` (deep copy: may allocate and costs O(size))"
            ),
            Repair::CopyShared(x) => {
                format!("`copy {x}` (no deep copy: a copied Shared root is global)")
            }
            Repair::UseOwner { owner } => {
                format!("use the value's own owner `{owner}` here (no cost)")
            }
            Repair::DropBrand(x) => format!("declare `{x}` without a brand (no cost)"),
            Repair::ReadDirectly(x) => format!("read {x} directly, without the closure (no cost)"),
            Repair::TakeParameter(x) => format!(
                "declare the parameter `take {x}` (no cost; callers write `take {x}`, which moves their value into this call)"
            ),
            Repair::LocalResult { parameter } => format!(
                "declare the result `{}` (no cost; signature)",
                qualifier(Qualifier::In(&[parameter]))
            ),
            Repair::PlaceParameter { parameter, arena } => format!(
                "declare `{parameter}` `{}` (no cost; signature)",
                qualifier(Qualifier::In(&[arena]))
            ),
            Repair::PlaceResult { arena } => format!(
                "declare the result `{}` (no cost; signature)",
                qualifier(Qualifier::In(&[arena]))
            ),
            Repair::HeapParameter(parameter) => format!(
                "declare `{parameter}` `{}` (no cost; signature)",
                qualifier(Qualifier::Heap)
            ),
            Repair::DeepCopy(x) => format!(
                "`unique {x}` (deep copy: allocates, costs O(size), and is not allowed under `@noalloc`)"
            ),
            Repair::CaptureSnapshot {
                source,
                saved,
                deep_copy,
            } => {
                let value = if *deep_copy {
                    format!("unique copy {source}")
                } else {
                    source.clone()
                };
                let cost = if *deep_copy {
                    "this independent deep copy may allocate and costs O(size); it requires permitted allocation"
                } else {
                    "this plain value copy needs no allocation"
                };
                format!(
                    "before creating the closure, write `let {saved} = {value}` and capture `{saved}` instead of `{source}` for read-only uses. This is a snapshot of the current value, not shared mutable state: it does not observe future writes to `{source}`; {cost}. The escaping closure's environment has its own allocation requirements"
                )
            }
            Repair::MutableCaptureLocal { source, evidence } => format!(
                "keep `{source}` within its owner's scope, or construct an independently owned immutable value before creating the closure. No concrete snapshot recipe is established here: {evidence}. Check copyability, locality, current uniqueness and allocation first; resources need a checked API, an ordinary counted copy retains binding loans, and deep copying keeps arena/region bounds and borrowed callable captures. Allocating copies are unavailable under `@noalloc`; a live mutable capture cannot be taken. Any read-only snapshot records the current value and does not observe future writes or provide shared mutable state"
            ),
            Repair::KeepCaptureLocal(x) => format!(
                "keep `{x}` within its source owner's scope, or construct an independent owned value; `unique` removes binding loans only for copyable counted data, and retains arena/region bounds and borrowed callable captures"
            ),
            Repair::TakeAtLastUse(x) => {
                format!("`take {x}` here (no cost: it moves `{x}`, which is not used again)")
            }
            Repair::PassBorrowed(x) => {
                format!("pass `{x}` itself (no cost: the parameter only borrows it)")
            }
            Repair::TakeChangesMeaning(x) => {
                format!(
                    "declare the parameter `take {x}` (changes meaning: the caller loses `{x}`)"
                )
            }
            Repair::CopyPart(x) => {
                format!("`copy {x}` (no deep copy: the copy shares the part by a count)")
            }
        }
    }
}
/// Repairs that look plausible and that the checker rejects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Caution {
    /// Each nested inspection has its own borrowing decision.
    NestedMatch,
    /// A copy of local content shares its blocks, which stay local (P2).
    CopyStaysLocal(String),
    /// A deep copy is made where its source lives and keeps its placement.
    CopyKeepsPlacement(String),
    /// Deep copies do not promote scoped storage or clone borrowed environments.
    CaptureBounds,
    CaptureCopy(String),
}
impl Caution {
    fn text(&self) -> String {
        match self {
            Caution::NestedMatch => "Each inspected match borrows its own scrutinee; consuming an outer owner alone does not make a nested match consume its scrutinee".into(),
            Caution::CopyStaysLocal(x) => format!(
                "`copy {x}` does not help: a copy shares the blocks of `{x}`, which stay local"
            ),
            Caution::CaptureCopy(x) => format!("when copying is allowed, `copy {x}` retains binding loans; it cannot promote borrowed content to global storage"),
            Caution::CaptureBounds => "A unique copy retains arena/region bounds and borrowed callable captures; changing the enclosing parameter to `take` does not make a borrowed closure capture owned".into(),
            Caution::CopyKeepsPlacement(x) => {
                format!("`unique {x}` and `copy {x}` keep the placement of `{x}` and do not help")
            }
        }
    }
}

/// Both sides of one failed judgment, with its repairs and root cause.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Explanation {
    /// The provided side, from the observed value back to its root cause.
    pub provided: Vec<Fact<Cause>>,
    pub required: Option<Fact<Requirement>>,
    /// A prior access failure may also know the receiving storage contract.
    pub also_required: Vec<Fact<Requirement>>,
    pub repairs: Vec<Repair>,
    pub cautions: Vec<Caution>,
    /// Definition-local identity of the provided proof, shared across its
    /// requirements. Producers must use the same provided source site too.
    pub root: Option<u64>,
    /// A mechanical edit whose source expression is known by the producer.
    pub fix: Option<hirsel_diag::Fix>,
}
/// A name that is a description, not source text: it is not quoted.
pub fn prose(text: &str) -> String {
    format!("{PROSE}{text}")
}
const PROSE: char = '\u{1}';
fn fill(template: &str, names: &[String]) -> String {
    let mut out = template.to_owned();
    let unnamed = [prose("the value")];
    let names = if names.is_empty() { &unnamed } else { names };
    for (i, name) in names.iter().enumerate() {
        match name.strip_prefix(PROSE) {
            Some(text) => {
                out = out.replace(&format!("`{{{i}}}`"), text);
                out = out.replace(&format!("{{{i}}}"), text);
            }
            None => out = out.replace(&format!("{{{i}}}"), name),
        }
    }
    out
}
impl Explanation {
    /// Recover expression names only for otherwise anonymous value facts. This
    /// uses syntax on the error path and cannot change a flow judgment.
    pub fn name_values(&mut self, sites: Sites<'_>) {
        for fact in &mut self.provided {
            if fact.names.is_empty()
                && matches!(
                    fact.kind,
                    Cause::Aliased | Cause::NotSendable | Cause::Noncopyable
                )
            {
                let name = sites
                    .syntax
                    .tree
                    .expressions
                    .iter()
                    .enumerate()
                    .find_map(|(i, e)| {
                        (e.origin.span == fact.span)
                            .then(|| spell(&sites.syntax.tree, ExprId(i as u32)))
                            .flatten()
                    });
                fact.names.push(name.unwrap_or_else(|| prose("the value")));
            }
        }
    }
    pub fn provided(mut self, fact: Fact<Cause>) -> Self {
        self.provided.push(fact);
        self
    }
    pub fn required(mut self, fact: Fact<Requirement>) -> Self {
        self.required = Some(fact);
        self
    }
    /// The root cause: the last provided fact.
    pub fn cause(&self) -> Option<&Fact<Cause>> {
        self.provided.last()
    }
    /// Two-sided message and labels; the rule and primary span never change.
    pub fn render(mut self, diagnostic: &mut Diagnostic) {
        if diagnostic.fix.is_none() {
            diagnostic.fix = self.fix.take();
        }
        let provided = self.cause().map(|f| fill(f.kind.text().0, &f.names));
        let required = self
            .required
            .as_ref()
            .map(|f| fill(f.kind.text().0, &f.names));
        diagnostic.message = match (provided, required) {
            (Some(p), Some(r)) => format!("{}: {p}, but {r}", diagnostic.message),
            (Some(p), None) => format!("{}: {p}", diagnostic.message),
            (None, Some(r)) => format!("{}: {r}", diagnostic.message),
            (None, None) => diagnostic.message.clone(),
        };
        for (i, fact) in self.provided.iter().rev().enumerate() {
            let prefix = if i == 0 { "provided" } else { "then" };
            diagnostic.secondary.push(hirsel_diag::Label {
                span: fact.span,
                message: format!("{prefix}: {}", fill(fact.kind.text().1, &fact.names)),
            });
        }
        for fact in self.required.iter().chain(&self.also_required) {
            diagnostic.secondary.push(hirsel_diag::Label {
                span: fact.span,
                message: format!("required: {}", fill(fact.kind.text().1, &fact.names)),
            });
        }
        self.repairs.sort_by_key(Repair::rank);
        self.repairs.dedup();
        for (i, repair) in self.repairs.iter().enumerate() {
            diagnostic
                .notes
                .push(format!("help: {}. {}", i + 1, repair.text()));
        }
        for caution in &self.cautions {
            diagnostic.notes.push(caution.text());
        }
        // docs/spec/diagnostics.md: the provided cause and its site alone identify a cascade.
        // Keep the definition-local root, and distinguish different creation
        // sites (for example two moves of the same reinitialised binding).
        if diagnostic.root_cause.is_none() {
            diagnostic.root_cause = self.root.map(|root| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                root.hash(&mut hasher);
                if let Some(fact) = self.cause() {
                    std::mem::discriminant(&fact.kind).hash(&mut hasher);
                    // Anchor-relative offsets: the root already names the
                    // definition, and an anchor differs between loads.
                    (fact.span.start, fact.span.end).hash(&mut hasher);
                    fact.names.hash(&mut hasher);
                }
                hasher.finish()
            });
        }
    }
}
impl Repair {
    /// Runtime cost first, then edit radius.
    fn rank(&self) -> u8 {
        match self {
            Repair::UseOwner { .. }
            | Repair::DropBrand(_)
            | Repair::ReadDirectly(_)
            | Repair::CopyShared(_)
            | Repair::PassBorrowed(_)
            | Repair::TakeAtLastUse(_)
            | Repair::CopyPart(_)
            | Repair::CaptureSnapshot {
                deep_copy: false, ..
            } => 0,
            Repair::PreserveUnique { .. }
            | Repair::TakeParameter(_)
            | Repair::LocalResult { .. }
            | Repair::PlaceParameter { .. }
            | Repair::PlaceResult { .. }
            | Repair::HeapParameter(_) => 1,
            Repair::DeepCopy(_)
            | Repair::UniqueResult { .. }
            | Repair::IterationItem { .. }
            | Repair::CaptureSnapshot {
                deep_copy: true, ..
            } => 2,
            Repair::ConsumingMatch(_)
            | Repair::TakeChangesMeaning(_)
            | Repair::KeepCaptureLocal(_)
            | Repair::MutableCaptureLocal { .. }
            | Repair::WholeRecord { .. }
            | Repair::ResourcePart(_)
            | Repair::WholeOwnerOnly(_) => 3,
        }
    }
}

/// The ranked repair table for a locality failure, keyed by cause and
/// requirement. Every row is a probe the checker accepts (moonshot-diagnostics,
/// verify-moon2): a borrowed parameter reaching global storage or escaping
/// (1b/1c, 3b/3g, 7b/7c), a `mut` parameter reaching a global result
/// (2b/2c/2d), and a copied Shared root, which is global (cu/c2-c4). A copy of
/// any other local value stays local, and a `mut` parameter cannot be
/// declared `in x` for itself (1e), so neither is ever suggested.
/// `tests/it/repair_probes.rs` additionally verifies all eight formerly unprobed
/// global-receiver pairs, including a borrowed parameter's global result.
pub fn locality_repairs(
    cause: Cause,
    required: Requirement,
    x: &str,
    shared: bool,
) -> (Vec<Repair>, Vec<Caution>) {
    use Requirement as R;
    let x = x.to_owned();
    match (cause, required) {
        // The producer may strengthen the conditional guidance to DeepCopy
        // only from concrete copyable counted data without placement/scope bounds.
        // A capture never becomes owned by changing its source parameter.
        (Cause::BorrowedCapture, required)
            if global(required) || matches!(required, R::Owned | R::OwnedResult) =>
        {
            if shared {
                (vec![Repair::CopyShared(x)], vec![])
            } else {
                (
                    vec![Repair::KeepCaptureLocal(x.clone())],
                    vec![Caution::CaptureCopy(x), Caution::CaptureBounds],
                )
            }
        }
        (Cause::BorrowedParameter, R::MutStorage | R::ResultGlobal | R::EscapingCapture)
            if shared =>
        {
            (vec![Repair::CopyShared(x)], vec![])
        }
        (Cause::BorrowedParameter, required) if !shared && global(required) => (
            vec![
                Repair::TakeParameter(x.clone()),
                Repair::DeepCopy(x.clone()),
            ],
            vec![Caution::CopyStaysLocal(x)],
        ),
        (Cause::MutParameter, R::ResultGlobal) if !shared => (
            vec![
                Repair::LocalResult {
                    parameter: x.clone(),
                },
                Repair::DeepCopy(x.clone()),
                Repair::TakeChangesMeaning(x.clone()),
            ],
            vec![Caution::CopyStaysLocal(x)],
        ),
        // repair_probes.rs covers every formerly caution-only pair. For a
        // mut parameter the signature change consumes the caller's storage;
        // an expression `take x` still fails OWN-1 and is never suggested.
        (Cause::MutParameter, required) if !shared && global(required) => (
            vec![
                Repair::DeepCopy(x.clone()),
                Repair::TakeChangesMeaning(x.clone()),
            ],
            vec![Caution::CopyStaysLocal(x)],
        ),
        _ => (vec![], vec![]),
    }
}
fn global(required: Requirement) -> bool {
    matches!(
        required,
        Requirement::ResultGlobal
            | Requirement::MutStorage
            | Requirement::GlobalBinding
            | Requirement::AnnotatedGlobal
            | Requirement::EscapingCapture
            | Requirement::GlobalArgument
    )
}
/// The repair table for a parameter placed in the wrong arena (probes 6a,
/// 6b, 6g); a deep copy keeps its placement (6c, 6d, 6f), so `unique` is never
/// suggested for placement.
pub fn placement_repairs(
    parameter: &str,
    from: &str,
    to: &str,
    result_follows: bool,
) -> (Vec<Repair>, Vec<Caution>) {
    let mut repairs = vec![Repair::PlaceParameter {
        parameter: parameter.to_owned(),
        arena: to.to_owned(),
    }];
    if result_follows {
        repairs.push(Repair::PlaceResult {
            arena: from.to_owned(),
        });
    }
    repairs.push(Repair::HeapParameter(parameter.to_owned()));
    (
        repairs,
        vec![Caution::CopyKeepsPlacement(parameter.to_owned())],
    )
}

/// A definition-local root-cause key: equal roots in one definition fold.
pub fn root_key(definition: &DefPath, root: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    definition.hash(&mut hasher);
    root.hash(&mut hasher);
    hasher.finish()
}

/// Names and declaration sites from the syntax tree. An IR built without
/// syntax (a hand-built checker fixture) has no sites; its labels fall back to
/// an empty span instead of failing the check.
pub struct Sites<'a> {
    pub syntax: &'a hirsel_syntax::ItemSyntax,
}
impl Sites<'_> {
    pub fn name(&self, symbol: hirsel_intern::Symbol) -> String {
        symbol.as_str().to_owned()
    }
    pub fn signature(&self, owner: NameSite) -> Option<&hirsel_syntax::FunctionSyntax> {
        match owner {
            NameSite::Item => match &self.syntax.item.kind {
                hirsel_syntax::ItemKind::Function { signature, .. } => Some(signature),
                _ => None,
            },
            NameSite::Statement(statement) => {
                match &self.syntax.tree.statements.get(statement.0 as usize)?.kind {
                    hirsel_syntax::StmtKind::Function { signature, .. } => Some(signature),
                    _ => None,
                }
            }
            NameSite::Expression(expression) => {
                match &self
                    .syntax
                    .tree
                    .expressions
                    .get(expression.0 as usize)?
                    .kind
                {
                    ExprKind::Closure { signature, .. } => Some(signature),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    pub fn owner_span(&self, owner: NameSite) -> Span {
        let tree = &self.syntax.tree;
        match owner {
            NameSite::Item => Some(self.syntax.item.name_span),
            NameSite::Statement(statement) => tree
                .statements
                .get(statement.0 as usize)
                .map(|n| n.origin.span),
            NameSite::Expression(expression) => tree
                .expressions
                .get(expression.0 as usize)
                .map(|n| n.origin.span),
            NameSite::Pattern(pattern) => {
                tree.patterns.get(pattern.0 as usize).map(|n| n.origin.span)
            }
            NameSite::Type(ty) => self.type_span(ty),
        }
        .unwrap_or_default()
    }
    fn type_span(&self, ty: hirsel_syntax::TypeExprId) -> Option<Span> {
        self.syntax
            .tree
            .types
            .get(ty.0 as usize)
            .map(|n| n.origin.span)
    }
    /// A parameter's name and its written type's site. A generated body (CT-5)
    /// binds the parameter by its function literal's name, at that name's site.
    pub fn parameter(&self, owner: NameSite, parameter: ParamId) -> Option<(String, Span)> {
        if let NameSite::Item = owner
            && let Some(hirsel_syntax::Generated::Body(names)) = &self.syntax.item.generated
        {
            let &(name, span) = names.get(parameter.0 as usize)?;
            return Some((name.map_or_else(|| "_".into(), |s| self.name(s)), span));
        }
        let syntax = self
            .signature(owner)?
            .parameters
            .get(parameter.0 as usize)?;
        Some((
            syntax.name.map_or_else(|| "_".into(), |s| self.name(s)),
            self.type_span(syntax.ty)?,
        ))
    }
    /// A binding's name and declaring site: a parameter's type or its pattern.
    pub fn binding(&self, binding: &BindingDecision) -> (String, Span) {
        if let ResolvedName::Parameter(parameter) = binding.binding {
            if let Some(site) = self.parameter(binding.owner, parameter) {
                return site;
            }
        }
        match binding.site {
            NameSite::Pattern(pattern)
                if (pattern.0 as usize) < self.syntax.tree.patterns.len() =>
            {
                let node = &self.syntax.tree.patterns[pattern.0 as usize];
                let name = match node.kind {
                    hirsel_syntax::PatternKind::Binding(symbol) => self.name(symbol),
                    _ => "_".into(),
                };
                (name, node.origin.span)
            }
            NameSite::Statement(statement)
                if (statement.0 as usize) < self.syntax.tree.statements.len() =>
            {
                let node = &self.syntax.tree.statements[statement.0 as usize];
                let name = match node.kind {
                    hirsel_syntax::StmtKind::Function { name, .. } => self.name(name),
                    _ => "_".into(),
                };
                (name, node.origin.span)
            }
            site => ("_".into(), self.owner_span(site)),
        }
    }
    /// A result's written type site, else its function's site.
    pub fn result(&self, owner: NameSite) -> Span {
        self.signature(owner)
            .and_then(|s| s.result)
            .and_then(|ty| self.type_span(ty))
            .unwrap_or_else(|| self.owner_span(owner))
    }
}

/// Short source spellings for headlines; None where a label shows the site.
pub fn spell(tree: &SyntaxTree, expression: ExprId) -> Option<String> {
    let name = |s: hirsel_intern::Symbol| Some(s.as_str().to_owned());
    let mut depth = 0;
    fn go(
        tree: &SyntaxTree,
        name: &dyn Fn(hirsel_intern::Symbol) -> Option<String>,
        e: ExprId,
        depth: &mut usize,
    ) -> Option<String> {
        *depth += 1;
        if *depth > 6 {
            return None;
        }
        Some(match &tree.expressions.get(e.0 as usize)?.kind {
            ExprKind::Path(path) => path
                .segments
                .iter()
                .map(|s| name(*s))
                .collect::<Option<Vec<_>>>()?
                .join("."),
            ExprKind::Field { base, field } => {
                format!("{}.{}", go(tree, name, *base, depth)?, name(*field)?)
            }
            ExprKind::Index { base, index } => format!(
                "{}[{}]",
                go(tree, name, *base, depth)?,
                go(tree, name, *index, depth)?
            ),
            ExprKind::Unary { op, value } => {
                let op = match op {
                    UnaryOp::Take => "take ",
                    UnaryOp::Copy => "copy ",
                    UnaryOp::Unique => "unique ",
                    UnaryOp::Mut => "mut ",
                    UnaryOp::Box => "box ",
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "!",
                    UnaryOp::BitNot => "~",
                };
                format!("{op}{}", go(tree, name, *value, depth)?)
            }
            ExprKind::Call { callee, arguments } if arguments.len() <= 3 => format!(
                "{}({})",
                go(tree, name, *callee, depth)?,
                arguments
                    .iter()
                    .map(|a| go(tree, name, a.value, depth))
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            ),
            ExprKind::Sequence(items) if items.len() <= 3 => format!(
                "[{}]",
                items
                    .iter()
                    .map(|i| go(tree, name, *i, depth))
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            ),
            ExprKind::Literal(Literal::Integer(text) | Literal::Float(text)) => text.clone(),
            ExprKind::Literal(Literal::Bool(b)) => b.to_string(),
            _ => return None,
        })
    }
    go(tree, &name, expression, &mut depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_diag::RuleId;
    use hirsel_span::Anchor;

    fn span(start: u32) -> Span {
        Span::new(Anchor(0), start, start + 1).unwrap()
    }
    /// The spec's cause vocabulary table as plain text, one string per side.
    fn spec_table() -> (String, String) {
        let spec = include_str!("../../../docs/spec/diagnostics.md");
        let start = spec
            .find("\n### Cause vocabulary\n")
            .expect("cause vocabulary");
        let table = &spec[start..];
        let side = |name: &str| {
            table
                .lines()
                .find(|row| row.starts_with(&format!("| {name} |")))
                .unwrap()
                .replace('`', "")
        };
        (side("Provided fact") + &side("Step"), side("Required fact"))
    }
    #[test]
    fn every_cause_kind_has_a_spec_kind_from_the_section_23_table() {
        use Cause::*;
        let kinds = [
            BorrowedParameter,
            MutParameter,
            TakenParameter,
            Capture,
            CreationInput,
            Binding,
            Holds,
            LocalResult,
            Temporary,
            FrameStorage,
            Environment,
            Unplaced,
            LocalValue,
            Placed,
            NoPlacement,
            Branded,
            Unbranded,
            IdentityEnded,
            Moved,
            Uninitialised,
            Lent,
            Immutable,
            MutAlias,
            Published,
            Once,
            Aliased,
            Noncopyable,
            ScopedClosure,
            ScopedCapability,
            NotSendable,
            Authority,
            Resource,
            MutableCapture,
            OperandBorrows,
            Part,
            ArgumentLoan,
            Iterated,
            FinalizerResource,
            Packed,
            TakenOwner,
            ClosureOwn,
            SiblingOwner,
            Header,
            MutLend,
            NotMut,
            NotTaken,
            BrandForgotten,
            ScopedPlacement,
            BorrowedCapture,
            ReceiverCapture,
            ReadOnlyParameter,
            AuthorityCapture,
            LastUse,
            BlockBinding,
            Sendable,
            UniquenessLoss,
            UniquenessMerge,
            InspectedMatch,
            BorrowedPattern,
            IterationItem,
        ];
        let seen: std::collections::BTreeSet<_> = kinds.iter().map(|kind| *kind as usize).collect();
        assert_eq!(seen, (0..=BorrowedPattern as usize).collect());
        let (provided, _) = spec_table();
        for kind in kinds {
            assert!(
                provided.contains(kind.spec_kind()),
                "{kind:?}: {:?} is not a provided cause kind",
                kind.spec_kind()
            );
        }
    }
    #[test]
    fn every_requirement_kind_has_a_spec_kind_from_the_section_23_table() {
        use Requirement::*;
        let kinds = [
            ResultGlobal,
            ResultSources,
            ResultNoTemporary,
            ResultTakenSource,
            MutStorage,
            StatePreservation,
            CreationEnvelope,
            GlobalBinding,
            AnnotatedBinding,
            Argument,
            EscapingCapture,
            Outlives,
            FunctionSlot,
            Exclusive,
            Write,
            Owned,
            OwnedResult,
            TakeIntoClosure,
            Unique,
            Many,
            Sendable,
            Brand,
            Placement,
            Heap,
            Read,
            Copy,
            DeepCopy,
            StoreClosure,
            ReturnClosure,
            MutArgument,
            TakeArgument,
            ConstructorOperand,
            Uses,
            NoResources,
            Index,
            Header,
            NoOwnPlacement,
            Arena,
            Mutate,
            Capture,
            OutlivesScope,
            Part,
            NoPlacement,
            BrandedUse,
            OwnerOutlives,
            StoreValue,
            ReturnValue,
            Inert,
            UnbrandedParameter,
            TaskCapture,
            GlobalArgument,
            AnnotatedGlobal,
            Borrows,
            Within,
            OutlivesBlock,
            SendableType,
            NotSendableType,
        ];
        let seen: std::collections::BTreeSet<_> = kinds.iter().map(|kind| *kind as usize).collect();
        assert_eq!(seen, (0..=NotSendableType as usize).collect());
        let (_, required) = spec_table();
        for kind in kinds {
            assert!(
                required.contains(kind.spec_kind()),
                "{kind:?}: {:?} is not a required fact kind",
                kind.spec_kind()
            );
        }
    }
    #[test]
    fn qualifiers_have_one_spelling() {
        assert_eq!(qualifier(Qualifier::In(&["x", "y"])), "in {x, y}");
        assert_eq!(qualifier(Qualifier::In(&[])), "in env");
        assert_eq!(qualifier(Qualifier::In(&["a"])), "in a");
        assert_eq!(qualifier(Qualifier::Brand("s")), "of s");
        assert_eq!(qualifier(Qualifier::Heap), "in heap");
    }
    #[test]
    fn descriptive_qualifiers_keep_prose_out_of_source_spelling() {
        let temporary = prose("a temporary");
        for (qualifier, expected) in [
            (Qualifier::Brand(&temporary), "requires of a temporary"),
            (Qualifier::In(&[&temporary]), "requires in a temporary"),
            (
                Qualifier::In(&["arena", &temporary]),
                "requires in {arena, a temporary}",
            ),
        ] {
            assert_eq!(
                fill("requires `{0}`", &[super::qualifier(qualifier)]),
                expected
            );
        }
    }
    #[test]
    fn different_provided_creation_sites_and_definitions_never_share_a_key() {
        let key = |root, cause, site| {
            let mut d = Diagnostic::error(const { RuleId::of("MODE-1") }, span(9), "base");
            Explanation {
                root: Some(root),
                ..Explanation::default()
            }
            .provided(Fact::new(cause, span(site), &["v"]))
            .render(&mut d);
            d.root_cause.unwrap()
        };
        assert_ne!(key(7, Cause::Moved, 1), key(7, Cause::Moved, 2));
        assert_ne!(key(7, Cause::Moved, 1), key(8, Cause::Moved, 1));
        assert_ne!(key(7, Cause::Moved, 1), key(7, Cause::Uninitialised, 1));
    }
    #[test]
    fn one_provided_callback_result_keeps_its_key_across_requirements() {
        let key = |required, requirement_site, field: &str| {
            let mut d = Diagnostic::error(const { RuleId::of("MODE-4") }, span(9), "base");
            Explanation {
                root: Some(7),
                ..Explanation::default()
            }
            .provided(Fact::new(Cause::NotSendable, span(1), &[field]))
            .required(Fact::new(required, span(requirement_site), &[]))
            .render(&mut d);
            d.root_cause.unwrap()
        };
        let generic = key(Requirement::Sendable, 2, "Payload.left");
        assert_eq!(generic, key(Requirement::SendableType, 3, "Payload.left"));
        assert_ne!(generic, key(Requirement::Sendable, 2, "Payload.right"));
    }
}
