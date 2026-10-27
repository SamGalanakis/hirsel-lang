//! Deterministic reference execution of ownership-explicit TIR.

#[cfg(test)]
hirsel_guard::install!();

pub mod aggregate;
mod bounded;
mod budget;
pub mod checked;
mod code;
mod constants;
pub mod conversion;
mod depth;
mod eval;
pub mod exit_report;
mod frame;
#[doc(hidden)]
pub mod inventory;
pub mod library;
pub mod observation;
pub mod ownership_trace;
mod parked;
mod profile;
mod provider;
mod resources;
mod runs;
pub mod scalar;
mod sealed_storage;
pub mod structural;
pub mod type_arguments;
pub mod value;

pub use budget::RunBudgets;
pub use eval::{run, run_with_operations, run_with_runtime, run_with_runtime_async};
use hirsel_diag::RuleId;
pub use hirsel_run_schema::BudgetKind;
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::DefPath;
use hirsel_tir::{BlockId, CleanupAction, RunConfiguration};
use hirsel_types::{GenericArgument, QueryError, TypeId, TypeInterner};
pub use profile::with_step_profile;
pub use provider::{AsyncTirProvider, ImmediateTirProvider};
pub use value::{Integer, Value};

/// Default tooling step bound for the reference evaluator and corpus fixtures.
pub const DEFAULT_STEP_LIMIT: u64 = 1_000_000;
/// Limits are explicit inputs, independent of host stack size or wall time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// None leaves execution steps unbounded; call depth is still checked.
    pub steps: Option<u64>,
    pub call_depth: usize,
    /// A compile-time evaluation's source budget (CT-4): the `Charge` units it
    /// may spend. None charges nothing; run time has no source budget.
    pub source: Option<u64>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: Some(DEFAULT_STEP_LIMIT),
            call_depth: usize::MAX,
            source: None,
        }
    }
}
/// A compile-time evaluation spent its source budget: the next charged event
/// was denied before it executed, and nothing further ran (CT-4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exhaustion {
    pub budget: u64,
    pub event: hirsel_tir::SourceEvent,
    /// The definition the denied event belongs to.
    pub definition: DefPath,
    pub site: Span,
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
    /// The run spent a host budget (`budgets.limits@2`) and stopped where it was, as a
    /// Blocked run does: no cleanup, finalizer or further effect ran.
    BudgetExceeded(BudgetKind),
    ToolingLimit(hirsel_tir::ToolingLimit),
    /// A compile-time evaluation spent its source budget; nothing further ran.
    Exhausted(Exhaustion),
    UndefinedBehavior(UndefinedBehavior),
    Returned {
        value: Value,
        exit_code: i32,
    },
    Failed(Failure),
    /// Checked execution found the memory model broken; nothing further ran.
    Trapped(checked::Trap),
    /// A host resource stopped the run; nothing further ran (`RunOutcome::Environmental`).
    Environmental(hirsel_run_schema::Environmental),
    /// A scripted signal took its default action (`RunOutcome::Signaled`); nothing further
    /// ran, as for Blocked.
    Signaled(hirsel_run_schema::SignalKind),
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
    /// Simulator diagnostics, outside semantic observations and the Io tape.
    RaceCandidate(crate::library::race::RaceCandidate),
    SimulationPoint(crate::library::simulation::SchedulePoint),
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
    /// Source-budget units charged (CT-4); zero without a source budget.
    pub charged: u64,
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
    /// Internal stop signal, converted to Outcome::BudgetExceeded before returning.
    BudgetExceeded(BudgetKind),
    /// Internal stop signal, converted to Outcome::Exhausted before returning.
    Exhausted(Box<Exhaustion>),
    /// Internal stop signal, converted to Outcome::Signaled before returning.
    Signaled(hirsel_run_schema::SignalKind),
    InvalidTir(String),
    Unavailable(String),
    Query(QueryError),
    /// A host resource stopped the run (the codec's operational budget, SER-2); the run
    /// entry points turn it into `Outcome::Environmental`.
    Environmental(hirsel_run_schema::Environmental),
    /// Checked execution's stop signal. The run entry points turn it into
    /// `Outcome::Trapped`; it never leaves them as an error.
    Trapped(Box<checked::Trap>),
    /// A failure that ends the run where it stands, without unwinding (FAIL-3: a
    /// serializable check's failure). The run entry points report it as
    /// `Outcome::Failed`.
    Ended(Failure),
    /// A provider event failed at its saved script/input source, without a new call.
    ProviderFailed(Box<observation::FailureRecord>),
}
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidTir(message.into())
}

/// A callee run a runner call started in this process (RUN-10). The caller's provider
/// built its world; the execution runs it beside its caller (RUN-12).
pub struct CalleeStart {
    /// The caller's wait for the call's outcome.
    pub token: u64,
    pub configuration: RunConfiguration,
    pub arguments: Vec<Value>,
    pub operations: Box<dyn Operations>,
    pub budgets: RunBudgets,
    /// `sim@1`'s `crash` fault: the callee's place stops it at this point, counted in its
    /// requests where its provider makes them (`Operations::stops_itself`), else in its
    /// suspensions. A callee that ends first ends with its outcome withheld.
    pub crash_after: Option<u32>,
    /// The call as a host provider admits its callee run.
    pub call: CallStart,
}
/// A sent call as its callee run's record states it (RUN-10).
pub struct CallStart {
    /// The place: the runner entry the call went through.
    pub runner: hirsel_run_schema::RunnerEntry,
    /// The grant set the request selected.
    pub grants: hirsel_run_schema::IoGrants,
    pub form: hirsel_run_schema::CallForm,
    pub cause: hirsel_run_schema::Cause,
    pub argument: Vec<u8>,
    pub schemas: CallSchemas,
}
/// The codec schemas of a call's result and error, and of a run's `Failure`.
#[derive(Clone, Debug)]
pub struct CallSchemas {
    pub result: hirsel_types::serialization::SerializableSchema,
    pub error: hirsel_types::serialization::SerializableSchema,
    pub failure: hirsel_types::serialization::SerializableSchema,
}
/// How a callee run ended, as its caller's provider learns it.
#[derive(Debug)]
pub enum CalleeEnd {
    /// Its entry returned this `Result`.
    Returned(Value),
    /// It ended in a language failure: the chain, primary first.
    Failed(Vec<observation::FailureRecord>),
    BudgetExceeded(BudgetKind),
    /// Its call's cancellation arrived before it started.
    Canceled,
    /// Its place stopped it without an outcome (`sim@1`'s `crash`).
    Abandoned,
}

/// The watched waits a poll may complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Woken {
    /// Every parked wait, watched or not.
    Every,
    /// These tokens, in any order; any other watched wait stays parked unpolled.
    Tokens(Vec<u64>),
}

/// A holding child's result an operation discarded: the child drops `values` in a turn of
/// its own (its rest) before that operation returns (native `hand_off`).
pub struct TaskRest {
    pub child: u64,
    pub values: Vec<Value>,
}

/// A transferred task environment, entered on its own evaluator stack.
pub struct TaskStart {
    /// Actual spawn invocation, independent of inherited failure attribution.
    pub site: Option<hirsel_span::Span>,
    pub id: u64,
    /// The spawning stack, also when a provider poll publishes the task.
    pub parent: u64,
    pub callee: Value,
    /// Owned inputs moved into the child in parameter order.
    pub arguments: Vec<Value>,
    /// The scope retains an immutable mapper environment until every child rests.
    pub borrowed: bool,
}
// One inventory defines the extension boundary and its forwarding adapter. A new
// operation hook therefore delegates automatically instead of inheriting a no-op.
macro_rules! operation_boundary {
    (shared { $( $(#[$smeta:meta])* fn $shared:ident(&self $(, $sa:ident: $st:ty)*) -> $sr:ty $sb:block)* }
     mutable { $( $(#[$mmeta:meta])* fn $mutable:ident(&mut $mself:ident $(, $ma:ident: $mt:ty)*) $(-> $mr:ty)? $mb:block)* }
     required { $( $(#[$rmeta:meta])* fn $required:ident(&mut self $(, $ra:ident: $rt:ty)*) -> $rr:ty;)* }) => {
        /// Extension boundary for the library: names and signatures come from its registry.
        /// Return None for an ordinary TIR definition. Arguments are in parameter order,
        /// already extracted with their explicit use modes. Returning Failed unwinds the
        /// call's cleanup; it must carry the defining operation's rule and source site.
        pub trait Operations {
            $($(#[$smeta])* fn $shared(&self, $($sa: $st),*) -> $sr $sb)*
            $($(#[$mmeta])* fn $mutable(&mut $mself, $($ma: $mt),*) $(-> $mr)? $mb)*
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
    /// Resolve a path once for a call target. The decision is static for this run;
    /// None executes its TIR body without offering the call again. The default
    /// legacy token still offers calls to providers implementing only `call`.
    /// Adapters overriding `call` must also override `call_resolved`.
    fn resolve(&self, _path: &DefPath) -> Option<OperationId> { Some(OperationId::LEGACY) }
    /// This id cannot create task rests or require completion hooks. The evaluator
    /// may finish a Complete result without suspending it, provided no untransferred
    /// taken owners need release. Adapters introducing those effects override this
    /// promise. Checked and memory-metered runs retain scheduled completion.
    fn immediate(&self, _operation: OperationId) -> bool { false }
    /// Absolute deadline of a parked timer, independent of host readiness or replay.
    /// The scheduler polls timers by deadline, preserving suspension order on ties.
    fn wait_deadline(&self, _token: u64) -> Option<u64> { None }
    /// A detached result is destroyed on the child's stack before completion.
    fn discard_task_result(&self, _id: u64) -> bool { false }
    /// Whether finished child `id` holds a result nobody took yet: the evaluator keeps its
    /// frame, where a discard drops the result (`take_rests`).
    fn holds_result(&self, _id: u64) -> bool { false }
    /// A provider-owned child has no source frame. Its result cleanup still
    /// runs on that task through the evaluator's release worklist.
    fn frameless_task(&self, _child: u64) -> bool { false }
    fn handles_failure(&self, _token: u64) -> bool {
        false
    }
    /// This layer reports the parked wait `token` through [`woken`](Operations::woken):
    /// until a poll's `woken` names it, its poll returns None without effect. Asked once,
    /// when the wait parks. An unwatched wait is polled at every poll.
    fn watched(&self, _token: u64) -> bool { false }
    /// The logical bytes of the values this layer holds for the run (channel buffers,
    /// task results), measured with the evaluator's own (`budgets.limits@2`).
    fn retained_bytes(&self) -> u64 { 0 }
    /// Canonical provider task path, requested only by opt-in ownership observation.
    fn ownership_task_path(&self, _id: u64) -> Option<Vec<u32>> { None }
    /// Actual provider channel identity for a live endpoint, only queried by diagnostics.
    fn ownership_channel(&self, _endpoint: u64) -> Option<u64> { None }
    /// A callee run whose provider stops it at its place's crash point itself, before one
    /// of its requests (`sim@1` on a host), instead of the execution counting suspensions.
    fn stops_itself(&self) -> bool { false }
    /// Exact replay cannot issue cleanup requests after an artificial evaluator cut:
    /// the recording describes the uninterrupted run. Stop at the guard instead.
    /// Other providers retain their ordinary tooling cleanup.
    fn stop_at_tooling_limit(&self) -> bool { false }
    }
    mutable {
    /// Execute an id returned by this provider's `resolve`. The default retains
    /// the path-based adapter API for small custom providers.
    fn call_resolved(&mut self, _operation: OperationId, call: OperationCall<'_>,
        context: &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error> {
        self.call(call, context)
    }
    /// A scheduling point: the index in `runnable` (queue order, nonempty) of the task to run
    /// next, the task that ran last being current (`enter_task`).
    fn choose_task(&mut self, _runnable: &[u64]) -> Result<usize, Error> {
        Ok(0)
    }
    /// At every poll, before the parked tasks: cancel the scopes whose deadline is due
    /// (CONC-3), in deadline order with ties in entry order.
    fn expire_deadlines(&mut self, _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Ok(())
    }
    /// At every poll, after `expire_deadlines`: the watched waits whose poll may complete
    /// now, so the scheduler polls those and the unwatched ones, as the native scheduler
    /// re-issues only ready requests.
    fn woken(&mut self) -> Woken { Woken::Every }
    /// Publish the host's entry report before consuming its returned owner.
    /// False retains the raw evaluator's returned-value ownership policy.
    fn entry_returned(&mut self, _value: &Value, _runtime: &aggregate::RuntimeTypes,
        _context: &mut OperationContext<'_>) -> Result<bool, Error> { Ok(false) }
    /// Publish callback snapshots even on failure/abandonment; return displaced owners.
    fn callback_finished(&mut self,
        _token: u64,
        _updates: &[(usize, Value)],
        _abandoned: bool) -> Result<Vec<Value>, Error> {
        Ok(vec![])
    }
    /// Simulator-only extra suspension; production providers leave it disabled.
    fn simulation_point(&mut self, _point: library::simulation::SchedulePoint) -> bool { false }
    /// A root release has taken effect at its cleanup anchor.
    fn shared_released(&mut self, _root: u64, _context: &mut OperationContext<'_>) {}
    /// A release of a `Shared` root by the running task: true when the provider took it
    /// (free mode defers a release that leaves other holders to the task's next op).
    fn defer_release(&mut self, _root: &value::SharedRoot) -> bool {
        false
    }
    /// The root task's frames ended. True keeps the other tasks running until none can
    /// proceed (a free-mode replay, whose other tasks still run to their cut).
    fn root_finished(&mut self) -> Result<bool, Error> {
        Ok(false)
    }
    /// Owners displaced by runtime cleanup, in creation order.
    fn take_releases(&mut self) -> Vec<Value> {
        vec![]
    }
    /// The rule and kind of a language failure the last handle drop raised.
    fn take_drop_failure(&mut self) -> Option<(String, String)> {
        None
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
    /// The holding children whose results the pending operation of task `parent` discarded.
    /// The evaluator hands `parent`'s turn to each child in order, which drops its values,
    /// and continues `parent` when the last returns, with no scheduling point between.
    fn take_rests(&mut self, _parent: u64) -> Vec<TaskRest> {
        vec![]
    }
    /// Child `child`'s rest ended with the failures dropping its result raised.
    fn rest_finished(&mut self,
        _child: u64,
        _failures: Vec<observation::FailureRecord>) -> Result<(), Error> {
        Ok(())
    }
    /// The rests of `parent`'s pending operation returned; `result` is still pending.
    fn rests_returned(&mut self, _parent: u64, _result: &mut OperationResult) {}
    /// Finish a calling operation after all of its child cleanup frames returned.
    fn rests_completed(
        &mut self,
        _parent: u64,
        _result: &mut OperationResult,
        _context: &mut OperationContext<'_>
    ) -> Result<(), Error> {
        Ok(())
    }
    /// The task of the operation that returned `Park { token }` parked: polled from the
    /// next scheduling point on.
    fn parked(&mut self, _token: u64) {}
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
    /// A destructor's owned wait, taken immediately after drop_handle. Existing
    /// synchronous hooks need no continuation. A returned completion must be unit.
    fn take_drop_wait(&mut self) -> Option<OperationResult> { None }
    /// Resume with retained operation arguments; mutable owners can change in place.
    fn resume(&mut self,
        _continuation: u64,
        _result: OperationResult,
        _arguments: &mut [Value],
        _context: &mut OperationContext<'_>) -> Result<OperationResult, Error> {
        Err(invalid("operation continuation is not implemented"))
    }
    /// Wait for external readiness only after runnable work is exhausted.
    /// False means no external source can make progress (CONC-8).
    fn wait(&mut self) -> Result<bool, Error> { Ok(false) }
    /// Whether a parked operation is ready now, asked of every idle run of an execution
    /// before any of them waits (RUN-12); never blocks.
    fn ready_now(&mut self) -> Result<bool, Error> { Ok(false) }
    /// The callee runs this run's runner calls sent since the last take (RUN-10).
    fn take_calls(&mut self) -> Vec<CalleeStart> {
        vec![]
    }
    /// The waits of sent calls whose scope was cancelled since the last take: each
    /// call's cancellation is forwarded to its callee (RUN-9).
    fn take_cancels(&mut self) -> Vec<u64> {
        vec![]
    }
    /// The callee run of the call parked at `token` ended.
    fn call_ended(&mut self,
        _token: u64,
        _end: CalleeEnd,
        _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Err(invalid("unexpected callee outcome"))
    }
    /// A forwarded cancellation: cancel this callee run's root scope with cause `Parent`.
    fn cancel_run(&mut self) {}
    /// This run, a callee, ended: its provider records its outcome (RUN-11).
    fn callee_ended(&mut self,
        _end: &CalleeEnd,
        _runtime: &aggregate::RuntimeTypes,
        _context: &mut OperationContext<'_>) -> Result<(), Error> {
        Ok(())
    }
    }
    required {
    fn call(&mut self,
        call: OperationCall<'_>,
        _context: &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error> ;
    }
}
/// Provider-local dispatch token. Its meaning is stable for the life of a run.
/// The canonical name is static metadata, never assembled on the call path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId {
    pub index: usize,
    pub name: &'static str,
}
impl OperationId {
    /// Compatibility for a provider whose path decision is still made by `call`.
    pub const LEGACY: Self = Self {
        index: usize::MAX,
        name: "",
    };
}
pub struct OperationCall<'a> {
    /// Checked nominal declarations; incoming bytes can never supply these.
    pub runtime: &'a aggregate::RuntimeTypes,
    pub path: &'a DefPath,
    pub generics: &'a [GenericArgument],
    /// Values in parameter order. An operation may write a `mut` argument in
    /// place: the evaluator restores that argument to its caller's storage on
    /// every exit, exactly as it publishes `Complete.arguments`. It writes no
    /// other argument.
    pub arguments: &'a mut [Value],
    pub result_type: TypeId,
    pub types: &'a TypeInterner,
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
    /// The evaluator prepares/commits the already-admitted taken grant/root.
    SealArena {
        arena: u64,
        root_type: TypeId,
    },
    /// The evaluator fills the scheduled callback's root and retains its loan.
    BorrowRegion {
        owner: u64,
        root_type: TypeId,
        result: Box<OperationResult>,
    },
    /// Deferred arena charge, before transferring any incoming owners.
    ArenaCharge {
        arena: u64,
        bytes: u64,
        result: Box<OperationResult>,
    },
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
    fn resolve(&self, _: &DefPath) -> Option<OperationId> {
        None
    }

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
