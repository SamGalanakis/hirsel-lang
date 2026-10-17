//! Deterministic reference execution of ownership-explicit TIR.
pub mod aggregate;
pub mod checked;
mod constants;
pub mod conversion;
mod eval;
mod frame;
pub mod library;
pub mod observation;
mod resources;
pub mod scalar;
pub mod structural;
pub mod value;

pub use eval::{run, run_with_operations, run_with_runtime};
use hirsel_diag::RuleId;
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::DefPath;
use hirsel_tir::{BlockId, CleanupAction, RunConfiguration};
use hirsel_types::{GenericArgument, QueryError, TypeId, TypeStore};
pub use value::{Integer, Value};

/// Limits are explicit inputs, independent of host stack size or wall time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub steps: u64,
    pub call_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 1_000_000,
            call_depth: 256,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub rule: RuleId,
    pub line: u32,
    pub site: Span,
    pub kind: String,
}
/// Reference execution stopped at a violated unchecked precondition (#49).
/// This is outside DET-1; it carries no cleanup or parity guarantee.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndefinedBehavior {
    pub kind: String,
    pub operation: DefPath,
    pub site: Span,
    pub line: u32,
    pub contract: String,
}
impl std::fmt::Display for UndefinedBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} at line {}: {}",
            self.kind,
            self.operation
                .0
                .modules
                .iter()
                .chain(std::iter::once(&self.operation.0.name))
                .cloned()
                .collect::<Vec<_>>()
                .join("."),
            self.line,
            self.contract
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// CONC-8: stopped without exit, failure or speculative destruction.
    Blocked(Vec<BlockedWait>),
    ToolingLimit(hirsel_tir::ToolingLimit),
    UndefinedBehavior(UndefinedBehavior),
    Returned {
        value: Value,
        exit_code: i32,
    },
    Failed(Failure),
    /// Checked execution found the memory model broken; nothing further ran.
    Trapped(checked::Trap),
}
/// One suspended root-task wait; ordinals identify runtime-owned targets only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockedWait {
    pub spawn_path: Vec<u32>,
    pub anchor: observation::Anchor,
    pub kind: String,
    pub targets: Vec<u64>,
}
/// Detailed evaluator events are optional; semantic effects are always retained.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TraceMode {
    #[default]
    Full,
    Effects,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceEvent {
    /// Reference-engine accounting probe, excluded from semantic observations.
    LoopOwnership {
        released: usize,
        live: usize,
    },
    Effect(observation::OrderedEffect),
    ToolingLimit(hirsel_tir::ToolingLimit),
    Enter {
        path: DefPath,
        depth: usize,
    },
    Instruction {
        depth: usize,
        block: BlockId,
        index: u32,
        site: Span,
    },
    Terminator {
        depth: usize,
        block: BlockId,
        site: Span,
    },
    Cleanup {
        depth: usize,
        action: CleanupAction,
    },
    Return {
        path: DefPath,
        depth: usize,
    },
    Failure(Failure),
    /// Operation modules may append stable, explicitly encoded events.
    Operation {
        name: String,
        bytes: Vec<u8>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Execution {
    pub failures: Vec<observation::FailureRecord>,
    pub outcome: Outcome,
    pub stdout: Vec<u8>,
    pub trace: Vec<TraceEvent>,
    pub steps: u64,
}
/// An execution API error retains all output produced before it stopped.
#[derive(Clone, Debug)]
pub struct ExecutionError {
    pub error: Error,
    pub stdout: Vec<u8>,
}
impl From<Error> for ExecutionError {
    fn from(error: Error) -> Self {
        Self {
            error,
            stdout: vec![],
        }
    }
}
/// Invalid/unavailable TIR is an API error, never a fabricated language failure.
#[derive(Clone, Debug)]
pub enum Error {
    /// Internal stop signal, converted to Outcome::Blocked before returning.
    Blocked(Vec<BlockedWait>),
    InvalidTir(String),
    Unavailable(String),
    Query(QueryError),
    /// Checked execution's stop signal. The run entry points turn it into
    /// `Outcome::Trapped`; it never leaves them as an error.
    Trapped(Box<checked::Trap>),
}
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidTir(message.into())
}

/// A transferred task environment, entered on its own evaluator stack.
pub struct TaskStart {
    pub id: u64,
    pub callee: Value,
}
// One inventory defines the extension boundary and its forwarding adapter. A new
// operation hook therefore delegates automatically instead of inheriting a no-op.
macro_rules! operation_boundary {
    (shared { $( $(#[$smeta:meta])* fn $shared:ident(&self $(, $sa:ident: $st:ty)*) -> $sr:ty $sb:block)* }
     mutable { $( $(#[$mmeta:meta])* fn $mutable:ident(&mut self $(, $ma:ident: $mt:ty)*) $(-> $mr:ty)? $mb:block)* }
     required { $( $(#[$rmeta:meta])* fn $required:ident(&mut self $(, $ra:ident: $rt:ty)*) -> $rr:ty;)* }) => {
        /// Extension boundary for the library: names and signatures come from its registry.
        /// Return None for an ordinary TIR definition. Arguments are in parameter order,
        /// already extracted with their explicit use modes. Returning Failed unwinds the
        /// call's cleanup; it must carry the defining operation's rule and source site.
        pub trait Operations {
            $($(#[$smeta])* fn $shared(&self, $($sa: $st),*) -> $sr $sb)*
            $($(#[$mmeta])* fn $mutable(&mut self, $($ma: $mt),*) $(-> $mr)? $mb)*
            $($(#[$rmeta])* fn $required(&mut self, $($ra: $rt),*) -> $rr;)*
        }
        /// An operation layer overrides only the hooks it intercepts. Every other
        /// hook, including future additions, forwards to the underlying provider.
        pub trait OperationsAdapter {
            fn operations(&self) -> &dyn Operations;
            fn operations_mut(&mut self) -> &mut dyn Operations;
            $(fn $shared(&self, $($sa: $st),*) -> $sr {
                self.operations().$shared($($sa),*)
            })*
            $(fn $mutable(&mut self, $($ma: $mt),*) $(-> $mr)? {
                self.operations_mut().$mutable($($ma),*)
            })*
            $(fn $required(&mut self, $($ra: $rt),*) -> $rr {
                self.operations_mut().$required($($ra),*)
            })*
        }
        impl<T: OperationsAdapter> Operations for T {
            $(fn $shared(&self, $($sa: $st),*) -> $sr {
                OperationsAdapter::$shared(self, $($sa),*)
            })*
            $(fn $mutable(&mut self, $($ma: $mt),*) $(-> $mr)? {
                OperationsAdapter::$mutable(self, $($ma),*)
            })*
            $(fn $required(&mut self, $($ra: $rt),*) -> $rr {
                OperationsAdapter::$required(self, $($ra),*)
            })*
        }
    };
}
operation_boundary! {
    shared {
    fn choose_task(&self, _runnable: &[u64]) -> Result<usize, Error> {
        Ok(0)
    }
    /// A detached result is destroyed on the child's stack before completion.
    fn discard_task_result(&self, _id: u64) -> bool { false }
    fn handles_failure(&self, _token: u64) -> bool {
        false
    }
    }
    mutable {
    /// Publish callback snapshots even on failure/abandonment; return displaced owners.
    fn callback_finished(&mut self,
        _token: u64,
        _updates: &[(usize, Value)],
        _abandoned: bool) -> Result<Vec<Value>, Error> {
        Ok(vec![])
    }
    /// Owners displaced by runtime cleanup, in creation order.
    fn take_releases(&mut self) -> Vec<Value> {
        vec![]
    }
    fn take_tasks(&mut self) -> Vec<TaskStart> {
        vec![]
    }
    fn enter_task(&mut self, _id: u64, _shielded: bool) {}
    fn task_resumed(&mut self, _id: u64, _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Ok(())
    }
    fn task_finished(&mut self,
        _id: u64,
        _result: OperationResult,
        _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Err(invalid("unexpected task completion"))
    }
    fn take_yield(&mut self) -> bool {
        false
    }
    fn poll(&mut self,
        _token: u64,
        _context: &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error> {
        Err(invalid("unexpected parked operation"))
    }
    fn drop_handle(&mut self,
        _kind: &str,
        _id: u64,
        _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Ok(())
    }
    fn resume(&mut self,
        _continuation: u64,
        _result: OperationResult,
        _context: &mut OperationContext<'_>) -> Result<OperationResult, Error> {
        Err(invalid("operation continuation is not implemented"))
    }
    /// Wait for external readiness only after runnable work is exhausted.
    /// False means no external source can make progress (CONC-8).
    fn wait(&mut self) -> Result<bool, Error> { Ok(false) }
    }
    required {
    fn call(&mut self,
        call: OperationCall<'_>,
        _context: &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error> ;
    }
}
pub struct OperationCall<'a> {
    pub path: &'a DefPath,
    pub generics: &'a [GenericArgument],
    pub arguments: &'a [Value],
    pub result_type: TypeId,
    pub types: &'a TypeStore,
    pub site: Span,
}
pub struct OperationContext<'a> {
    pub configuration: &'a RunConfiguration,
    pub sources: &'a SourceMap,
    pub stdout: &'a mut Vec<u8>,
    pub trace: &'a mut Vec<TraceEvent>,
    pub trace_mode: TraceMode,
    /// Current operation site, independent of optional diagnostic tracing.
    pub anchor: Option<&'a observation::Anchor>,
}
/// A mutable callback parameter writes directly into a saved operation argument.
/// Field ordinals address nested library state, independently of callback outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallbackWriteback {
    pub parameter: usize,
    pub argument: usize,
    pub fields: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationResult {
    /// Runtime-owned wait. The evaluator retains the operation and all its loans.
    Park {
        token: u64,
        wait: BlockedWait,
    },
    /// An already-unwound task or isolation boundary preserves every cause/site.
    FailureChain {
        failures: Vec<observation::FailureRecord>,
        released: Vec<Value>,
    },
    Call {
        callee: Value,
        arguments: Vec<Value>,
        mut_arguments: Vec<usize>,
        /// Restored after callback cleanup on success, failure and tooling exit.
        writebacks: Vec<CallbackWriteback>,
        /// Saved mut-argument state after transferring values into `owned`.
        updates: Vec<(usize, Value)>,
        /// Owned operation temporaries, in creation order. The scheduler releases
        /// them high to low after callback cleanup, before resume, including failure.
        owned: Vec<Value>,
        /// Incoming taken owners moved into callback temporaries or saved updates.
        consumed: Vec<usize>,
        continuation: u64,
    },
    Complete {
        value: Value,
        arguments: Vec<(usize, Value)>,
        /// Taken argument ordinals transferred into the result or saved updates.
        /// All remaining taken arguments are released, on every exit.
        consumed: Vec<usize>,
        /// Displaced/removed values, in creation order, released after publication.
        released: Vec<Value>,
    },
    Failed(Failure),
    UndefinedBehavior(UndefinedBehavior),
}
pub struct NoOperations;
impl Operations for NoOperations {
    fn call(
        &mut self,
        _: OperationCall<'_>,
        _: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        Ok(None)
    }
}

impl OperationContext<'_> {
    pub fn anchor(&self) -> Option<&observation::Anchor> {
        self.anchor
    }
    pub fn consume(&mut self, event: hirsel_tir::CapabilityEvent) -> Result<(), Error> {
        let anchor = self
            .anchor()
            .cloned()
            .ok_or_else(|| invalid("operation has no execution anchor"))?;
        self.trace
            .push(TraceEvent::Effect(observation::OrderedEffect {
                anchor,
                effect: observation::Effect::CapabilityConsumed(event),
            }));
        Ok(())
    }
}

impl OperationResult {
    pub fn returned(value: Value) -> Self {
        Self::Complete {
            value,
            arguments: vec![],
            consumed: vec![],
            released: vec![],
        }
    }
    pub fn transferring(mut self, arguments: impl IntoIterator<Item = usize>) -> Self {
        let Self::Complete { consumed, .. } = &mut self else {
            panic!("ownership transfers commit only on operation completion")
        };
        consumed.extend(arguments);
        self
    }
}
