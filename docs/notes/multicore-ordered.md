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
settlement. Host and remaining boundaries still need native adapters. Strict-head adapters now cover scope entry, committed spawn,
holding result publication, join, detach, nonempty scope exit, receive and
physical completion.
Spawn scans the unstarted environment with the linked capture program. Task
result types remain erased by the native spawn ABI, so captured Task handles
are conservatively unknown, including the spawning parent's obtainable footprint.
Receive outputs are scanned into their owner's next-turn held set. Native task sets, add/has/take/await-any, member detach/cancel
and cancel checks now have strict-head adapters too. TaskSetTake refreshes the
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

## Integration status

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
