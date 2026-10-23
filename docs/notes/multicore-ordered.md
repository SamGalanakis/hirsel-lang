# Ordered retirement protocol

The protocol in `hirsel-rt/src/ordered` implements design section 4.3's
logical frontier independently of fiber dispatch. `Frontier` accepts owned
operations and exact R4 stretches, proves the four early-commit rules, and
absorbs each head's log in program order. Retirement alone records outcomes,
publishes pending sends, polls waits, and advances the logical queue.
Calls insert holding children's rests; a child's end returns to its caller
without a poll. Logged `detach_all` calls members in order before yielding.
No queued turn receives a fixed numeric retirement slot.

`Held` metadata is conservative: unknown capture or type metadata blocks early
commit. `Carries` follows recursive handle types, holding descendants, and
callers' remaining holding members. Receiver leftover finalizers and TaskSetTake
must register their obtainable handle types. Parked receiver polls are filtered
reads, so they do not conflict with invisible pending sends.

The native send adapter commits into the actual channel ring. An early send
creates an opaque receipt, enqueues a pending entry, and does not touch its
channel. Consuming the receipt at retirement clears pending and touches the
channel before polling. Both readiness and receive see only the visible FIFO
prefix. A terminal prefix may discard a receipt and leave its suffix hidden.

`tasks::role` supplies semantic-entry guards and explicit segment/suspension
handoff. Every descriptor-classified runtime entry acquires a guard, except
four unconditional process terminators. A source-graph test checks direct
coverage. Guards contain task context pointers, never thread lock guards.
Task identity and stack-save slots live in TaskCtx, so task-table growth cannot
invalidate a save address. Runtime calls into compiled task, root and finalizer
code release the role and reset nested entry depth until the call returns.
The diagnostic region-owner map has its own mutex.

Mutable run fields use `RoleCell<T>`, which contains an `UnsafeCell<T>`.
Runtime entries project through
`RoleCell::get` while holding the role, without borrowing the whole run
exclusively. Such field borrows must end before a role handoff. The remaining
`TaskCtx::run` callers are restricted to admission and quiescent teardown.
`Execs` is shared during execution: topology and thread handles are published under the compute lock,
the idle stack has its own lock, and shutdown drops
those locks before joining. The Io binding pointer uses release/acquire
publication. These projections remove whole-run aliasing; they do not replace
the role or prove individual concurrent service implementations safe.

Idle callbacks use a run-scoped `role::IdleGuard`; its nonreentrant acquisition
excludes other idle callbacks even though they share an owner marker. Dropping
it reads no task context, so it can follow task-row retirement. Callbacks must
raw-project role-owned fields and retain the run; they cannot enter through a
task context, switch fibers or call a home bridge while holding this guard.
Remote cancellation and readiness read a task-row shield snapshot. The owner
publishes it at role handoff and finalizer shield changes, including width 1.
Parked receive slots and both join frames are inaccessible until physical idle
save; the logical policy's after-save hook must retry those deferred polls.
Ordered task completion and calls now use the shared physical-save publication
order; host and remaining boundary interactions still need integration.

The scan-format constants live in the data-only `hirsel-core-contract` crate;
`hirsel-kernels::handle_scan` retains its public compatibility re-export. LIR
therefore has no dependency on executable kernels.

Native descriptor version 14 (introduced in runtime ABI 34) adds an independent holder scan
pointer and word count, taking the C descriptor from 72 to 88 bytes. Both
native emitters lower the existing `LayoutRecord.scan`; destruction programs
cannot substitute for it because atomics and tasks are not resources.
`ordered::capture` visits active variants, live captures, counted graphs,
inline arrays, sequences and structure-of-arrays columns. Shared values are
inert except for a mutex's exact-root wrapper. Missing descriptors, borrowed
pointers, variable-flat sequences and scans exceeding the work bound remain
unknown. Zero scan words certify an empty holder set; `u64::MAX` means unknown.
Runtime ABI 35, LIR 44 and layout version 7 retain the descriptor size and add
family, instantiated handle type and obtainable payload type to handle scans.
Separate static edges visit all variants and recursive payloads without reading
a value. Typed captures distinguish overlapping atomic and task words; missing
or opaque type metadata cannot certify a complete closure. Shared payloads
remain inert during value scans. Static closure may conservatively include them.
Ambiguous raw words contribute every registered family. Creation entries that
erase their generic type register objects under every linked type of that family;
this admits extra conflicts rather than excluding a possible object. The
test-only native admission and committed-spawn adapters install this metadata
and registry; production admission remains gated.

`ordered::objects::Registry` supplies stable identities and obtainable-type
bindings. A raw word shared by multiple native handle families contributes all
their possible footprints; an unclassified binding keeps the result unknown.
The typed operation boundary resolves its own family exactly. A future object
registered after a capture still participates in that capture's type closure.
`tasks::ordered` connects the frontier to native atomic load/store, channel
send, and mutex take/put effects using owned ABI images. Unavailable mutexes
cannot commit early. Its record and poll callbacks run only at retirement;
an early send's receipt publishes and touches the channel before that poll.
Synchronizers and channels split row reservation from initialization. The
noncloneable reservation belongs to its logged creation command. A provisional
cell/endpoint cannot serve a dependent operation before commit, and reserving a
sender duplicate adds no live sender count. Existing cooperative creation uses
the same reserve/init path. Native adapters cover atomic/mutex creation, channel
open, sender duplication and mutex cause; cause chains are owned outcomes for
the operation's task, not mutations of a different committing task's context.
Mutex wrappers still belong to their owner's allocation/budget path.
All ten E operations have native entry branches behind the ordered selector.
The driver retains payload descriptors and None images alongside provisional
handles, so later same-owner commands can be logged before creation commits.
Duplicated senders retain the source's logical object binding. Native endpoint/TaskSet and mutex closes, exact root decrements and thaw tests
now have strict-head adapters. A close first detaches owned images in a bounded
callback, then scans and finalizes those images outside Frontier. Finalizers may
yield and close nested handles without retaining a policy or object borrow.
MutexTake also scans its output before same-turn operations can use those
handles. Mutex-root decrements use the same head gate as Shared roots, before
any last-wrapper finalizer. Root increments remain immediate. These strict
boundaries do not yet implement positional speculative root logs or credit
settlement. Host and remaining boundaries still need native adapters.
Scope entry reserves a provisional identity and logs an owned command; only its
retirement installs the scope and pushes it on the originating task's scope stack,
even when a different worker/task commits the command. Strict-head adapters cover committed spawn,
holding result publication, join, detach, nonempty scope exit, receive and
physical completion.
Spawn scans the unstarted environment with the linked capture program. Task
result types remain erased by the native spawn ABI, so captured Task handles
are conservatively unknown, including the spawning parent's obtainable footprint.
Receive outputs are scanned into their owner's next-turn held set. Native task sets, add/has/take/await-any, member detach/cancel
and cancel checks now have adapters too. Task-set creation reserves an unpublished
row, returns its provisional handle and logs initialization. Insertion and close
also log owned words; the set's membership and reusable row change only at their
respective retirements. Closing a set leaves its members as live scope children.
Cancellation and sender close use owned
D commands: later tasks may log them, but older polls see neither effect.
Sender close keeps the turn and blocks a conflicting early channel operation;
an independent E operation can still pass it. Provisional sender duplicates use
the retained carrier binding before their initialization retires.
TaskSetTake refreshes the
same-turn obtainable closure, conservatively unknown for erased Task results.
Await-any chooses by completion rank, independent of member add order.
Detach-all calls holding rests in add order and yields once after its members;
cancel-all cancels each member before detaching it, then drains the saved
completions. Waiting join/receive/await-any/drain preserve cooperative
block(yielded=true): first yield, then park when the caller reaches the front
again, with both continuations retaining the original op ordinal. A join calls a saved holding child's rest; its
completion returns to the caller without polling, and an already-holding join
yields after that return. A waiting join's call/cleanup continuation consumes no
new operation ordinal. The actual task effects execute inside Frontier's commit
or continuation callback, after settlement, rather than preceding a marker op.
Nonempty scope exit calls unjoined holding children in reverse spawn order.
Detach of a holding child calls its rest; a still-running detached child retains
its result for its own cleanup. Successful detached rows retire only in idle,
after the completed image has been invalidated and its retirement poll/return
barrier has finished. Receive polls access a saved destination; only a retired
send can wake it. Runtime-only CONC-8 additionally requires no frontier head,
queue, pending poll/return, running image or armed host request.

Native object retirement can return an owned `Progress::Poll` continuation.
The caller restores policy metadata before accessing host readiness; wake merge
then precedes the yielding task's requeue and next-head selection. There is no
head during that interval. New commands are retained by their caller, and an
early send publishes and touches before the deferred poll is requested.
`tasks::ordered::driver` tracks selection separately from logical turns and
physical saved-frame ownership. It suppresses duplicate notifications and
requires both a physical save and head promotion for full logs or allocation
pauses. Scheduler.logical has an Ordered variant whose idle callback updates
this metadata under the generic saved-flag publication guard. Lifecycle adapters
can defer a retirement until its owner's physical save. The idle callback then
preflights every woken runtime wait before performing any slot effect, polls in
park order, merges wakes, and selects the next head. An unsupported service wait
leaves the whole retirement pending for a future host continuation. Free and
Ordered share the existing bounded runtime wait implementation; Free retains its
callback and notification order. The strict native lifecycle adapters use this boundary. A separate return
barrier keeps a caller suspended until physical completion invalidates the
child's image, publishes Done and recycles its stack. Only then can the caller
vacate the child's native row. A real eight-worker
control cycles the root through 256 saved-stack resumptions without duplicate
notifications. Every E entry now has an ordered submission branch. A
command owns its image, an unaccepted full-log command remains on its owner's
stack, and recorded outcomes are delivered to the owner's continuation by
ordinal. Bounded object commit temporarily removes the frontier from its driver
slot, restoring it before host polling or suspension; during a home poll other
boundaries wait. The test-only admission path accepts supported E and strict task boundaries,
dropless closure environments and installed resource finalizers, no budgets or
host effects, and no deadlines. Endpoint cleanup is still explicit in the fixtures. Native controls at widths 2/4/8 cover both running and already
holding joins, yielding rests, scope exit, detach, receive, runtime Blocked,
physical completion and repeated row/stack reuse. Drop controls include FIFO
receiver leftovers, nested sender closes, last-mutex-root finalizers that yield,
exact overflowed root counts, and head-only thaw tests.
The remaining native boundaries, budget hooks and production admission still
need installation. These controls are not a parallel ordered
corpus or speed gate.

Native ordered fuel now uses task-owned grants and real stretches. Switch-in
claims physical dispatch even for a task that reaches only fuel points; switch-out
captures consumed fuel before clearing the compiled word. It never leases fuel
from the shared Free pool. Operation boundaries attach consumed points to their
owned logs, retained full-log commands keep their original stretches, and head
promotion settles the preceding segment before granting further fuel. Refills
and physical pauses add no semantic operation or readiness poll. A younger task
whose own logged and segment fuel exceed the settled remainder saves its frame
until promotion, so an older turn's effects retire before the fuel stop.
Switch accounting raw-projects policy under the role without observing stops;
the compiled refill entry drops its temporary guard before reporting its result.
This also permits external-stop abandonment without recursively entering the
stop path. Tests cover exact exhaustion at widths 2/4/8, partial chunks, multiple
unbounded refills, external stops, and younger exhaustion with and without fuel
in earlier logged turns. Memory preflight, positional credit events and the
combined memory/fuel native gate remain unfinished. Allocation allowance pauses
must precede any worker-heap or region-pool borrow.

Finite ordered speculation allowances now preflight heap creation, sequence
growth/detach, the bare-system reuse conversion, and regional segment growth.
The charge calculation mirrors the chosen heap class or canonical region size.
A denied reservation leaves the request untouched and saves until head promotion;
the head does not wait on the allowance. Preflight runs before worker heap/pool
borrows, and region growth reborrows its task-owned record after migration.
Physical reservations are currently conservative until their retiring turn is
absorbed; ordinary free credits do not replenish the allowance yet. Memory
budget admission and exact charge/credit settlement remain gated.

## Integration status

The normal launcher now prepares an Ordered admission plan from the validated
`tasks.cooperative@1` provider, FIFO schedule, placement knobs and bound memory
budget. A separate production qualification constant remains **false**. The
plan therefore installs no scheduler, role or physical dispatcher, including
when placement requests eight cores or dedicated commit. Substrate worker
tests keep their explicit admission. Free and seeded non-FIFO schedules do not
select this plan. Once E1 is qualified, the prepared plan will select the same
saved-frame dispatcher at width one and wider widths, after topology and
budgets are configured and before root publication. Dedicated execution uses
its fixed bounded commit executor; it never silently selects shared retirement.
This staging is not production activation or completed E1 acceptance.

`hirsel run --native --trace-events events.jsonl` and an embedding host's
`RunHandle::with_trace_events` select canonical JSONL export. A standalone
executable takes the observer prefix `--hirsel-trace-events events.jsonl`
before its run or replay arguments. While E1 is unqualified, these requests
refuse before program entry, without opening the trace destination. They do
not opt a program into physical dispatch or change its run identity.

The collector and exact charge observer are available in production builds.
The task-local journal allocates only for an enabled trace; untraced execution
retains bounded summaries with the existing 64-piece/1024-envelope limits.
The complete trace is copied and validated after root completion, before
scheduler destruction. Running workers, unsupported initialized values and
incomplete evidence refuse export. Only validated evidence can create a file;
the existing pinned recorder checks protect mounts and sinks, and distinct
destination checks protect recordings and placement sidecars from aliases.
Native direct tests exercise this writer while the production gate is closed.

This is a protocol and guarded migration substrate, not an enabled concurrent
native ordered executor. The default remains W1's cooperative run token.
Concurrent drivers must first validate their service lifetimes and field access,
install saved-fiber dispatch and supply capture/type metadata, operation logs,
physical-allocation hooks, root-release positions and canonical event emission.
The width-1 canonical emitter remains an unfinished R5 dependency.

Protocol differentials cover widths 1/2/4/8, reorder off/auto and 32 seeded
arrival orders (256 combinations). These are Rust protocol programs, not the
language corpus gate. They do not establish the design's native speedup gate.
Runtime controls additionally exercise role-enabled task-table growth at widths
1 and 4 with migration. No native parallel speedup is claimed.

## Concurrent saved host waits

The explicitly admitted `ConcurrentAdapter` route accepts parked requests when
`BodyRun.concurrent_adapter` supplies their run-owned service. An unowned or
different adapter still refuses before an ordinal, token or host effect. All
widths, including one, use the same physical saved-frame dispatcher.

Each request keeps one token and ordinal across Blocked, spurious readiness and
completion. Initial calls execute at the head; the existing retirement/home
poll executes retries in deadline and park order. It validates generation,
wait token and physical save, installs owned discovery and attempt/completion
evidence, then publishes wakes. Adapter callbacks retain no scheduler role or
parked TaskCtx borrow. Home and concurrent sources share an idle wait, and the
combined readiness snapshot preserves armed deadlines and sorted token lookup.
The run retains completion buffers until their owner consumes them, and finishes
abandoned registrations before its task metadata is destroyed.

Controls compare canonical traces at widths 1/2/4/8 with reorder off/auto for
sleep and byte-input waits, including the real virtual-time readiness driver.
Cancellation, a spurious retry, reused task rows, mixed home deadlines, and an
external stop have focused controls. These are scoped native controls; the
full language corpus, general terminal cuts and production activation remain
qualification work. The normal launcher still uses its existing home route
while the independent E1 production gate is OFF.

## Exact home output prefixes

`Adapter::print_observed` adds an optional exact written-byte count to the
existing bounded borrowed print. Its default preserves `print` and reports an
unknown count. RealIo uses the existing counted write/flush loop in both its
borrowed fast path and recording path; validated SimIo replay observes the
recorded prefix without consuming a mismatched input. Shared/home facades
forward both the completion and count while keeping non-Send adapters on home.

Ordered trace capture copies only the observed visible prefix before yield.
It certifies full writes, partial write errors, zero writes, flush errors and
validated cancellation with the same owned request/result identities. Missing,
out-of-range or completion-inconsistent counts remain unexportable. Neither a
successful completion nor an error supplies an inferred count. Untraced output
continues through the existing print path. Native controls compare canonical
bytes with actual output at widths 1/2/4/8, reorder off/auto and history on/off.
Production Ordered remains OFF pending the remaining qualification gates.

## Declared home resource graphs

Home adapters may supply optional `keys`, `identity` and `domains` observations.
Absent declarations preserve conservative Coupled behavior. Shared and home
facades forward declarations without requiring a Send adapter; `Coupled<A>`
retains its explicitly conservative concurrent contract. The Ordered observer
copies metadata outside the scheduler role and uses the existing initialized
result-field walk and atomic resource discovery to name aliases, split halves
and their parents. Numeric handle and key allocation order never enters a name.
Saved retries must retain their entered footprint; missing or conflicting
metadata refuses export while preserving the adapter's live result.

Auxiliary domains use an already discovered owner's logical resource and a
stable label. Built-in Net port allocation and inherited process sinks expose
those snapshots before effects; aliases and shared sinks retain one resource.
Canonical JSONL adds the `domain` resource kind with `owner` and `label`, and
publishes its creation before its first access. Controls vary native keys and
handles across widths 1/2/4/8 and reorder off/auto, exercise parked discovery,
file aliases and split-direction use, and reject changed or missing metadata.
They extend the existing host graph and saved-wait protocol; production
activation, terminal cuts and full differential qualification remain gated.

## Host terminal ownership

Ordered home requests and their returned buffers use the existing task-owned
`pending_ordered_host` across entry, metadata callbacks, token registration and
post-effect yield. A blocked retry moves into its saved wait row. Implicit
handle closes use the same observed boundary. Traced output uses that owned
route; untraced borrowed output keeps its fast path.

A home callback additionally retains a trace-only owned result, initialized
discovery and exact output witness before releasing its adapter loan. This
snapshot survives a stop that prevents the caller from reacquiring a compute
permit. Normal initial publication releases it. Quiescent trace capture can
publish a stopped transfer without calling the home adapter or sampling a
parked task context. The home adapter and its non-Send state still die on the
admission thread, and production stop returns promptly with lingering workers.

A terminal boundary records `host_abandoned` with phase `blocked` or `result`
for each fully observed unfinished host phase. It does not manufacture a caller
`host_finished`. The entire phase batch is validated before it is closed;
missing requests, invalid discovery and unknown output prefixes still refuse
export. Task-local terminal ordering and first-stop budget witnesses retain
their existing retirement path. An external Requested stop is projected only
after workers leave; a trace request with running workers still refuses.
Controls compare bytes at widths 1/2/4/8 and reorder off/auto for saved blocked
requests, completed saved input, stopped home transfer, exact partial output
and implicit close. Production Ordered remains OFF pending later gates.

## Dedicated commit execution

Explicit Ordered admission with `commit="dedicated"` installs one run-owned
commit thread. Frontier commands, retirement and partial head-budget settlement
execute there under the issuer's delegated scheduler role. Physical dispatch,
saved-frame publication and readiness polling retain their existing guarded
worker paths. Bounded head effects and continuations return synchronously to
the issuer to preserve home, pool and foreign TLS affinity. No task context
moves to the commit thread, and no user code or suspension runs in its loan.

The issuer lends its existing process compute permit under the permit-state
lock; no additional executing capacity appears, including at process grant 1.
Home/pool foreign transitions without a permit release the role using `Segment`
before acquiring a temporary permit, reenter before publishing the loan, and
return that temporary permit afterwards. An idle committer holds no permit.
The committer participates in stopped-run lost-permit accounting. The run
outlives every synchronous issuer; its final owner closes and joins the idle
committer before destroying the service.

The callback loan follows `pool::call_home_in` and `pool::run_call` (pool.rs),
including predicate-under-lock notification after the borrowed callback has
fully returned. Unlike withdrawable home I/O, a started commit loan finishes
restoring Frontier before observing a requested stop. Stop still returns
promptly to the launcher with a pending worker, retaining its lent permit
until it actually leaves. Controls cover two competing real sibling workers,
shared/dedicated canonical byte equality at widths 1/2/4/8 and both reorder
modes, one-permit role contention, foreign affinity, host saved waits, child
Holding/rest cleanup, channel/mutex carriers and exact first budget cuts.
Production qualification remains false pending the corpus and final gates.

## Run-local auto decisions

The native driver now feeds the existing M0b policy with its own observations.
Each 64-turn probe window measures segment duration, accepted early results,
offer overhead and useful continuation time after an early result. Physical
switch-out ends a sample; suspended time earns no credit. Continuations that
have become head also earn no parallel credit. These are host timing estimates,
so preemption may change the next policy window without changing any semantic
operation, budget, run identity, tape input or early-commit safety rule.

Short, conflicting and losing windows disable early commit. After one strict
64-turn window, a bounded probe allows recovery when the workload changes.
The thresholds remain M0b's existing constants: 2 microseconds mean segment,
25 percent eligible probes and 10 percent gain margin. Explicit `reorder: off`
collects no timing rows and reads no policy clock. Completed tasks discard their
timing rows; there is no worker-local or global adaptive state.

Focused native controls exercise real shared-object siblings at widths 2/4/8
under shared and dedicated commit, including fallback, periodic recovery and
refusal of conflicting early mutation. The existing allowance, envelope,
fuel/memory order, stop and dedicated-loan controls retain their bounds.
Production qualification remains disabled pending the remaining acceptance gates.

## Prior art and deviations

- The queue and call transitions are ports of the executable checked model,
  `proofs/ordered/Ordered.tla:398` (ClearPending), `:416` (Absorb), `:442`
  (ContinueCall), and its `Eligible`/`Fp` definitions. Its mutation traces are
  the regression cases in `ordered/tests.rs` and `channels.rs`.
- Holder scan lowering follows this repository's existing native drop-program
  lowering (`hirsel-lir/src/rt_program.rs`, `Lower::program` and `Lower::arm`)
  and the checked `ScanStep` graph. It retains tag/live-bit/loop structure but
  uses a separate read-only program, visits shared graph nodes once, and returns
  unknown on unsupported storage or bounded work instead of inferring from drops.
- Role waiting follows Rust std commit
  `ea137335b78829b4514bf1b4c16302f74fab8581`,
  `library/std/src/sync/poison/condvar.rs:182-194`: inspect and change the
  predicate under the same lock, and retry after waking. The task owner replaces
  a thread-bound MutexGuard because fibers migrate between OS threads.
- Tokio commit `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
  `tokio/src/runtime/scheduler/multi_thread/worker.rs:570-622`, runs local
  work, steals, then parks. Ordered mode's logical FIFO retirement is an
  intentional departure: physical arrival order cannot change observations.
  W1/F1 own the worker queue and thread lifecycle; this module does not duplicate
  their stealing implementation.

The tests also found an empty-ring growth error in the existing channel ring:
`at(0)` evaluated `capacity - 1` before the first allocation. Growth now skips
copying the absent old buffer; nonempty ring copying is unchanged.


Runtime ABI 48 appends a bounded callable-code identity table to the compiled
program descriptor. It is separate from RUN-3's admitted Entry definitions and
adds no runner authority. Both emitters retain the stable symbols from unit
summaries, including cached units. Canonical values resolve code addresses to
those names, and erased environments through their actual linked descriptor.
Unknown code or missing value metadata still refuses the whole observation.

Initialized-value decoding now includes callable pairs, iterator triples and
once environments. Environment live bits select only present captures; moved
fields and padding are never read. Runtime file-walk iterators have a fixed
built-in code identity and encode only unread results, resolving Path words to
text and reading only active FileError fields. The cursor and consumed rows
are private storage. Generic nested functions with a null code word use the compiler's concrete
closure key, which includes definition, body and capture types. An erased null
code without that key still refuses; environment bytes cannot establish code
identity.

The variable-layout walk follows the existing codec's FlatFormatV0 cursor,
Packed active-arm extents, Sequential used-byte prefix and declaration-order
SoA rows (`codec.rs`, `flat_extent` and `encode`). Linked value metadata retains
all SoA fields, including scalars absent from holder scans. Checked allocation
bounds, framing, live counts, tags, work and output limits reject incomplete
images. Spare capacity, inactive arms, native addresses, count words and
padding never enter canonical values. This does not expand holder/type
admission or change the four early-retirement rules. Production qualification
remains a separate gate.
