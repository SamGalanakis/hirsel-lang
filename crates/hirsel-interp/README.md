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
`ExecutionError`, retaining the original `Error` and stdout bytes. Values are
canonical by construction (SEM-2): entry inputs, TIR constants, scalar results and
the library's float producers (`from_bits`, codec decoding) canonicalize NaNs, and
nothing walks a value to re-canonicalize it; an `Operations` implementation returns
canonical floats. Equality of executions includes float bits and the selected trace. Semantic observations are identical across trace modes.

`exit_report` formats the typed payload of a returned Error using a worklist. The scheduler consumes the returned entry Result after its entry hook publishes the report; the observation retains only a tooling snapshot of the value. The formatter performs no Io; `ObservedExecution.exit_report` carries the host text beside the semantic observation, and the driver decides when to write it.

## Core invariants

- Bodies, instructions, operands, constructor fields, call arguments, and cleanup
  actions execute in stored order. Field/parameter ordinals determine layout,
  independently of evaluation order. Every TypeId is a row of the session TypeInterner; no
  type graph is copied or imported at run time.
- A slot contains `Option<Value>`. Take clears it; Copy/Borrow/Mut read it; Assign
  requires dead storage. Replace atomically installs a prepared value and puts old content in its declared displaced temporary; assignment follows it with explicit Drop; core replace returns the displaced owner. Borrow checking and loan validity are lowering's job;
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
  normalization, aggregates, resources and closures. Values name definitions by
  their interned `DefId` and box closure environments and function references, so
  a Value is 56 bytes; observations and effects carry the `DefPath`.
- `scalar.rs`: TIR literal, unary and binary operations, with typed scalar errors.
  Ownership unary operators and eager logical operators are invalid TIR here.
- `conversion.rs`: representation adapters for the shared pure exact/representability and low-bit integer conversion kernels
  across all widths and signedness. These choose no library spellings or allowed
  widening signatures. M can wrap `exact` as widen/Option/checked failure and use
  `truncate` once the signatures in #18 are pinned.
- `frame.rs`: slots, operand extraction, projections, assignment and simple drop.
  L can extend projection and drop behavior here without changing CFG scheduling.
- `code.rs`: each executed body lowered once per run into dispatch form (a flat op
  array with block offsets, whole-local operands resolved to slots, scalar constants
  decoded for their destination, scope ends and whole-local cleanup lists expanded
  to slots) and each direct call site's resolved target (CT-5 instance, closed
  generic arguments, entry body). The scheduler steps the top frame through these
  ops while each completes without failure, a call or queued work; any other op
  executes from the TIR as before, so steps, sites, failures and traces are
  unchanged. The forms live in the run's scheduler, beside its frames.
- `eval.rs`: the small scheduler, rvalue/instruction/terminator dispatch and ordered
  cleanup worklist. L adds its rvalues and cleanup handlers at these matches. Its
  finalizers must also schedule frames/work items, continue field release after a
  failed finalizer, and preserve the original failure; do not recurse per call.
- `parked.rs`: the run's parked tasks in poll order (waits without a deadline, then
  timers by deadline, ties in suspension order). A layer marks a wait `watched` when
  its poll returns None without effect until the layer's `woken` names it; a poll
  then asks the woken and unwatched waits only, as the native scheduler re-issues
  only ready requests. The host library watches its host waits, except those parked
  by a task already cancelled, and reports every wait after a cancellation.
- `run_with_operations`/`Operations`: M implements registry-backed named calls in
  a separate module. Calls receive values in parameter order, the
  session TypeInterner/result TypeId, generic arguments and source site. Context holds only
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
`copy_to` deep-copies semantic values; checked result bounds follow its allocator
parameter. Sealed-region cleanup remains unavailable. Unavailable features are
reported explicitly, never silently approximated.

## Explicit limit and exit policies

`Limits` defaults to `Some(1_000_000)` steps and 256 calls.
Application fixtures that exceed this default declare a finite bound of at least
1,000,000 as the last argument to `hirsel_engine::run_observed_snapshot_bounded`
or `run_observed_checked_bounded`. Each result's `step_usage` records both
`limit` and `consumed`; print or retain it with the test receipt. Require a
completed observation and expected outcome: exhaustion must fail the test.
These helpers restore the prior policy after every result, including errors.
Do not increase CPU or memory caps when choosing a fixture's step bound.

For example, a test may pass `2_000_000`, assert that `step_usage.limit` is
`Some(2_000_000)`, and check the complete output against its oracle. The default
and corpus policy remain unchanged. Lower-level evaluator tests can still use
smaller bounds to test exact stop behavior.

At the lower-level evaluator API, `steps: None`
removes only the step bound; call depth is still checked. Step accounting saturates
at u64::MAX for unbounded runs. One instruction or terminator
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

Entry return mapping preserves unit Ok (0) and Error (1); a returned std process ExitCode or Ok(ExitCode) preserves its unsigned byte value.
Other return values are core fixtures and exit 0. v20 PROG-3 fixes exit 0/1; M supplies the nominal Result registry mapping.
The driver must supply correctly lowered root capability arguments; the core does
not grant capabilities on its own.

SEM-1's unsettled shift details use bit-vector left shift, sign-extending signed
right shift, invalid negative counts failing SEM-1, and representable MIN % -1
returning zero. These choices are recorded in
[#32](https://github.com/SamGalanakis/hirsel-lang/issues/32) and comments at the
implementation. Signed constants carry width-sized two's-complement bits.

## Validation

`//:hirsel-interp-unittest` and `//:hirsel-interp-test-it` run scalar boundary tests, ordered
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

`Value::Sequence { kind, elements, arena }` represents Array/List/Inline/Simd. `Value::Map`
contains optional `(key,value)` tuple entries in insertion order; `Value::Set`
contains optional elements in insertion order; each has an `arena` tag as a Sequence does. M owns their named operations;
structural map equality is order-sensitive, set equality and hashing ignore order.
Opaque `Handle { kind, id }` is for capability/runtime identity, never host addresses.
The registry chooses tags and deterministic IDs; no behavior is inferred from tags.

Semantic copying uses an iterative tree copy that stops at blocks: the elements of a
List, Array, Inline, Simd, Map or Set (`value::Block`) are shared by every copy and
detached by a holder's first write (copy on write), so moving, borrowing and copying
a block costs O(1). Sharing is this model's cost, never a semantic fact: the native
RC pass, holder counts, COW allocation audits and reuse are separate oracles, and
checked execution's witnesses ignore it. Reads of an operand (copy, borrow) resolve
its place without writing, so they never detach. `work::Counter::InterpCopiedValues`
counts the nodes copied beneath a copied value and the elements a detach copies; the
driver's scaling probe bounds it. A run asks its `TirProvider` for each definition
once, and for each own-bodied CT-5 declaration once; a failed lookup is asked again.
Mut arguments transfer into frame slots and write
back on success or failure, without rollback. An operation may write its `mut`
arguments in place through `OperationCall.arguments`; library `Complete.arguments`
updates are validated before publication. Projection and whole-value
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
mutable arguments and live fields. `iter.next` uses this for its `iter.from` state,
so cleanup releases the current state without rollback.
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
and SizeParamId. Concrete type arguments travel as declaration-owned graphs for
type-directed operations. Direct calls bind constant sizes and resolve forwarded
symbolic sizes in the caller before dispatch.
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
checked too. A place's shadow follows the same steps as its value: an element place
reads that element's own subtree with its container's bound added, never the whole
container's shadow, so a shadow stays proportional to the value it describes.
Complete native signatures transfer result-position lifetimes,
taken values and callback returns. Borrowed callable parameters and captures retain
environment field shadows beneath a separate access view; invocation checks that
view before selecting the fields, preserving the returned content's provenance.
`@noalloc` also checks projected COW detaches,
deep unique copies and implicit destruction, using shadow ownership witnesses
preserved by takes and invalidated by copies. A witness also counts the block's
holders (`checked::Holder`): a copy is a new holder that lives while some shadow
keeps it (a copy of a `flat type` value owns fresh blocks, MEM-12), and a native `mut`
write-back keeps the holders its operation took. Wherever the IR claims a value unique
it must have no other live holder, or execution traps (`UniqueShared`, MODE-2): a whole
local declared `unique` when assigned, a parameter declared `unique` at frame entry, and
a native operation's `unique` parameter, such as `shared.freeze`'s (MEM-11). This is
independent of both checkers' uniqueness proofs. Tasks never share counted blocks
(OWN-6): every operand use claims its tracked blocks for the running task, a task
capture taken by a new task hands its blocks over, and a use of a block another task
owns traps (`TaskShared`). Values crossing through a channel, a task result, a mutex
or a `Shared` root reach the other task through a native operation with fresh shadows. Traps transport as the internal `Error::Trapped` and never leave the run
entry points. With `checked: None` nothing is tracked and execution is unchanged.

Normal execution runs `arena.with` by calling its callback with an opaque
`Arena` handle: placement is never observable (MEM-10), so the arena allocates
and frees nothing here; the native and checked executions are measured against
this.

`arena.bounded(cap, body)` (MEM-6, `src/bounded.rs`) calls its callback the same way
with a fresh bounded arena handle (a checked arena instance under checked execution), whose
budget is `cap`. A box, sequence, map, set or closure construction into that arena, and a
`copy_to` into it, takes its logical charge (`hirsel_type_analysis::logical`, the table in the
runtime README) before any operand is consumed; a charge past the budget records the failure
`OutOfSpace` (MEM-6) at that site. When the callback frame exits, a return is `Ok`, and a
failure whose first record is that arena's `OutOfSpace` with no later record (no failure
during its cleanup) is removed and becomes `Error(OutOfSpace)`; every other failure
propagates unchanged. A charge of a generic type, which parametric execution cannot size,
is `Error::Unavailable`, as is a copy of a callable into a bounded arena.
A List, Array, Map or Set block in an arena carries `ArenaStorage` (its arena, one
element's or entry's charge and a shared flag): `list.push` and an insert of a new key
charge one element or entry, and the first write to a shared one charges its length or
entry count (`eval.rs` `list_write`, `keyed_write`). A List of `Packed[T]` carries one too,
charged `T`'s logical size per element.

The private heap producer returns an opaque stable `$Heap` handle with identity zero. It performs no allocation. This executes the heap keyword independently of the M2 `copy_to` operation.

BindOrigin and EndOrigin are validated semantic identity bookkeeping with no
runtime value effect. ReadConstant reads the program-lifetime materialized value
served by TirProvider; it has no initializer operand. A call of a CT-5
generic generated declaration runs the definition `TirProvider::instance_body`
names for its closed actuals. The scheduler decodes the
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
Collection mutations write their `mut` owner in place; `collection::Displaced`
carries the values they displaced into Complete.released. Equal-key Map
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
Ordinary calls add a borrowed argument's home to its lifetime, except a borrowed
`Local` whose bound names only its caller's instances (mapped through parameters and
captures): relations compose through such a view (MODE-1). The single-yield
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

`SimulationConfig.mounts` supplies named virtual selectors with `hirsel_run_schema::Access`
rights. File snapshots use those virtual names, including root aliases for shared
physical trees. A missing selector returns FileError::NotGranted independently
of the Dir projection; narrowing retains access rights and all mutation paths
check them before consuming faults or changing storage. Test-created trees retain
explicit read_write authority and independent file generations.

CONC-8 quiescence has `Outcome::Blocked`, separate from Failure and tooling limits.
The scheduler accepts the internal Blocked stop signal without executing pending
cleanup or checked resource-exit audits. Its observation retains stdout, confirmed
effects and stable wait rows (spawn path, source anchor, kind, logical targets).

Run budgets (`RuntimeTypes.budgets`, `budgets.limits@2`; `budget.rs`) stop a run the
same way, as `Outcome::BudgetExceeded(kind)`. Fuel is charged at each block start a frame
reaches by entry, self-tail restart or a jump to the same or an earlier block. Memory is
`Value::logical_bytes` over every frame of every task plus `Operations::retained_bytes`,
measured before the next step once constructions' and library results' shallow charges
pass the headroom left at the last measurement, or once enough fuel has passed; the
decision uses only the exact measurement, so frees never cause a stop and replay stops
alike.

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
transfers its argument only on success; a closed send returns it. Send never checks
cancellation, while receive still does.
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

Free mode at width 1 (lane F0, `library/free.rs`): under F0's test channel
`HIRSEL_RT_FREE_TEST` (`hirsel_free::session::TestHook`), which the driver installs, the
library drives a `hirsel_free` session under a seeded random schedule. Every synchronising
operation is an op that yields (channel open, dup, send, receive and an endpoint's close, the
take and put around a `mutex.with` body, atomic loads and stores, `shared.thaw`, the task and
set operations, cancellation checks, `out.print`/`err.print` as host writes on the
`stdout`/`stderr` keys, and each task's end); a waiting op publishes (k, entered) when it parks
and (k, done) when a poll completes it. A `Shared` release that does not find its root's count
at 1 is deferred to the releasing task's next op (`Operations::defer_release`); a walk that
ends an entered phase releases its nested roots at once (Amendment F0-1). In a replay a phase
the cut does not admit parks its task for good, one waiting for a predecessor parks until a
publish, a drop's endpoint close whose version is not reached applies later in program order,
and the root drains the other tasks at its end (`Operations::root_finished`).

`Operations::callback_finished` receives mutable callback snapshots on return,
failure and tooling abandonment before the continuation resumes. Runtime-managed
owners such as mutexes use it to retain the final payload without rollback;
abandoned payloads return to the evaluator's explicit cleanup worklist.

## Runs of one execution

`runs::execute` drives the root run and every callee run its runner calls start
(docs/spec/runners.md). Each run has its own `Scheduler` (frames, tasks, failures,
outcome) and its own Operations; `execute_frames` runs one until none of its tasks can
proceed and returns `Step::Idle` where it would wait. Ready runs take turns in the
order they became ready. With nothing ready, each run's provider `wait`s in run order,
and only when none progresses is the execution Blocked, with every run's waits (CONC-8
across the execution, RUN-12). At a run's idle point the driver takes its new calls
(`take_calls`: a `CalleeStart` with the callee's configuration, decoded arguments,
operations, budgets and `sim@1` crash point) and its forwarded cancellations
(`take_cancels`): a callee not yet started ends `Canceled` at once; a started one has
its root scope cancelled (`cancel_run`) and its caller keeps waiting for its outcome
(RUN-9). A callee's end reaches its caller through `call_ended` as a `CalleeEnd`. A
callee's failure, Blocked state inside a run that waits, or budget refusal ends only
it; its tooling limit, undefined behavior, trap or environmental stop ends the
execution, as a process crash would. A callee in this process writes its stdout after
the root's.

Production host operations are supplied by the driver's Operations adapter; this crate
remains host-independent. Scripted `net.listen` checks the exact grant before any
provider effect, so an empty rule set returns NotGranted. A handle drop that raises a
language failure (a scripted client's unmet Expect) reports it through
`take_drop_failure`; the evaluator records it at the drop's anchor.

`library::process` is the reference Process provider. `SimulationConfig.process`
holds the `io.process` grant; the `process` projection mints the root Proc with its
rules. `proc.run` returns Canceled in a cancelled scope, then checks its `Command`
(`hirsel_run_schema::Command::check`: a canonical absolute program, NUL-free arguments
and a well-formed exact environment, InvalidInput; the exact rule and its environment
names, NotGranted); a deadline virtual time has reached is TimedOut, and otherwise it
serves the first unused `process.script@1` or `test.process` entry with equal program
and args (none: NotFound), bounding each stream by `max_output` (OutputLimit).
`proc.pipeline` checks its stage list (`hirsel_run_schema::stages_error`: empty, or
stages whose `own_group` differ, InvalidInput), then every stage, then takes one entry per
stage, all or none, with
`hirsel_run_schema::script_pipeline`; `proc.run` is its one-stage case. It never spawns: the driver's host library sends `process.spawn@1` runs to the runtime.
Replay takes the recorded completion and re-derives the checks; a contradiction
refuses. `proc.spawn` makes the same checks and plays the first unused scripted child
entry with `hirsel_run_schema::ScriptChild`, minting a `Child` grant of its Proc. A read
at an `Expect` parks its task as a scripted peer's read does (`waiting_call`), and a
contradicted or unmet `Expect` (at a write, `close_stdin`, `wait` or the Child's drop)
fails LANG-9 after its Test report. `proc.spawn_pipeline` plays its stages' entries with
`hirsel_run_schema::ScriptGroup` behind a `ChildGroup` grant, which the `group.*`
operations and `proc.wait_group` take. Host children, like host runs, go through the
driver's host library to the runtime.

SimulationConfig supplies stdin bytes and exact Env name-to-optional-value snapshots.
Library dispatch owns stdin cursor advancement, append-only mut byte buffers,
UTF-8 argument text, and distinct stderr capture in ObservedSimulation. Each
operation records its typed input and completion through the ordinary replay
check. `supply_stdin` lets a host adapter provide one already-read chunk without
changing authority or replay state; it then uses the same operation implementation.

`OperationsAdapter` forwards every hook from the Operations method inventory; operation
layers override only intercepted hooks. `Operations::wait` is called only after all
runnable work is exhausted. The simulation returns false (CONC-8); a host adapter may
wait for external readiness and ask the scheduler to poll again. Parked operations are
polled only at suspension points, so pure code does not issue readiness syscalls. `Operations::wait_deadline`
exposes a timer's absolute ticks to the scheduler: timer polls use deadline order,
with equal deadlines in suspension order, in both virtual time and host adapters.
`Operations::expire_deadlines` runs at every poll before the parked tasks: the simulation
cancels each `task.scope_until` scope whose deadline its virtual reading reached (deadline
order, ties in entry order), recording `task.scope_until.expire`, and pending scope
deadlines count as the earliest timer when no task is runnable. An expired scope cancels
its children `DeadlineExceeded` and the children of the scopes its owner entered inside it
`Parent`, and its exit wraps the body's result in `Expired` or `Finished`. A host adapter
calls `Library::host_deadlines` and arms the entered scopes (`take_deadlines`) on its own
clock, firing them with `expire_deadline`.

Detached task results are released on the child's evaluator stack before completion;
an already-completed result is released by detach's ordinary Complete.released work.
Completed detached children leave neither task rows nor scope child entries. Per-task
spawn counters and empty active-scope stacks are retired at completion too. Scopes keep
detached failure chains separately, sorted by spawn path, until their ordered join.

Concrete generic type arguments travel with interpreter frames and captured closures, alongside lexical sizes. Named callable values close their actuals before escaping a frame. Type-directed codec operations use these declared type snapshots; ordinary parametric evaluation still uses the same TIR bodies. A decode whose schema reaches checked opaque types (SER-1) reconstructs the whole value, then calls each check as a library callback, inner occurrences first: the first `false` returns `Error(Invalid)`, and a failing check is `Error::Ended`, which the run entry reports as `Outcome::Failed` without unwinding (FAIL-3).

The scheduler reports CONC-8 Blocked after polling every parked operation when
no task is runnable and `Operations::wait` finds no external wakeup source. This
includes channel and task-join waits, with no user destruction or cancellation.
Observed waits sort by spawn path, stable source site, kind and logical targets.
Scripted stdin exhaustion is enabled EOF; live host stdin and sockets retain
readiness interests and prevent a premature Blocked conclusion. CONC-5 mutex
bodies are synchronous and hold no authority, so they cannot suspend in this
cooperative evaluator.

Checked Replace transports displaced caller-owned content with its mutable
parameter's original lifetime bounds, as mutable writeback does. Its callee
access instances are expanded transitively through mutable wrappers before the
owner moves; ordinary binding/capture roots and real region/arena bounds remain.
Mutable parameter roots retain their original incoming bounds separately from
ordinary roots, so this transport cannot erase a genuine borrowed lifetime.

Failure observations retain and compare the original operation site independently
of the primary attribution. Tooling reports render both complete locations.

Caller attribution travels as an immutable definition handle, body and source span,
with no per-call allocation to discover a site. Marked named bodies inherit their
caller's site through direct calls and generated contract invocations. Ordinary
function-value invocations, callbacks and spawned tasks establish their actual
invocation as the boundary. A marked entry with no language caller retains no
inherited site and reports its own operation. Primitive continuations and release
work carry the selected policy explicitly, so temporary cleanup cannot recover a
surrounding marked frame's attribution after a function-value boundary. Failure
records retain the actual operation separately; existing anchored child failures
and ordered cleanup causes keep their original sites.

SortedMap keeps its arena storage tag on its entries sequence across comparator
continuations. Once comparison succeeds, an `OperationResult::ArenaCharge`
checks entry growth and any shared copy before completion transfers incoming
owners. Comparator failure charges nothing; deep copy retags entries in the
destination arena.

Blocking Io calls apply CONC-3 program validity and authority checks before cancellation;
scripted peer transitions, faults and deadline observations follow cancellation.

Directory operations normalize the caller's portable path lexically through
`hirsel_run_schema::confined_relative` before resolving snapshot symlinks.
Physical link targets are still traversed component by component beneath the
pinned directory.

Secondary file and child handles are validated before authority, argument
checks, cancellation or injected provider faults.

`bytes.append` publishes a byte-list update through the same operation completion
boundary as list mutations. Its bounded-arena charge covers the input length and
any shared destination copy before the update, matching native bulk reads.

Operation targets cache `Operations::resolve` once per direct closed call site,
including `None` (execute TIR). Built-in libraries dispatch provider-local ids
through a handler table; anchors retain `DefId` until source projection.
Call-only providers retain the legacy path-dispatch fallback. An
`OperationsAdapter` that intercepts calls overrides `call_resolved`; its resolver
forwards to the underlying provider unless it also introduces new operation paths.
Interpreter-only body caches resolve local operands and place offsets. Frames
reuse projection workspace, retaining dynamic indices and variant checks at access.

`Operations::immediate` is an opt-in promise that a primitive produces no task
rests and needs no completion hook. A fully transferred Complete result with
no updates or releases can then restore its mut inputs and publish immediately.
Untransferred non-inert owners, callbacks, Free mode, checked execution and
memory-metered execution keep the ordinary scheduled completion path.

`ownership_trace::run_owned` explicitly observes Resources and owned String/Bytes
leaves through construction, moves, calls and cleanup, on reference and checked
execution. Copies and unsupported library producers refuse the diagnostic while
preserving the source outcome. `run_resources` keeps its resource-only selection.
