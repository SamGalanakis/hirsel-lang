//! How a run ended, as its host receives it. Every ending of a run, in either engine, is one
//! [`RunOutcome`] returned to the host that started it; nothing below the host exits the
//! process. Only a standalone launcher turns the outcome into a process exit status, through
//! [`RunOutcome::exit_code`].

/// The budget a run spent (`RunOutcome::BudgetExceeded`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetKind {
    Fuel,
    Memory,
}

/// What an entry returned: its `core@1/Result[(), E]` tag. The `Error` payload's PROG-3 report
/// is already on the run's standard error sink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Returned {
    Ok,
    Error,
}

/// A source position named by a program's failure table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Site {
    pub definition: String,
    pub file: String,
    pub line: u64,
    pub column: u64,
}

/// One language failure record (FAIL-1): its rule ID, the failing site and the operation site
/// when it differs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub rule: String,
    pub site: Site,
    pub operation: Option<Site>,
}

/// A host resource that ended the run rather than anything the program observes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Environmental {
    /// A canonical codec value past its operational byte or value budget (SER-2).
    CodecLimit,
    /// The run's recording could not be written after an effect was issued.
    Recording(String),
    /// A host output sink failed.
    Output(String),
}

/// How a run ended. Closed: a host handles every ending, and both engines produce the same
/// variants for the same ending.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// The entry returned.
    Returned(Returned),
    /// A language failure: the primary record, then its causes in order.
    Failed {
        primary: Failure,
        causes: Vec<Failure>,
    },
    /// Every task waits on another and nothing outside the run can wake one (CONC-8). No user
    /// code, cancellation or destruction ran after it.
    Blocked,
    /// A run budget was spent; the run stopped like a Blocked run.
    BudgetExceeded(BudgetKind),
    /// The host refused the run before entry, or an adapter refused a request mid-run.
    Refused(String),
    /// A replay's requests left its recording (mismatch, missing or unconsumed events).
    Diverged(String),
    /// A host resource ended the run.
    Environmental(Environmental),
    /// The host's own reporting failed after the run ended (an observation or report sink).
    Tooling(String),
}

/// Process exit statuses of a standalone launch, one per [`RunOutcome`] class.
pub mod exit {
    pub const RETURNED_OK: i32 = 0;
    pub const RETURNED_ERROR: i32 = 1;
    /// A language failure (FAIL-1).
    pub const FAILED: i32 = 2;
    /// A refused launch or request, a replay divergence or a tooling failure.
    pub const REFUSED: i32 = 3;
    pub const BLOCKED: i32 = 4;
    pub const BUDGET_EXCEEDED: i32 = 5;
    pub const ENVIRONMENTAL: i32 = 6;
}

impl RunOutcome {
    /// The single exit policy: the status a standalone launcher exits with for this outcome.
    /// Hosts that run several runs never exit for one of them.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Returned(Returned::Ok) => exit::RETURNED_OK,
            Self::Returned(Returned::Error) => exit::RETURNED_ERROR,
            Self::Failed { .. } => exit::FAILED,
            Self::Refused(_) | Self::Diverged(_) | Self::Tooling(_) => exit::REFUSED,
            Self::Blocked => exit::BLOCKED,
            Self::BudgetExceeded(_) => exit::BUDGET_EXCEEDED,
            Self::Environmental(_) => exit::ENVIRONMENTAL,
        }
    }
}
