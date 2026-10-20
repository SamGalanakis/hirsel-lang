# Concurrent programs for coding agents

Express concurrency with tasks, owned messages and explicit capabilities. The
runtime chooses execution width; programs cannot read the core count. The same
code runs on today's cooperative provider and on multicore providers.

## Fan compute out under a nested scope

Use `task.map_all(tasks, take items, grain, f)` for independent computations.
It opens its own scope, gives each child up to `grain` consecutive items, and
returns results in input order through `task.all`. `f` borrows each item and
returns a `Result`; it has `uses {}` and an inert owned environment. Items and
results are copyable and inert; the library freezes them for safe sharing. The first
completed error cancels and joins the remaining children before returning.
Choose a positive grain from the cost of each item, keeping enough useful work
in each child to amortize task creation. Zero grain fails SEM-1, even for empty
input. Empty input with positive grain returns `Ok([])`.

```hirsel
fn square(n: u64) -> Result[u64, ()] uses {} { Ok(n * n) }

fn squares(tasks: Tasks) -> Result[List[u64], ()] {
  let items: List[u64] = [1, 2, 3, 4]
  task.map_all(tasks, take items, 2, square)
}
```

For tasks that need capabilities or different result types, open `task.scope`,
start children with `task.async`, add them to a `TaskSet`, and join through
`task.all`. A scope owns the lifetime of every child. Freeze large immutable
inputs into `Shared` before sharing them across tasks.

## Give mutable state one owner

Use an actor when a state update may wait or perform I/O. `actor.spawn` starts
one task owning `unique S` and a FIFO mailbox. Its handler receives the state
by `mut` and consumes each message. `actor.Actor[M]` owns a sender; acquire
another handle with `actor.dup(tasks, actor)`. Handles cannot be copied
implicitly. Releasing every handle lets the task drain queued messages and
finish; an error or failure exit cancels and joins the actor. Release mailbox
handles before a normal scope exit, which waits for its children.

```hirsel
type CounterMsg = Add(u64) | Get(actor.Reply[u64])

fn counter(tasks: Tasks) -> Result[u64, ChannelError] {
  task.scope(tasks, fn(s: Scope) -> Result[u64, ChannelError] {
    let endpoint = actor.spawn(s, tasks, take 0u64,
      fn(mut n: unique u64, take message: CounterMsg) {
        match take message {
          Add(k) => n += k,
          Get(reply) => { let _ = actor.reply(tasks, take reply, n) },
        }
      })
    let mut state = take endpoint
    match actor.cast(tasks, mut state, Add(5)) {
      Ok(_) => (),
      Error((_, error)) => return Error(error),
    }
    actor.call(tasks, mut state,
      fn(take reply: actor.Reply[u64]) -> CounterMsg { Get(take reply) })
  })
}
```

`actor.call` creates a reply channel, sends its sender in the request and waits
on the receiver. If the handler abandons the reply, the caller gets `Closed`.
`actor.reply` consumes the reply endpoint; a caller that has gone away gets its
value returned with `Closed`. Waiting for a reply may return `Canceled`.
Avoid a synchronous call cycle between actors: both would wait for a handler
that cannot run until its current call finishes.

For short, nonblocking updates, use `mutex.with(tasks, state, body)` instead.
The body cannot wait, do I/O, acquire another mutex or use channels. The
[key-value server](../examples/kvstore/main.hir) shows parallel parsing, a
mutex as the single state writer and a separate writer task for output.

## Make cancellation and scheduling deliberate

A cast is a channel send: it succeeds or returns the closed mailbox's message
without checking cancellation. Put `task.check_canceled(s)` in producer loops
so that a canceled producer stops sending. Receiving and I/O remain
cancellation points. Already queued messages may be left when a scope cancels
its actor; do not treat a successful cast as an acknowledgement of processing.

Avoid hot atomics under `tasks.free@1`: contended loads and stores serialize
useful work and add recording traffic. Batch related updates into a message
or a mutex body. The free provider is a first-class execution and replay mode;
the choice of provider does not change ownership or capability requirements.

Exercise alternate interleavings with the test runner's seeded schedulers:

```sh
hirsel test app/hirsel.json --scheduler random@1:42
hirsel test app/hirsel.json --explore 100 --scheduler pct@1 --race
```

Keep the seed printed for a failing test. In interpreted tests, `--race`
names both operations in each application-level race candidate, including
task paths, source locations and the shared object. Candidates are possible
interactions, not proof of what caused the failure. Replay the printed command
to reproduce its scheduler seed and simulator point settings. See
[testing](building.md#schedules) and [determinism](spec/determinism.md) for recording and
replay, and the [API index](api.md) for complete signatures.
