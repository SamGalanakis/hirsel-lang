# hirsel-interp

Execute ownership-explicit TIR through `TirProvider` as the deterministic semantic
reference. Dependencies are shared data crates and the pure LeafKernel crate only. There is no host I/O, clock,
randomness, pointer identity, or hash-table traversal in the evaluator.

`run(provider, sources, configuration, arguments, limits)` returns `Execution`:
returned value and exit code, language `Failure { rule, line, site, kind }`, or
ToolingLimit, plus stdout bytes, ordered semantic effects, an optional diagnostic
trace, and the number of executed steps. `RuntimeTypes.trace_mode` defaults to
`Full` for low-level diagnostic clients; `Effects` retains only semantic effects,
using no trace storage for pure computation. Outcomes, stdout, failure chains,
step accounting and cleanup are identical in both modes. `SourceMap` must retain
the TIR's immutable source snapshot: line numbers come from its spans, never
guessed from offsets. Invalid/unavailable IR and provider errors return
`ExecutionError`, retaining the original `Error` and stdout bytes. Inputs and
operation results canonicalize float NaNs; equality of executions includes float
bits and the selected trace. Semantic observations are identical across trace modes.

## Core invariants

- Bodies, instructions, operands, constructor fields, call arguments, and cleanup
  actions execute in stored order. Field/parameter ordinals determine layout,
  independently of evaluation order. Types remain in their owning definition store.
- A slot contains `Option<Value>`. Take clears it; Copy/Borrow/Mut read it; Assign
  requires dead storage. Replace atomically installs a prepared value and puts old content in its declared displaced temporary; explicit Drop follows. Borrow checking and loan validity are lowering's job;
  loan instructions have no scalar runtime effect. StorageLive requires a dead
  slot. Snapshot reads its already lowered operand's explicit use mode.
- Calls use caller temporaries already prepared by earlier instructions. Entry
  transfers Take values into callee parameter slots. A failing later argument
  preparation executes that instruction's cleanup without entering the callee.
  Failure after entry unwinds callee cleanup, then each suspended call's cleanup.
- Return extracts its operand before its cleanup. Conditional drops inspect live
  bits. Scope nesting does not infer drops. Full diagnostic cleanup traces
  include every listed action (including skipped conditional drops) in order.
- The frame stack is a Vec driven by one loop. Hirsel calls never recurse on the
  host stack. Short circuit operators, ranges/iterators, `?`, nested returns,
  and self-tail loop decisions arrive from checking/lowering.
- Match selects the first stored arm at or after its start index by directly
  observing typed pattern projections. Guards enter explicit blocks and restart
  the same table after false; binding instructions commit only after success.
  Switch handles scalar/tag CFG operations such as propagation and iteration.
  Tuple, record, distinct, variant, sequence and box fields can be projected
  and individually moved. Closure environments retain capture ownership.
- SEM-1 uses checked mathematical arithmetic in every width, signed division
  toward zero, and dividend-signed remainder. SEM-2 uses separate binary32/64
  operations with round-to-nearest-even, canonical quiet NaNs, signed zeros and
  gradual underflow; no fusion or reassociation. SEM-3 string comparison is over
  unchanged UTF-8 bytes; Bytes can contain arbitrary bytes. TIR literals are
  decoded already, so lexical escaping and invalid source UTF-8 belong to A.

## Modules and extension points

- `value.rs`: public observable values, sized integer bit vectors and NaN
  normalization, aggregates, resources and closures.
- `scalar.rs`: TIR literal, unary and binary operations, with typed scalar errors.
  Ownership unary operators and eager logical operators are invalid TIR here.
- `conversion.rs`: representation adapters for the shared pure exact/representability and low-bit integer conversion kernels
  across all widths and signedness. These choose no library spellings or allowed
  widening signatures. M can wrap `exact` as widen/Option/checked failure and use
  `truncate` once the signatures in #18 are pinned.
- `frame.rs`: slots, operand extraction, projections, assignment and simple drop.
  L can extend projection and drop behavior here without changing CFG scheduling.
- `eval.rs`: the small scheduler, rvalue/instruction/terminator dispatch and ordered
  cleanup worklist. L adds its rvalues and cleanup handlers at these matches. Its
  finalizers must also schedule frames/work items, continue field release after a
  failed finalizer, and preserve the original failure; do not recurse per call.
- `run_with_operations`/`Operations`: M implements registry-backed named calls in
  a separate module. Calls receive values in parameter order, the caller's owning
  TypeStore/result TypeId, generic arguments and source site. Context holds only
  explicit configuration/events/source snapshots plus stdout/trace sinks, the
  trace mode and a borrowed current operation anchor. The anchor is independent
  of tracing; it is never recovered by searching debug events. Return
  None without changing context for ordinary TIR definitions; return a value or
  a rule/site-bearing Failure for a recognized operation. The scheduler owns
  continuation and failure cleanup. M need not rewrite the scheduler.

The core dispatches no named numeric/string operations. In particular integer
conversion signatures, float-to-integer behavior, explicit fused math, IEEE bits,
float min/max/total order, scalar views are dependency
owned by M against docs/spec/core-library.md (which settles #18). There is no conversion opcode in frozen `Rvalue`.
Region cleanup is unavailable until its owning runtime task. Unavailable features are
reported explicitly, never silently approximated.

## Explicit limit and exit policies

`Limits` defaults to 1,000,000 steps and 256 calls. One instruction or terminator
costs one step; entering a frame and mandatory cleanup cost no steps. Exhaustion
is reported before the next scheduled operation and still runs cleanup. Goto/Branch/Switch/Match have no failure list in the frozen TIR, so limit exhaustion
on those edges runs no current-frame actions, then unwinds suspended calls.
Depth counts the root frame; an over-limit callee is entered, then fails at its
first scheduled site, so transferred arguments are covered by its emitted entry
cleanup. At most one over-limit frame is allocated. These local adapter policies
are provisional under [#34](https://github.com/SamGalanakis/hirsel-lang/issues/34):
Revision 1 classifies these as ToolingLimitKind::Steps/CallDepth with no RuleId;
trace and outcome distinguish them from language failures. A logical-frame-byte limit is future
work, distinct from the required call-depth host-stack safeguard.

The entry Result lowering uses Ok ordinal 0 (exit 0), Error ordinal 1 (exit 1).
Other return values are core fixtures and exit 0. v20 PROG-3 fixes exit 0/1; M supplies the nominal Result registry mapping.
The driver must supply correctly lowered root capability arguments; the core does
not grant capabilities on its own.

SEM-1's unsettled shift details use bit-vector left shift, sign-extending signed
right shift, invalid negative counts failing SEM-1, and representable MIN % -1
returning zero. These choices are recorded in
[#32](https://github.com/SamGalanakis/hirsel-lang/issues/32) and comments at the
implementation. Signed constants carry width-sized two's-complement bits.

## Validation

`cargo test -p hirsel-interp` runs scalar boundary tests, ordered
control-flow/call/cleanup tests, an explicit 2,000-call fixture, budget/depth
failures, and a resource-cleanup program whose trace and observable projection
are checked over 100 reruns. `tests/support` is a crate-local hand-built TIR
builder, with no shared API changes. These fixtures validate the interpreter
boundary; the full source pipeline remains the driver's integration.

Revision 1 keeps Arc DefinitionRef handles in frames and accepts dyn providers.
The shared operational-error variant is retained as Error::Query, while InvalidTir
remains a compiler error. TIR's LocalId is a storage ID, distinct from resolver
LocalId. Execution observations and richer cause/effect records remain local
extensions of the library; the static validator is `hirsel-validate`.

## Runtime aggregates, resources and observations

`run_with_runtime(provider, sources, configuration, arguments, limits, operations,
&RuntimeTypes)` executes records, payload variants, tuples, boxes, sequences,
distinct values, resources and closures in the same iterative scheduler. Existing
`run`/`run_with_operations` use empty nominal metadata. The driver supplies
`RuntimeTypes.nominals` from declared type signatures, with resource/distinct flags,
record arity and an optional `(DefPath, BodyId)` for each finalizer. The resource
owner's lowered Definition contains that body. Named record/variant construction
without metadata is unavailable, preventing a missing resource signature from
silently becoming an ordinary record. `RuntimeTypes.forget` names the registered
intrinsic; its one argument must be Take, a resource from the caller's defining
module. Forget suppresses both the finalizer and all owned-field destruction.

`Value::Sequence { kind, elements }` represents Array/List/Inline/Simd. `Value::Map`
contains optional `(key,value)` tuple entries in insertion order; `Value::Set`
contains optional elements in insertion order. M owns their named operations;
structural map equality is order-sensitive, set equality and hashing ignore order.
Opaque `Handle { kind, id }` is for capability/runtime identity, never host addresses.
The registry chooses tags and deterministic IDs; no behavior is inferred from tags.

Semantic copying uses an iterative tree copy. This reference model preserves value
semantics independently of RC implementation costs: the native RC pass, holder
counts, COW allocation audits and reuse are separate oracles. Mut arguments transfer
into frame slots and write back on success or failure, without rollback. Library
`Complete.arguments` updates are validated before publication. Projection and whole-value
writes still require explicit replacement drops. Loan validity, copying bounds,
unique access and source pattern legality belong to checked lowering/validation.
The evaluator also rejects explicit copies of resource-containing or once values.

Patterns execute from typed Body match tables, with no AST or decision structure.
Selection borrows existing storage by reference; explicit binder uses implement
copy, take and loan effects after selection. This covers wildcard/name/unit,
literal (including negative/wide integer), tuple, nullary/payload constructor,
labelled/rest and guarded patterns. Captures initialize `Storage::Capture` locals
in declaration order, independently of ordinary parameters. Scoped captures borrow,
escaping captures copy/take, and Indirect invokes the specified definition/body.
Owned captures release with their closure until invocation moves them into the
callee frame. Once-environment snapshots share an invocation cell, so a scoped
once callback can consume its captures without its saved environment releasing
them again. This cell is interpreter ownership metadata, not program storage;
many-use environments have no cell. Frames retain Arc DefinitionRef snapshots.

Cleanup runs its list in emitted order; fields, active variant payloads, sequence
entries and closure environments release in reverse order (v20 DROP-2). Each release
is a work item, and finalizers are ordinary scheduled frames: no host recursion.
A failing finalizer abandons its body, releases its fields and continues the pending
cleanup. The first failure remains primary; later cleanup failures extend
`Execution.failures` through indexed `cause` links. Normal replacement cleanup stops
the body after a failure. A resource self parameter remains borrowed throughout its
finalizer and cannot be consumed; finalizers must return unit.

`TraceEvent::Effect(OrderedEffect)` records resource construction, finalizer entry,
forget, handle cleanup and capability consumption in execution order. Each carries
an Anchor with definition, body, site and optional `(CleanupId, action ordinal)`.
Moves are not construction events; skipped conditional drops have no semantic effect.
M records capability inputs/effects with `OperationContext::consume(event)`; the
scheduler supplies the current borrowed anchor on the operation context before
dispatch, resume or handle cleanup. `context.anchor()` retrieves that input
independently of diagnostic tracing. Direct standalone operation tests can supply
their own anchor.

`Execution::observable(&SourceMap)` exports semantic observation v1: typed value
nodes in a flat arena, exact output, ordered effects and indexed failures carrying
kind, rule, stable source site, spawn path and cause links. `equivalent` excludes
function values until behavior probes are supplied. Body/cleanup IDs and steps
are excluded. `LaunchedExecution::observation` also includes final simulation
trees and surviving logical grants. The driver adds canonical declared result
types. Tooling limits have no semantic observation; partial output remains available
to the tool API.

`structural::{binary, compare, hash}` use iterative worklists. IEEE NaN/signed-zero
rules survive nesting. Hashing uses keyed SipHash-1-3 over a stable length-framed
encoding; set members have a canonical sorted encoding. Order helpers are available
to explicit dictionaries, not implicit Order derivation. Type bounds must authorize
an operation; resources/functions/handles and float keys are rejected. The provisional
MEM-3 boundary is 64 native plus 1024 scratch levels, root depth zero; exceeding 1088
edges fails MEM-3/Depth at the operation and runs its cleanup. The numeric constants
remain [#16](https://github.com/SamGalanakis/hirsel-lang/issues/16), not settled spec.
The existing step/depth ToolingLimit accounting and cleanup policy remains #34;
finalizer bodies and callbacks use the same limits.

## Library callbacks and handle cleanup

`Operations::drop_handle(kind, id, context)` defaults to no-op. The scheduler invokes
it when an owned Handle releases, including resource fields, in drop order. Its
anchor and HandleDropped effect precede the hook. M closes File/aborts AtomicFile
through this hook; host state stays in the operation implementation.

An operation may return `OperationResult::Call { callee, arguments, mut_arguments,
continuation }`. Callee is a Value::Closure; arguments enter its parameter slots.
`Call.writebacks` binds mutable callback parameter ordinals to fields of saved
mutable operation arguments. The scheduler publishes them after callback cleanup
on return, failure and tooling abandonment, before restoring caller storage; nested
native callbacks forward them through each suspended operation. Targets must name
mutable arguments and live fields. `iter.next` uses this for its state beneath any
`iter.map` wrappers, so cleanup releases the current state without rollback.
Mut argument ordinals are also collected from the finished callback and delivered as
`OperationResult::Complete { value, arguments, consumed, released }` to
`Operations::resume(token, result, context)`. Callback completion carries empty
ownership lists; its mut updates are collected from callback parameter storage. Continuation is
an opaque u64 token owned by the operation module; it carries no source identity.
Resume may return another Call, a final Complete result or Failed. Every
callback is a frame on the same scheduler stack: normal step/depth guards and emitted
failure cleanup apply. Callback failure is delivered to resume for disposal, then
propagates regardless of a returned success, unless its runtime continuation explicitly
implements a FAIL-2 isolation or structured join boundary.
Artificial limits propagate directly without being converted into callback failures.
The default resume rejects unimplemented continuations as InvalidTir. Tests perform
1000 resumes with maximum depth 2, check mut callback results, failure shielding,
limits and ordered handle cleanup.

Closure environments carry `owned_captures`, one boolean per capture: Copy/Take
owns the captured value; Borrow/Mut retains only a scoped snapshot. Discarding a
closure releases owned captures in reverse order and disposes borrowed snapshots
without finalizers. Lowering still enforces loan lifetimes and prohibits borrowed
environments from escaping. External closure values must supply a matching mask.

`OperationResult::Call` also supplies `owned: Vec<Value>` for operation temporaries
in creation order. Callback arguments and the callee are snapshots; `owned` is the
explicit ownership transfer to the scheduler. After the callback's own cleanup,
the scheduler releases this list high to low, with finalizers, handle hooks and
the original operation anchor, before invoking `resume`. Callback failure, a
failing temporary finalizer, and tooling-limit abandonment all release the list.
Finalizer failures join the cause chain and cannot be swallowed by `resume`.
If cleanup invalidates a returned value, that value is released too; ignored
continuations' newly owned temporaries are also released. No host destructor runs
Hirsel finalizers, and no callback or finalizer recurses on the host stack.

The hand-lowered fixture declarations retain owned owner/callee/nominal snapshots
in `tests/support/signatures.rs`. The main crate tests validate the regular core,
aggregate, pattern and callback fixtures directly. The intentionally
reachable tooling-depth sentinel and explicit malformed-TIR rejection cases have
narrow expected-violation checks; all other retained definitions are asserted
valid, including callees that never execute. The counted-slot cleanup lists are
emitted by the fixture builder, with temporary releases before named locals;
production evaluation never infers cleanup.

`OperationResult::Call.updates` publishes saved mutable argument snapshots before
scheduling the callback. Each `(parameter_ordinal, value)` must target an original
Mut operand; invalid or duplicate ordinals are API errors. An iterator moving an
item into `Call.owned` must remove that item in its corresponding saved-state
update. Successful completion, callback failure and tooling abandonment restore
from the updated state, so caller cleanup cannot finalize the transferred item a
second time. Updates on ignored continuations are applied before their newly owned
temporaries are released too. This is explicit native-operation state transport;
borrow/copy snapshots do not run user finalizers when replaced.

The unary Unique TIR operation requires Copy extraction and produces an
independent owned value graph. Checked execution removes binding and own-root
bounds on that graph while retaining region/arena placement, including placement
hidden behind borrowed parameters or captures (MODE-7). Opaque Shared handle identities remain boundaries.
Resources and once values retain the existing copy rejection.

Generic size reads use explicit per-frame bindings keyed by declaring NameSite
and SizeParamId. Type arguments need no runtime entry. Direct calls bind constant
sizes and resolve forwarded symbolic sizes in the caller before dispatch.
`Rvalue::Size` returns a u64; absent bindings are InvalidTir, never zero.
Closure values retain lexical size bindings, including native callback transport
and copying; a generic callable adds its own scoped bindings to captured outer
bindings. Resource
and resource-variant values retain their declared type-size bindings until their
finalizer executes, even after construction returns. These rows contain concrete
u64 values, not foreign TypeIds. Native cursor snapshots and stable private value
encodings preserve the bindings too.

TIR Body.parameter_scope preserves the source parameter binder for validation.
Runtime storage still uses LocalId; execution never treats a source scope or
parameter ordinal as a storage address.

TIR loan ends now explicitly distinguish strict and conditional branch-alternative
ends with `if_active`. As with BeginLoan, execution performs no dynamic borrow check;
validation proves the selection and lifetime before execution. Runtime operand
transport already relocates temporary slots without releasing their payloads.

## Checked execution

`RuntimeTypes.checked` turns on checked execution (`checked.rs`): a third witness of
the memory model beside the front end and the IR checker. Each slot carries a
shadow of lifetime bounds; scope entries, parameters, closure environments and
arenas are instances that end at `EndScope`, frame exit, self-tail restart or the end of a checked
`arena.with`, and any later operand, write or `Drop` reaching an ended instance
stops execution with `Outcome::Trapped(Trap)`. A resource count, the reverse
declaration order of live resource owners across scopes and cleanup chains is
checked too. Complete native signatures transfer result-position lifetimes,
taken values and callback returns. Borrowed callable parameters and captures retain
environment field shadows beneath a separate access view; invocation checks that
view before selecting the fields, preserving the returned content's provenance.
`@noalloc` also checks projected COW detaches,
deep unique copies and implicit destruction, using shadow ownership witnesses
preserved by takes and invalidated by copies. Traps transport as the internal `Error::Trapped` and never leave the run
entry points. With `checked: None` nothing is tracked and execution is unchanged.

Normal execution runs `arena.with` by calling its callback with an opaque
`Arena` handle: placement is never observable (MEM-10), so the arena allocates
and frees nothing here; the native and checked executions are measured against
this.

The private heap producer returns an opaque stable `$Heap` handle with identity zero. It performs no allocation. This executes the heap keyword independently of the M2 `copy_to` operation.

BindOrigin and EndOrigin are validated semantic identity bookkeeping with no
runtime value effect. ReadConstant reads the program-lifetime materialized value
served by TirProvider; it has no initializer operand. The scheduler decodes the
provider's ordered constants before entry and caches each once. Source reads use
the reference evaluator's semantic-copy representation of readonly shares. No
initializer user code executes at startup. Named FunctionReference values retain
the definition and generic owner, use direct-call dispatch, and share their
representation with constant function fields. Capturing functions remain closures.

BindFields, like BindOrigin, has no runtime value effect. It makes simultaneous
field-name introduction explicit for validation; ordinary operand extraction still
performs every destructuring copy, borrow and move.

TIR Place.loan records statically validated alias access permission. The
interpreter continues to address the explicit root/projection; BeginLoan and
EndLoan remain validation bookkeeping with no runtime value representation.

Generic indirect calls bind explicit actuals in declaration order, retaining Type
positions while installing concrete size values. Forwarded sizes resolve in the
calling frame. All closures retain their captured lexical size environment;
generic local callables add their own arguments under their body scope. Types execute parametrically, like
direct user calls, through scalar widths and aggregate runtime shapes.

Dynamic Index projections fail with MEM-14 at the access site and execute emitted
failure cleanup. Field ordinals outside the declared layout remain malformed-IR
errors. Replacement resolves its destination before taking the prepared value,
so an invalid index preserves both owners for cleanup.

Native incoming Take operands remain evaluator-owned in source evaluation order.
Call/Complete.consumed explicitly transfers declaration ordinals into callback
temporaries, the result or saved mutable updates. Duplicate and non-owned
transfers are API errors. Untransferred operands release high to low on every
completion, callback failure and tooling abandonment, before caller cleanup.
Complete.released transports displaced values in creation order. Mut updates
publish before those values release through the ordinary finalizer/handle path;
a failing release prevents the success continuation and joins the failure chain.

Semantic effects export source/role anchors for resource construction, finalizer
entry, forget, handle drop and capability consumption. Simulation replay acceptance
precedes publication; transient body and cleanup indices stay in the debug trace.
Collection mutations carry their updated owner and displaced values together in
`collection::Mutation`; publishing consumes it into Complete. Equal-key Map
replacement retains its old key and insertion position; duplicate Set insertion
returns its unused input for ordinary release. Consumed file handles explicitly
transfer out of evaluator cleanup even when their operation returns an error Result.


The primary operation macro and comparator/cursor tables export the exact paths
that they dispatch. `operation_dispatch()` includes explicit unavailable operations
and the evaluator's forget role. Registry coverage is checked in both directions
by the driver without reading Rust source arms.

Execution API errors retain prior stdout in ExecutionError. Launch carries the
original categorized cause plus output; ObservedExecution.outcome is an inner
Result, with tooling_output when no semantic observation exists. Pre-execution
launch errors stay outer errors. Ordinary run adapters write captured output
before returning the cause; unavailable execution remains exit 2.

Violated executable unchecked preconditions stop with UndefinedBehavior carrying
`undefined behaviour detected`, operation, source site and contract. Prior stdout
is retained, but DET-1, semantic parity and cleanup guarantees do not apply (#49).
No language failure is synthesized; observable returns None and CLI exits 2.
Malformed operation schemas remain API errors.

`Body.tail_sites` transports H's source-authorized decisions; V independently
checks declarations, surviving owners, borrows and return continuations. A checked
self-tail site reuses its frame, preserving lexical size bindings and its caller
continuation. Single-use counted temporary borrow arguments move into internal
owners; ordinary release replaces each old owner and releases the last on return
or language/tooling failure. `LoopOwnership` is an accounting trace, excluded from semantic observations:
its live count measures retained internal slots, not every physical allocation.
Pending resource finalization, frame borrows and exit contracts keep ordinary
calls. Constructor-hole normalization remains reserved under spec issue #46.

Map Index projects the insertion-order entry tuple; SortedMap Index projects its key-order entry array while preserving the hidden Order dictionary. Borrow/mut loops use those addresses. Consuming map cursors own the entire original container and remove each entry before returning it in Complete; opening transfers operand zero, and cleanup retains the remaining entries and comparator.

Materialized Map/Set data constructors build complete insertion-ordered entries
or elements directly. Malformed tuple entries, missing elements and mismatched
collection kinds remain API errors; static validation owns bounds and duplicate
key proofs. Construction performs no hashing or comparator callback.

Conditional Replace installs the new owner atomically, transferring any present
old value into displaced storage. Dead alternatives leave that temporary empty.
The subsequent conditional drop uses the ordinary live mask and failure shielding;
new content remains installed before an old finalizer or its failure cleanup.

Wave-1 list mutations, byte/position/path bridges and finite failure.check dispatch from exact core signatures. Read-only Bytes indexing extracts a plain u8 and checks bounds even when its result is discarded. std@1 algorithms run through ordinary source calls, never dispatch slots.

Borrowed protocol receipts are checked against retained nominal and callable
headers before lifetime transfer. Open/next compose source content bounds and
exclude their temporary operand homes; nested captured/view bounds remain real.
Ordinary local calls retain their existing argument-home policy. The single-yield
and repeated-step regressions run through the driver's checked execution seam.

`SimulationConfig.links` retains physical snapshot aliases independently of the host
adapter. The reference model expands relative components within each issued directory's
resolved root; file writes, atomic temporaries, cleanup and sorted walks share that
snapshot identity. Link escape/cycle faults remain operation outcomes.

Opened File grants retain a monotonically issued simulation identity. Physical
entries have separate current identities: ordinary/partial replacement issues a
fresh one, atomic append preserves it, publication moves it, and unlink retires
it. Narrow resolves the stored path through the target grant before comparing
identity, including the original Dir. Handle transfer preserves identity; close
retires the grant. This model is independent of heap and runtime Io.

`SimulationConfig.random_seed` initializes Random's generator;
`SimulationConfig.test_seed` supplies Test's property input. Neither reads
`RunConfiguration.hash_seed`, which affects placement only. All default to zero.

`SimulationConfig.mounts` supplies named virtual selectors with `hirsel_run_config::Access`
rights. File snapshots use those virtual names, including root aliases for shared
physical trees. A missing selector returns FileError::NotGranted independently
of the Dir projection; narrowing retains access rights and all mutation paths
check them before consuming faults or changing storage. Test-created trees retain
explicit read_write authority and independent file generations.

CONC-8 quiescence has `Outcome::Blocked`, separate from Failure and tooling limits.
The scheduler accepts the internal Blocked stop signal without executing pending
cleanup or checked resource-exit audits. Its observation retains stdout, confirmed
effects and stable wait rows (spawn path, source anchor, kind, logical targets).

## Structured concurrency implementation

The reference runtime uses cooperative fibers: each task owns a frame stack and
failure chain. A FIFO runnable queue advances only at Io calls, CAP-4 operations
and explicit cancellation checks. Pure code does not yield. Destruction suppresses
cancellation but can yield at permitted Io operations.
Spawn transfers the once closure into a new task; its captured value tree has no
shared counted storage in this value-based interpreter. Scope continuations join
all children, including detached children, before releasing the scope body.
Cancellation is persistent task state and is observed before a blocking effect,
never by pure code, joins or mutex acquisition. Synchronizing operations record
ordered capability events through the existing strict replay input. The default
choice is FIFO; task.resume inputs may select any runnable task and reject blocked
tasks. Completion is recorded after cleanup, separately from await completion.
Blocking simulated network operations retain their typed arguments and mutable
loans while another task runs.

Channels own FIFO payload values, endpoint counts and suspended receives. A send
transfers its argument only on success; a closed or canceled send returns it.
Closing the final receiver schedules buffered payload destruction through evaluator
work items. Mutex callbacks run on the acquiring stack, without a scheduling point
inside the body. Mutable callback results replace the payload on success; failure
kills the mutex and releases its payload through the same worklist. Atomics store
scalar values and use the same total operation order.

An isolate continuation alone may catch the callback's language failure after all
cleanup; ordinary callbacks preserve failure propagation. Child completion retains
its failure chain and spawn path. Scope joins select failures in spawn order and
cancel siblings and the enclosing scope body at its next blocking operation.
Tooling termination and global quiescence bypass isolation.

Scope exit distinguishes the declared Result nominal from other variants: an
Option::Some result joins normally. For a generic erased result R, values retain
only their variant ordinal and fields, without nominal identity. A second variant
at that boundary reports Error::Unavailable rather than guessing whether it is a
Result::Error. Supporting that case requires carrying the instantiated result
nominal through generic calls or into value metadata.

The native implementation will require separate task contexts, each owning its
allocator, plain counts and failure state. Cooperative stackful fibers avoid
concurrent access to those contexts. Runtime ABI entries must transfer unique
allocation graphs (including slab ownership) between contexts at spawn, send,
receive and join; copying a plain counted pointer across contexts is invalid.
Mutex payloads need a separately owned context entered only for their synchronous
callback. Native cleanup must finish before publishing task completion, and an
isolate must save/restore the acquiring context's failure chain. Until those ABI
and lowering changes exist, native concurrency remains explicitly unavailable.

`Operations::callback_finished` receives mutable callback snapshots on return,
failure and tooling abandonment before the continuation resumes. Runtime-managed
owners such as mutexes use it to retain the final payload without rollback;
abandoned payloads return to the evaluator's explicit cleanup worklist.

Production host operations are supplied by the driver's Operations adapter; this crate
remains host-independent. Scripted `net.listen` checks the exact grant before reporting
that scripted incoming peers are unavailable, so an empty rule set returns NotGranted
without a provider effect.
