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

/// A completed entry's status: the legacy 0/1 pair or an explicit unsigned byte code.
/// The `Error` payload's PROG-3 report is already on the run's standard error sink;
/// explicit status values do not produce an error report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Returned {
    Ok,
    Error,
    /// An explicit application status outside the legacy 0/1 pair.
    ExitCode(u8),
}
impl Returned {
    /// Preserve the legacy encoding for statuses 0 and 1.
    pub fn from_exit_code(code: u8) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Error,
            code => Self::ExitCode(code),
        }
    }
}

/// A source position named by a program's failure table.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// The run's place stopped it before its next request: a simulated place's crash
    /// (`sim@1`), which its caller sees as `InDoubt(Abandoned())` (RUN-6), or the embedding
    /// host's stop (`RunControl::stop`).
    Stopped,
}
impl std::fmt::Display for Environmental {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CodecLimit => f.write_str("codec operational budget exhausted"),
            Self::Recording(reason) => write!(f, "the recording: {reason}"),
            Self::Output(reason) => write!(f, "output: {reason}"),
            Self::Stopped => f.write_str("its place stopped the run"),
        }
    }
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
    Blocked(Vec<Wait>),
    /// A run budget was spent; the run stopped like a Blocked run.
    BudgetExceeded(BudgetKind),
    /// A signal the run did not capture arrived, or a second Interrupt while one was
    /// pending, in a scripted world (`signals.script@1`): the host's default action ends the
    /// run, which stops like a Blocked run. On a host the process itself dies by the signal.
    Signaled(crate::SignalKind),
    /// The host refused the run before entry, or an adapter refused a request mid-run.
    Refused(String),
    /// A replay's requests left its recording (mismatch, missing or unconsumed events).
    Diverged(String),
    /// A host resource ended the run.
    Environmental(Environmental),
    /// The host's own reporting failed after the run ended (an observation or report sink).
    Tooling(String),
    /// An artificial evaluator guard, outside language semantics.
    ToolingLimit(ToolingLimit),
    /// An unchecked precondition was violated; no cleanup or parity is promised.
    UndefinedBehavior(ExecutionDiagnostic),
    /// Checked execution found a broken memory-model invariant.
    Trapped(ExecutionDiagnostic),
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
    /// Added to a signal's number for a `Signaled` run whose launcher cannot end by the
    /// signal itself, as a shell reports a process a signal killed.
    pub const SIGNALED_BASE: i32 = 128;
}

impl RunOutcome {
    /// The single exit policy: the status a standalone launcher exits with for this outcome.
    /// Hosts that run several runs never exit for one of them.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Returned(Returned::Ok) => exit::RETURNED_OK,
            Self::Returned(Returned::Error) => exit::RETURNED_ERROR,
            Self::Returned(Returned::ExitCode(code)) => i32::from(*code),
            Self::Failed { .. } => exit::FAILED,
            Self::Refused(_)
            | Self::Diverged(_)
            | Self::Tooling(_)
            | Self::ToolingLimit(_)
            | Self::UndefinedBehavior(_)
            | Self::Trapped(_) => exit::REFUSED,
            Self::Blocked(_) => exit::BLOCKED,
            Self::BudgetExceeded(_) => exit::BUDGET_EXCEEDED,
            Self::Environmental(_) => exit::ENVIRONMENTAL,
            Self::Signaled(kind) => exit::SIGNALED_BASE + kind.number(),
        }
    }
}

/// A suspended task, using source coordinates and run-local target ordinals.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Wait {
    pub spawn_path: Vec<u32>,
    pub site: Site,
    pub kind: String,
    pub targets: Vec<u64>,
}

/// An execution diagnostic outside the language failure protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDiagnostic {
    pub rule: String,
    pub site: Site,
    pub message: String,
}
impl std::fmt::Display for ExecutionDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// An artificial interpreter limit, with an optional source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolingLimit {
    pub kind: String,
    pub limit: u64,
    pub site: Option<Site>,
}

impl Site {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "definition": self.definition, "file": self.file, "line": self.line, "column": self.column })
    }
    /// Render a complete FAIL-1 diagnostic, including its initiating operation.
    pub fn report(&self, rule: &str, json: bool, operation: Option<&Self>) -> String {
        if json {
            let mut report = serde_json::json!({ "schema_version": 1, "runtime_failure": { "rule": rule, "site": self.json() } });
            if let Some(operation) = operation {
                report["runtime_failure"]["operation"] = operation.json();
            }
            report.to_string()
        } else {
            let mut text = format!("runtime failure[{rule}] at {self}");
            if let Some(operation) = operation {
                text.push_str(&format!("\n  in {operation}"));
            }
            text
        }
    }
}
impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}:{}:{})",
            self.definition, self.file, self.line, self.column
        )
    }
}

impl Wait {
    /// Encode the host's complete parked-wait report for a native observation record.
    pub fn encode(waits: &[Self]) -> String {
        serde_json::to_string(waits).expect("wait records serialize")
    }
    /// Decode a parked-wait report without executing any program code.
    pub fn decode(text: &str) -> Result<Vec<Self>, String> {
        serde_json::from_str(text).map_err(|error| error.to_string())
    }
}
