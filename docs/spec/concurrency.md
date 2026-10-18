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

The core operations are `task.scope(tasks, body)`, `task.async(s, take f) -> Task[R] of s`,
`task.await(s, take h)`, `task.cancel(s, h)` and `task.detach(s, take h)`. The scope body is
once and may take owned values; children cannot outlive it. A normal exit waits for
unawaited children; an error or failure exit cancels them, then joins. A child may start
inline, but then runs only until its first suspension. Every task handle is awaited or
explicitly detached; detaching discards the result but never lets the child outlive its
scope. Detached completion releases its result and scheduling metadata without waiting
for scope exit; an already-completed result is discarded by `detach`. A detached child's
failure still belongs to the scope and is ordered with other child failures by spawn
order. `task.all` and `task.select` are library functions built on these. Foreign functions
declared `@blocking` run on a bounded synchronous pool.

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
unique is not sendable. A child may copy its parent's `Mutex` or `Atomic` handle, or take a
Sender, since those handles are run-owned ([MODE-4](ownership.md#mode-4-sendable)).

### CONC-3: Cancellation

Cancellation is level-triggered: once a scope is cancelled, every later blocking `Io` call
in it returns `Canceled(cause)`, and every blocking `Io` error type includes that variant.
Nonblocking grant projections and clock sampling introduce no cancellation check and use
their stated grant or source error variants. Joins and mutex acquisition are not
cancellation points. `Canceled` is returned only when the operation had no effect. Pure code
is not cancellable; `task.check_canceled(s)?` marks long loops. Each operation's signature
states what it owns on success and on failure; a borrow or `mut` loan lasts until the call
completes. Deadlines read the granted monotonic clock. A scope can carry a deadline:
`task.scope_until(tasks, deadline, body)` runs `body` in a fresh scope as `task.scope`
does, and when `deadline.clock` reaches `deadline.at` before that scope has exited, cancels
the scope with cause `DeadlineExceeded`. From then every blocking `Io` call the body makes
in the scope returns `Canceled(DeadlineExceeded())`, each child of the scope is cancelled
with that cause and the children of scopes nested in the body with `Parent`, and the exit
joins them as an error exit does. The call returns `Expired(r)` when the deadline cancelled
the scope and `Finished(r)` otherwise, where `r` is the body's result. A deadline already
reached at entry, or whose clock cannot be read, cancels the scope before the body runs.
Nothing polls for a deadline: each scope arms one timer on its deadline's clock and removes
it at exit. A pending scope deadline is an outstanding event source
([CONC-8](#conc-8-global-quiescence)), so a body waiting for a message nobody will send is
cancelled at its deadline, never Blocked. A network timeout (`NetTimeout`) is
not a deadline: it is a relative bound measured by the network provider, observed only as
the outcome `TimedOut()`, and needs no clock grant. Drops run shielded. An operation
interrupted by environmental termination is never described as `Canceled` unless no effect
was established under its contract; an unknown outcome is not a cancellation result.

### CONC-4: Memory model

Programs are data-race-free by construction: tasks share no mutable or counted memory
except mutex payloads (under the lock), `Shared` roots (atomic) and atomics. Every execution
is explained by a sequentially consistent order of synchronising events: spawn before the
child starts; child completion before its join and the scope's exit; send before receive;
mutex release before the next acquire; atomic operations; suspend before resume.

### CONC-5: Mutexes

`mutex.create(tasks, take v: unique T)` for sendable `T`.
`mutex.with(tasks, m, fn(mut v: unique T) { .. } uses {})` runs its body synchronously in
the acquiring task. The body holds no authority, so it cannot block, do I/O, take another
lock or use a channel, and lock-order deadlock is impossible. It may capture inert values
under the ordinary borrow rules and take sendable ones; values stored into the payload are
unique, and its result must be sendable. Payload counts are plain, serialised by the lock.

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
