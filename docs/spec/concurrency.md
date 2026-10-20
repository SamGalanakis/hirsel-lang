# Concurrency

Concurrency is structured: tasks are spawned into a scope that outlives them, share no
counted memory, and communicate by moving sendable values through channels, by locking
mutexes whose bodies hold no authority, through atomics, or by reading frozen `Shared`
values. Every synchronising operation takes the `Tasks` capability, so its order is an input
of the run ([DET-1](determinism.md#det-1-contract)).

### CONC-1: Runtime

`Io` is an interface; its implementation (threads, a pool, green threads, io_uring, a
deterministic simulation) is chosen by the run configuration. An implementation may suspend
a task only at `Io` calls, capability calls under [CAP-4](capabilities.md#cap-4-synchronisation)
and cancellation checks, and is fair among tasks that reach those points. A task looping
with no such point may starve others on a cooperative implementation. Whether a program
terminates may depend on scheduling decisions. Whether children run in parallel is the
implementation's choice.

### CONC-2: Tasks

The core operations are `task.scope(tasks, body)`, `task.async(s, take f) -> Task[R] of
s`, `task.await(s, take h)`, `task.cancel(s, h)` and `task.detach(s, take h)`. The scope
body is once and may take owned values; children cannot outlive it. A normal exit waits
for unawaited children; an error or failure exit cancels them, then joins. A child may
start inline, but then runs only until its first suspension. Every task handle is awaited
or explicitly detached; detaching discards the result but never lets the child outlive its
scope. Detached completion releases its result and scheduling metadata without waiting for
scope exit; an already-completed result is discarded by `detach`. A detached child's
failure still belongs to the scope and is ordered with other child failures by spawn
order. Foreign functions declared `@blocking` run on a bounded synchronous pool, except a
call to a `home` package, which runs on the admission thread, and a nested call, which
runs on its enclosing call's thread. See [PROG-4](programs.md#prog-4-interop) for the
trusted package threading declaration.

A task set collects handles of one scope: `task.set(s)` creates an empty `TaskSet[R] of s`,
and `task.add(s, mut set, take h)` moves `h` in as its next member. `task.await_any(s, set)`
waits until an unawaited member has completed and returns the add-order index of the one that
completed first, or `None` when every member was awaited. Completion order is the
scheduler's, an input of the run like every scheduling choice, so a replay chooses the same.
`task.await_at(s, mut set, index)` awaits member `index`; one never added or already awaited
fails CONC-2. `task.cancel_all(s, take set)` cancels every unawaited member and returns once
each has completed, dropping their results; `task.detach_all(s, take set)` detaches them. A
failure of a member that nobody awaited belongs to the scope, as a detached child's does. The
standard library builds the combinators on these:

```text
fn task.all[T, E](s: Scope, take set: TaskSet[Result[T, E]] of s) -> Result[List[T], E]
fn task.select[R](s: Scope, take set: TaskSet[R] of s) -> (u64, R)
```

`task.all` returns every result in add order, or the first `Error` to complete once it has
canceled and joined the members still running. `task.select` returns the add-order index and
result of the member that completes first once it has canceled and joined the others; an
empty set fails CONC-2. Neither returns while a member runs.

A set holds its members' obligations, so it is consumed exactly as a handle is. A handle or
set passed `take` to a parameter declared `Task` or `TaskSet` (`task.await`, `task.detach`,
`task.add`, `task.cancel_all`, `task.detach_all`, or a function of the program) takes its
obligation along, and in that function's body the parameter holds it. Passing one anywhere
else, such as into a `List`, leaves the obligation behind. A set dropped by a failure exit
leaves its members to the scope's exit join.

```hirsel
fn sizes(tasks: Tasks, dir: Dir, a: Path, b: Path) -> Result[(u64, u64), FileError] {
  task.scope(tasks, fn(s: Scope) -> Result[(u64, u64), FileError] {
    let ha = task.async(s, fn() -> Result[u64, FileError] { file.size(dir, a) })
    let hb = task.async(s, fn() -> Result[u64, FileError] { file.size(dir, b) })
    Ok((task.await(s, take ha)?, task.await(s, take hb)?))
  })
}
```

A child capturing a borrowed `List[u64]` instead is CLO-3: a counted value that is not
unique is not sendable. A child may copy its parent's `Mutex` or `Atomic` handle, or a
record of such handles and plain fields, or take a Sender, since those handles are
run-owned ([MODE-4](ownership.md#mode-4-sendable)).

### CONC-3: Cancellation

Cancellation is level-triggered: once a scope is cancelled, every later blocking `Io` call
in it returns `Canceled(cause)` after the program checks below, and every blocking `Io`
error type includes that variant. Each blocking call first performs its effect-free
program checks in their specified order, then checks cancellation before any host effect,
and only then observes deadlines and provider outcomes (including resets, I/O errors and data):

- Argument validity (`InvalidInput`, including zero read limits and timeouts, repeated TLS
  upgrades, TLS verification names and trust bundles, canonical process paths and NUL-free
  arguments).
- Authority (`NotGranted`, including network rule/name checks and file access rights).
- Other effect-free program checks, including lexical path confinement, file modes and
  seek bounds, and a child's explicitly closed stdin.

Nonblocking grant projections and clock sampling introduce no cancellation check and use
their stated grant or source error variants. Joins (`task.await`, `task.await_any`,
`task.await_at`, `task.cancel_all`) and mutex acquisition are not cancellation points: a
cancelled scope cancels its children, so a join waits only for them. `channel.send` is not
a cancellation point: it succeeds or returns `Closed` with its value whatever its scope's
cancellation; `task.check_canceled` marks producer loops. `Canceled` is returned only when
the operation had no effect. A runner call (planned) is a cancellation point only until
its request is sent; after that its scope's cancellation is forwarded to the place and the
call waits for the acknowledgement ([RUN-9](runners.md#run-9-cancellation)). A store
transaction (planned) checks cancellation before its body starts; a body, once started, is
not cancellable ([STORE-3](storage.md#store-3-transaction-scope)). Pure code is not
cancellable; `task.check_canceled(s)?` marks long loops. Each operation's signature states
what it owns on success and on failure; a borrow or `mut` loan lasts until the call
completes. Deadlines read the granted monotonic clock. A scope can carry a deadline:
`task.scope_until(tasks, deadline, body)` runs `body` in a fresh scope as `task.scope`
does, and when `deadline.clock` reaches `deadline.at` before that scope has exited,
cancels the scope with cause `DeadlineExceeded`. From then every blocking `Io` call the
body makes in the scope returns `Canceled(DeadlineExceeded())`, each child of the scope is
cancelled with that cause and the children of scopes nested in the body with `Parent`, and
the exit joins them as an error exit does. The call returns `Expired(r)` when the deadline
cancelled the scope and `Finished(r)` otherwise, where `r` is the body's result. A
deadline already reached at entry, or whose clock cannot be read, cancels the scope before
the body runs. Nothing polls for a deadline: each scope arms one timer on its deadline's
clock and removes it at exit. A pending scope deadline is an outstanding event source
([CONC-8](#conc-8-global-quiescence)), so a body waiting for a message nobody will send is
cancelled at its deadline, never Blocked. A network timeout (`NetTimeout`) is not a
deadline: it is a relative bound measured by the network provider, observed only as the
outcome `TimedOut()`, and needs no clock grant. Drops run shielded. An operation
interrupted by environmental termination is never described as `Canceled` unless no effect
was established under its contract; an unknown outcome is not a cancellation result.

### CONC-4: Memory model

Programs are data-race-free by construction: tasks share no mutable or counted memory
except mutex payloads (under the lock), `Shared` root counts and atomics. Every execution
is explained by a sequentially consistent order of synchronising events: spawn before the
child starts; child completion before its join and the scope's exit; send before receive;
mutex release before the next acquire; atomic operations; suspend before resume; and the
releases of a root that a `shared.thaw` of it observes, before that thaw. The outermost
entry of a `serial` package's foreign gate is also a synchronising event
([PROG-4](programs.md#prog-4-interop)); nested re-entry is not.

### CONC-5: Mutexes

`mutex.create(tasks, take v: unique T)` for sendable `T`.
`mutex.with(tasks, m, fn(mut v: unique T) { .. } uses {})` runs its body synchronously in
the acquiring task. The body holds no authority, so it cannot block, do I/O, take another
lock or use a channel, and lock-order deadlock is impossible. It may capture inert values
under the ordinary borrow rules and take sendable ones; values stored into the payload are
unique, and its result must be sendable. A `unique` copy of the payload or of a part of it
(`unique v.items`, `unique v.items[0]`) is fresh, so it is a sendable result whenever its
type is; a plain `copy v.items` shares the payload's blocks and cannot leave the body.
Payload counts are plain, serialised by the lock.

```hirsel
fn tally(tasks: Tasks) -> u64 {
  let m = mutex.create(tasks, take 0u64)
  mutex.with(tasks, m, fn(mut v: unique u64) -> u64 uses {} {
    v += 1
    v
  })
}
```

### CONC-6: Channels

`channel.open(tasks) -> (Sender[T], Receiver[T])` for sendable `T`. A Sender is a
noncopyable resource, duplicated with `sender.dup(tasks, s)`, and may travel inside
payloads; a Receiver is noncopyable and crosses tasks only as a task-closure capture, never
inside a payload. `channel.send(tasks, mut s, take v) -> Result[(), (T, ChannelError)]`;
`channel.receive(tasks, mut r) -> Result[T, ChannelError]`. Dropping the last Receiver drops
buffered values and makes later sends return `Closed`; dropping the last Sender ends
receiving once the buffer drains.

```hirsel
fn pipe(tasks: Tasks) -> Result[u64, ChannelError] {
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  let mut tx = take sender
  let mut rx = take receiver
  match channel.send(tasks, mut tx, 7u64) {
    Ok(()) => {},
    Error((_, e)) => return Error(e),
  }
  channel.receive(tasks, mut rx)
}
```

### CONC-7: Atomics

`Atomic[T]` holds scalars only, is sequentially consistent, and is created and operated
through `Tasks` ([CAP-4](capabilities.md#cap-4-synchronisation)).

### CONC-8: Global quiescence

An execution is globally quiescent when it has live tasks, none is runnable, no readiness
transition is enabled, and no outstanding event source can make a task runnable. This is
**Blocked**, an observation of noncompletion, not a failure, a cancellation or a scope exit.
A deterministic `Io` simulator recognizes it after processing enabled transitions and before
waiting for input that cannot arrive. A real `Io` runtime recognizes it when every wakeup
source is runtime-owned and exhausted; an outstanding external event of unknown completion
prevents that conclusion. No implementation must decide nontermination of running code or
of external operations. Reporting Blocked runs no user code, cancellation or destruction
([the Blocked observation](determinism.md#the-blocked-observation)).

A receive on an empty channel with a live Sender keeps waiting even when the receiving task
owns that Sender; whether the execution is Blocked is decided by this rule. Blocked is never
returned from `receive`, `await` or `task.isolate`.

Callee runs of in-process runner providers (planned) belong to their caller's execution
for this rule; a request outstanding at an out-of-process place is an external event of
unknown completion ([RUN-12](runners.md#run-12-quiescence-across-runners)). Blocked is
never returned from a runner call; a callee Blocked at an out-of-process place reaches its
caller as `InDoubt(Abandoned())`.
