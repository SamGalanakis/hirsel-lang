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

Native descriptor version 14 (runtime ABI 34) adds an independent holder scan
pointer and word count, taking the C descriptor from 72 to 88 bytes. Both
native emitters lower the existing `LayoutRecord.scan`; destruction programs
cannot substitute for it because atomics and tasks are not resources.
`ordered::capture` visits active variants, live captures, counted graphs,
inline arrays, sequences and structure-of-arrays columns. Shared values are
inert except for a mutex's exact-root wrapper. Missing descriptors, borrowed
pointers, variable-flat sequences and scans exceeding the work bound remain
unknown. Zero scan words certify an empty holder set; `u64::MAX` means unknown.
The raw handle set still needs typed object resolution and obtainable-type
closure before it can supply `Held`; ambiguous words never certify independence.

`ordered::objects::Registry` supplies stable identities and obtainable-type
bindings. A raw word shared by multiple native handle families contributes all
their possible footprints; an unclassified binding keeps the result unknown.
The typed operation boundary resolves its own family exactly. A future object
registered after a capture still participates in that capture's type closure.
`tasks::ordered` connects the frontier to native atomic load/store, channel
send, and mutex take/put effects using owned ABI images. Unavailable mutexes
cannot commit early. Its record and poll callbacks run only at retirement;
an early send's receipt publishes and touches the channel before that poll.
These adapters are not yet installed in the fiber entry path. Creation,
provisional handles, drops, task operations and host operations still need
native boundary adapters.

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
