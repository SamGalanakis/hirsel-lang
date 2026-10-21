//! What the executor rejects. A violation ends the run: it is a defect in the
//! generated code (or in the executor's input), never a language failure.
use hirsel_syntax::DefPath;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ViolationKind {
    // Holder credits and counts.
    /// A holder credit outlived its frame or storage: an omitted decrement.
    LeakedCredit,
    /// An owning use of a value with no credit left: a double consume, or the
    /// release of a borrow.
    CreditUnderflow,
    /// A block's actual count differs from its ideal holders.
    CountMismatch,
    /// The terminal audit found a live allocation with no exemption record.
    UnexplainedAllocation,
    // Access checks.
    UseAfterFree,
    /// A handle whose generation is older than its allocation's (reused storage).
    StaleGeneration,
    UninitializedRead,
    /// A drop or reset program visited bytes that are not initialized.
    UninitializedScan,
    /// Pointer-sized bytes read as a handle without a checked handle there.
    ForgedPointer,
    /// A value, layout or operand of the wrong representation.
    TypeConfusion,
    /// An access outside its allocation's extent.
    OutOfExtent,
    /// A store over initialized storage, which would lose its content.
    StoreOverInitialized,
    /// A write into a counted block that another holder can observe.
    WriteShared,
    /// A write into published uncounted storage.
    WritePublished,
    /// A borrowed reference stored where a holder credit is required.
    BorrowedStore,
    /// A borrowed value or frame address returned to the caller.
    BorrowedReturn,
    /// A borrowed value used after its loan ended or before it began.
    BorrowExpired,
    /// A borrowed view used after its backing owner changed (a detach).
    ViewDetached,
    /// A prepared address used after its place moved (a growth, a detach, a
    /// replaced block): native code keeps the location its preparation found.
    StaleAddress,
    // Raw reuse tokens.
    LeakedToken,
    /// A token consumed a second time.
    TokenReused,
    /// A token whose storage generation changed after it was minted.
    StaleReuseGeneration,
    /// A token installed into a layout of different capacity, alignment or domain.
    IncompatibleReuse,
    /// A reset of storage that cannot donate (uncounted, static, borrowed).
    IneligibleDonor,
    /// A token copied into storage.
    TokenStored,
    /// A reuse skipped the store of a field whose storage did not already
    /// hold the operand's bits (`Reuse::kept`).
    SkippedStore,
    // Resources and handles.
    /// A finalizer entered for a resource that is not live.
    DuplicateFinalizer,
    /// A resource's storage destroyed before its finalizer ran.
    ResourceNotFinalized,
    OpenResource,
    OpenHandle,
    /// A handle closed twice.
    DoubleClose,
    // Regions and frames.
    /// Storage of a compiler-local region reachable after its exit.
    RegionEscape,
    /// A region still live when its frame exits or at the terminal audit.
    LiveRegion,
    /// A frame slot's address used after the frame exited.
    FrameStorageEscape,
    /// A slot's storage ended while it still held a holder or resource.
    LeakedSlot,
    /// A loan still active when its frame exits.
    LoanOutlivesFrame,
    /// Logical frame admission and leave do not pair up.
    FrameImbalance,
    // Failure protocol.
    /// A frame returned normally after a failure edge was taken.
    FailureSwallowed,
    /// A frame propagated failure without one being active.
    PropagateWithoutFailure,
    UnreachableReached,
    /// The independent physical tally and full recount disagree: an H defect.
    InternalAudit,
    // Input shape.
    /// Referential integrity, linking, stage vocabulary or an undefined value.
    Malformed,
    /// An operation this executor or M1b does not execute; never a pass.
    Unsupported,
    /// A detected MEM-9 precondition violation, outside the parity domain.
    UndefinedBehavior,
    /// Every task of the run waits on another (CONC-8): no violation but the run's
    /// Blocked observation, which `run` reports without the terminal audit.
    Blocked,
    /// A scripted signal's default action ended the run: like Blocked, no violation but the
    /// run's observation.
    Signaled,
}

/// Where a violation was detected: the unit, block and statement (the
/// statement equal to the block's instruction count is its terminator).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Site {
    pub definition: DefPath,
    pub block: u32,
    pub statement: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub message: String,
    /// None for terminal-audit and harness findings.
    pub site: Option<Site>,
}

impl Violation {
    pub fn new(kind: ViolationKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            site: None,
        }
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)?;
        if let Some(site) = &self.site {
            write!(
                f,
                " at {:?} block {} statement {}",
                site.definition, site.block, site.statement
            )?;
        }
        Ok(())
    }
}

/// Violations are rare and large: boxed so the success path stays small.
pub(crate) type Checked<T> = Result<T, Box<Violation>>;

pub(crate) fn err(kind: ViolationKind, message: impl Into<String>) -> Box<Violation> {
    Box::new(Violation::new(kind, message))
}

pub(crate) fn fail<T>(kind: ViolationKind, message: impl Into<String>) -> Checked<T> {
    Err(err(kind, message))
}
