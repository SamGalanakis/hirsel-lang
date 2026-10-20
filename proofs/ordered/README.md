# TLA+ model of ordered multi-core execution

A TLC-checked model of `tasks.cooperative@1` at width > 1: turns, the retirement frontier,
run-ahead (refinement 1) and early commit of the set E (refinement 2), as the multicore design
specifies them in section 4.3 (with the op classes of 4.1 and the budget settlement of 6). It is
lane E0 of that design. TLC checks it against the width-1 run for 3 tasks of up to 4 ops each,
over every interleaving. It checks invariants (a)–(c) of section 4.3 Soundness, every named
trace of section 4.3, and a mutation run per rule, which shows that each rule is needed.

## Toolchain and commands

TLC from `tla2tools.jar` **1.7.4** (TLC 2.19), pinned by SHA-256 in `check.sh`. The script
downloads it once to `~/.cache/tla/` (or uses `$TLA2TOOLS_JAR`). It needs Java 11 or later.

| command (in `proofs/ordered`) | what it checks |
|---|---|
| `./check.sh` | every run in its table has its expected result (about 4 minutes, mostly JVM start-up) |
| `./check.sh '<regex>'` | only the runs whose names match |

`tools/check-native.sh` runs `check.sh` as a stage. `tools/gate` runs it in the floor when a
change touches `proofs/ordered/` (`tools/buck2/impact.py`, `proof_checks`).

## The model

`Ordered.tla` is one state machine, written as functions over a state record `st`.

- **The head step** (`HeadStep`) is deterministic. The head task absorbs its log in program
  order (it commits logged ops, takes over early-committed ones and settles budget pieces), then
  runs its next op. A turn's end runs the fifo@1 scheduling point: `poll_parked` with
  `wake_receivers` and the park-order polls, then the queue append and pick, the host wait loop,
  and CONC-8 Blocked. It also runs calls (`hand_off`): a holding child's rest runs as the head,
  and its end returns to the caller with no scheduling point. These follow
  `crates/hirsel-rt/src/tasks.rs` (`try_yield`, `try_block`, `poll_parked`, `poll`, `completed`,
  `detach`, the Holding finish).
- **The width-1 reference** (`SB`, `SF`) is `HeadStep` iterated to the end of the run, computed
  once per program. `SB` uses the scenario's budgets and `SF` none, because an early commit past
  a budget stop is internal.
- **The other workers** (`AheadSucc`) can each step any queued non-head task at any time, so every
  width and interleaving is a behavior. Such a step does one of these:
  - charges a budget piece, or pauses: past the speculation allowance, or with more fuel in its
    stretch than remains;
  - logs a D op (refinement 1);
  - commits an E op early when rules 1–3 hold (`Eligible`; a send's message is then pending,
    which is rule 4);
  - stops, pending, at any other op.

  Whenever an early commit is allowed, the model also lets the task log or pend instead, so
  `reorder: off` and every `auto` choice are covered.

Rule 3's possible footprint (`Fp`) is computed the way an implementation can compute it: from
held handles, not from the program. It is the union of:
- the objects of the handles the task holds;
- the type closure of what it can still obtain through them (`carries`);
- recursively, the possible footprints of its holding children;
- for a child running inside a call, its callers'.

The scheduling point's term (parked receivers' channels) is a filtered read, so it conflicts
with no E op, and that is exactly what rule 4 justifies.

### Invariants

| design | TLA+ | meaning |
|---|---|---|
| (a) | `TurnsPrefix` | the retired turns equal the width-1 run's first k turns: ops with ordinals and outcomes, poll results, the readiness-point count, the next head and the logical queue. The turn in progress is a prefix of turn k+1, and the external output is a prefix |
| (b) | `HistPrefix`, `PendingExact` | each object's committed sub-events are a prefix of its width-1 sequence, with width-1 outcomes. A message is pending exactly while its sender's early-committed send is unretired |
| (c) | `QueueEqual` | the logical queue equals the width-1 queue after the same scheduling points |
| — | `FinalEqual`, `NoFault` | at the end the run equals the width-1 run: turns, output, stop kind, and histories when no budget stop. No take of a held mutex and no use of an unheld handle |

`Soundness` is the conjunction. TLC's deadlock check is on, so a stuck frontier also fails a
run. `Behavior` is `Soundness` without `PendingExact`, and is what the mutation runs must
violate, so that they fail on what a run exposes, not on bookkeeping.

## Runs (`check.sh`)

**Generic.** `mix-3x4` checks every combination of four candidate 4-op programs per task (64
programs, 34,457 states) over shared atomics, a mutex, a channel and a `Shared` root.
`tree-mix-3x4` checks 27 programs with cancellation, a host wait, a failing child, detaches of
holding children and a finalizing Receiver drop. Witness runs show that both mixes reach early
commits and logged ops.

**Named traces of section 4.3**, each a scenario in `MC.tla`:

| trace | scenario |
|---|---|
| a Sender close (keep, logged) then an atomic store, against a runnable load | `CloseStoreLoad` |
| a Receiver drop whose finalizer yields | `DropFinalizerYields` |
| `detach_all` over two holding members whose cleanups yield | `DetachAllYields` |
| a head detaching a holding child whose finalizer yields while a queued sibling has committed an independent early send | `DetachWhileEarlySend` (and `DetachCallTerm`) |
| readiness-point counts against width 1 | `ReadinessCounts` (every scenario compares counts in (a)) |
| an early send to a parked receiver stays pending, then wakes it at its turn's scheduling point | `EarlySendParked` |
| an older head's poll of a parked host waiter while a younger parent logs its `task.cancel` | `CancelHostWaiter` |
| a sibling's failure against an early store (passes) and a receive (never early) | `FailureStoreRecv` |
| thaw against a younger release | `ThawRelease` |
| a mutex body reaching thaw | `MutexBodyThaw` |
| review-2 finding 3, both counterexamples | `Budget2a`, `Budget2b` |
| review-3 finding 3, both counterexamples (the first in its two orders) | `Budget3a`, `Budget3b`, `Budget3c` |
| two siblings' mutex, atomic and send ops pass a long head | `PassMutexAtomic`, `PassSendLoad` |

**Positive runs** use eager scheduling: non-head steps take priority and every allowed early
commit is taken. They check `Positive`, which says the named ops have committed early by the time
the first turn retires. This covers the passing ops in `DetachWhileEarlySend`, `EarlySendParked`,
`FailureStoreRecv` (the store), `MutexBodyThaw` (the take), `PassMutexAtomic` and `PassSendLoad`.

**Mutation runs.** Each removes one rule (`Mut`) and must fail:

| mutation | scenario | what goes wrong |
|---|---|---|
| `norule1` | `OneTurnPerHead` | a second store, in the task's next turn, passes the load between them |
| `norule2` | `LoggedCloseThenSend` | a send commits before the same turn's logged close of another Sender of its channel |
| `norule3` | `CloseStoreLoad`, `MutexBodyThaw` | a load passes the store before it; a second taker finds the mutex held |
| `noclosure` | `HandleThroughChannel` | a load passes the head's finalizer, which stores through a handle the head obtains from a dropped leftover |
| `nocallterm` | `DetachCallTerm` | a load passes a holding child's rest that the head's detach runs first |
| `nopending` | `EarlySendParked` | the parked receiver takes the early message at the older head's scheduling point |
| `touchatcommit` | `EarlySendParked` | the early send's channel touch is used up by the older scheduling point, so the receiver never wakes at the sender's |
| `nofilter` | `EarlySendParked` (positive) | treating the parked receiver's channel as a conflict stops the early send |
| `cancelE` | `CancelHostWaiter` | the waiter's cancellation ends its wait at the older head's poll |
| `recvE` | `FailureStoreRecv` | the receive takes a message instead of observing its sibling's failure |
| `immrelease` | `ThawRelease` | the younger release passes the thaw |
| `droprecvD` | `DropFinalizerYields` | the dropper's continuation runs before the leftover's finalizer |
| `callnoinsert` | `DetachAllYields` | a logged detach's call runs the child's rest after the detacher instead of before |
| `earlysp` | `ReadinessCounts` | an early-committed yield polls, so readiness counts differ |
| `headseesspec` | `Budget2a` | the head's live check counts a younger segment's speculative bytes and stops |
| `netonly` | `Budget2b` | settling by net bytes misses the younger record high |
| `peakonly` | `Budget3b` | memory is reported where fuel ran out first |
| `mergepieces` | `Budget3c` | settling a turn's stretch before its close's credit stops the run |

## Precisions the model adds to the design

The model needed four choices that the design text implies but does not state. The E1 executor
must make the same ones:

1. **An early send's channel is touched at its retirement, not at its commit.** The runtime's
   touched set is consumed by every `wake_receivers`. If the early send touched its channel at
   commit, an older scheduling point would consume the touch while it skips the pending message,
   and the receiver would not wake at the sender's scheduling point. Retirement step 3 clears
   the pending mark and touches the channel (`ClearPending`; mutation `touchatcommit`).
2. **Rule 3's call term is transitive, and it covers callers.** A holding child's rest can detach
   its own holding child, so the term includes holding descendants. While the head is a child
   running inside a call, the turn's remaining members belong to its callers, so those are
   included as well (`PossFp`, `Fp`).
3. **A queued task that becomes head absorbs its logged turn first.** Before running any new op,
   it commits its logged ops, takes over its early-committed ones and settles its pieces, in
   program order (`Absorb`). A logged detach of a holding member then becomes a call at that
   point.
4. **A leftover's handles join the dropper's handles.** A Receiver drop is a keep op that obtains
   handles a later op of the same turn can use, through the finalizers. In the runtime,
   TaskSetTake is another, outside the model. These are the cases the type-closure term
   (`carries`) exists for. Receive and join end the turn before a received handle can be used,
   and a mutex body has no authority to use one.

## Scope

The model covers what section 4.3's argument depends on. Some things are left out deliberately:

- `join`, `await_any`, scope exit, task sets and completion ranks: task ops outside E, never
  early, whose polls read only member states and ranks, which no E op writes;
- create ops (provisional identities);
- deadlines, extern calls and `serial` gates;
- more than one host key per request;
- `mutex.kill`;
- stops other than budget stops, `done` and CONC-8 Blocked.

The `send` op follows ruling (b): it never checks cancellation. The fifo@1 queue is the only
scheduler modeled. `run_ahead` is 4 (unbounded for 4-op programs), and the speculation allowance
is unbounded except where a scenario sets it.
