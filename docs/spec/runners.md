# Runners

**Status: partly shipped.** `core@1` declares `Runner`, `Entry`, the outcome and grant
types and every operation of the [ledger](#ledger), and the checker forms entries
(RUN-3). The interpreter runs `local.threads@1` and `sim@1` places as callee runs of the
caller's execution (RUN-1 to RUN-7, RUN-9, RUN-10's isolation and derived seeds, RUN-12),
both under a simulation fixture and in a host launch. A host launch records linked tapes
(RUN-11): with `--record T` each callee run that starts journals its own tape to
`T.call-<ordinal>`, every tape replays alone, and `hirsel tapes check` joins a set.
`hirsel tapes trial` replays a tape's calls against a candidate revision
([upgrade trials](#upgrade-trials)). The
arrival check covers the definition allowlists and a runner entry's `admit.safe@1` code
policy (RUN-8). An interpreted host launch calls `remote.tls@1` runners, and `hirsel serve`
is their runner host (crate `hirsel-remote`). The native engines report the operations
unavailable. Durable callee recording (RUN-14), fetching code from a code store at
arrival, and the `local.process@1` provider are planned. Items marked **Reserved** are
designed here but have no implementation owner yet.

**Interim deviations** of the shipped interpreter: a callee's outcome bytes are recorded
as its tape's last event, `call_return ok|err|failed <bytes>`, rather than in the
trailer, so the trailer stays `complete N`; `local.threads@1` interleaves its callee runs
with the caller on the caller's thread, bounded to `threads` running at once, rather than
on operating-system threads; and `hirsel serve` runs one callee at a time (`runs` other
than 1 is refused), holds only its own revision (`code.none@1`), and refuses `durable`
callee recording. A `sim@1` crash on a host launch stops the callee before one of its
requests but keeps every write it completed, since its directories are host directories,
not simulated ones.

A **runner** is a place functions can run. The run configuration decides what each place
is (threads in this process, a child process, another host, a node of a deterministic
simulation) and which authority code running there receives. The program sees one
capability type, `Runner`, and the same operations whichever provider the administrator
chose. Only data crosses a runner: the caller names a top-level definition and passes
serializable arguments; the callee receives capabilities projected from the runner's own
grant set, never from the caller. Every call is one recorded effect at the caller and runs
as its own run with its own recording, so any subset of a system replays.

All of this is core library operations and run configuration. One checker rule is new: the
formation of an `Entry` value from a definition name ([RUN-3](#run-3-entries)). It lowers
to the existing TIR `FunctionReference` rvalue typed as `Entry[F]` and adds a type to the
core type table. No TIR or LIR instruction is added.

### RUN-1: Runner capability

`Runner` is a primitive global capability ([MODE-6](ownership.md#mode-6-scoped-capabilities)):
plain, `sendable`, never `data`, `key` or `serializable`
([CAP-5](capabilities.md#cap-5-capability-sealing)). A `Runner` designates one place and
one grant set. It is obtained only by projecting a runner named in the run configuration
from `Io`:

```text
N fn io.runner(rt: Io, name: String) -> Result[Runner, GrantError]
```

A name the validated run configuration does not list returns `NotGranted()`; the name is
designation inside grants the `Io` already holds, as a mount path is for `io.dir`. Every
runner in the configuration, its provider, its grant set and its provider settings are
validated, and its host resources opened and pinned, during admission before entry
([PROG-3](programs.md#prog-3-entry)); an absent, unenforceable or unauthorized runner is a
launch refusal, never a later `RunError`. No operation creates a runner from data, and a
serialized runner description is inert input to the launcher, never authority. A runner's
grant set is administrator policy: it may hold authority the entry's own `Io` lacks, and
it is not derived from the caller's grants.

Like `Net`, a `Runner` is charged at use ([CAP-2](capabilities.md#cap-2-charged-at-use)):
every call operation takes it explicitly. Holding a `Runner` grants nothing beyond calls
through it; the callee's capabilities exist only inside the callee's run.

### RUN-2: Narrowing

A runner can only be narrowed. Narrowing mints a new `Runner` for the same place whose
grant set is a subset of the original's, and never widens anything:

```text
fn runner.grants(r: Runner) -> GrantSet
fn runner.narrow(r: Runner, keep: GrantSet) -> Result[Runner, GrantError]
fn runner.only(r: Runner, definitions: List[String]) -> Result[Runner, GrantError]
```

`runner.grants` returns the runner's current grant set as data. `runner.narrow` returns a
runner whose grant set is exactly `keep` when `keep` is covered by `r`'s grant set, and
`NotGranted()` otherwise, minting nothing. `keep` is covered when:

- each of `stdout`, `stderr`, `tasks`, `monotonic_clock`, `wall_clock` and `random` is
  true in `keep` only where it is true in `r`;
- each `dirs` entry of `keep` names the same virtual path as an entry of `r`, with equal
  or lesser access (`Read()` is less than `ReadWrite()`); v1 has no subdirectory
  narrowing, because a narrowed subdirectory would have to be resolved and pinned under a
  host root at narrowing time;
- `net` is covered by `r`'s rules exactly as [`net.narrow`](core-library.md#network)
  covers rules;
- each `process` rule of `keep` names a program of a rule of `r`, with an `env` list that
  is a subset of that rule's.

This is the run configuration's grant subset relation, applied to the members a runner
carries. Callees receive no `Env`, `Args` or `Unsafe`, and a runner's grants cannot name
further runners (no nested runners in v1).

`runner.only` restricts which definitions the runner will run to the listed full versioned
definition paths. When `r` already has an allowlist, every listed path must be on it;
otherwise `NotGranted()`. An empty list permits no definition. Narrowing is recorded like
`net.narrow`: a request (the runner handle and the requested data) and a completion (the
new handle or the error). A narrowed runner sent to a remote place travels as the
requested grant set and allowlist inside each call: a claim the receiver checks against its
own grants ([RUN-8](#run-8-identity-and-arrival)), never authority on the wire.

```text
GrantSet    = { stdout: bool, stderr: bool, tasks: bool, monotonic_clock: bool,
                wall_clock: bool, random: bool, dirs: List[DirGrant], net: List[NetRule],
                process: List[ProcessRule] }
DirGrant    = { path: String, access: GrantAccess }
ProcessRule = { program: String, env: List[String] }
GrantAccess = Read() | ReadWrite()
```

These are root core data types with the `copyable`, `data`, `key`, `inert` and
`serializable` bounds; lists are sorted and unique as in the run configuration, and an
unsorted or duplicated `keep` returns `NotGranted()`.

### RUN-3: Entries

`Entry[F]` is a root core type: a reference to one top-level function definition whose
callable type is `F`. It is plain, `copyable`, `inert` and `sendable`; it is not `data`,
`key` or `serializable` ([SER-1](serialization.md#ser-1-serializable-schemas)), and it
cannot be called from source: calling it locally would duplicate a direct call by name,
and `local.threads@1` is the local path.

Where the expected type of an expression is `Entry[F]`, a path naming a top-level function
definition forms an entry of that definition, as a named definition forms a callback where
an `FnPtr` is expected ([PROG-4](programs.md#prog-4-interop)). The definition must be
non-generic, not `extern`, not `@incomplete`, and visible at the use
([PROG-2](programs.md#prog-2-visibility)); its declared callable type, with every parameter
and result mode, relation, allocation contract and `uses`, must equal `F`, so
`fn(a: A)` and `fn(take a: A)` are different entry types. A closure, a local function, a
parameter or local of callable type, a generic or extern definition, or a definition of a
different type in that position is RUN-3 at the expression. A value already of type
`Entry[F]` passes as any value does, so entries flow through ordinary library code (retry
helpers, choreographies) without losing their identity. Generic entries wait until code
hashes cover instantiations.

```hirsel
@serializable
type Order { id: u64, cents: u64 }

fn price(o: Order) -> Result[u64, String] {
  Ok(o.cents)
}

fn quote(r: Runner, o: Order) -> Result[u64, RunError[String]] {
  runner.compute(r, price, o)          // price forms Entry[fn(o: Order) -> Result[u64, String]]
}
```

Writing `runner.compute(r, fn(o: Order) -> Result[u64, String] { Ok(o.cents) }, o)` is
RUN-3 at the closure. An entry's value names the definition, never an implementation: a
call through it runs the revision the receiving place selects for that definition, and the
receiver checks that it is the caller's ([RUN-8](#run-8-identity-and-arrival)).

```text
fn runner.entry_path[F](f: Entry[F]) -> String
```

`runner.entry_path` returns the definition's full versioned path, the spelling `runner.only`
takes.

### RUN-4: Calls

A runner call has a form fixed by the callee's shape. Authority enters a callee only
through a capability parameter, which comes first; a callee with none is pure. In each
form, `A`, `R` and `E` are `serializable` ([RUN-5](#run-5-placement-independence)).

```text
fn runner.compute[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.call[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.spawn[A: serializable, R: serializable, E: serializable](s: Scope, r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> Task[Result[R, RunError[E]]] of s
```

- **Compute.** The callee takes only inert data, so it is pure
  ([CAP-2](capabilities.md#cap-2-charged-at-use)), and its retry class can be read off its
  signature ([RUN-7](#run-7-retry-classes)). Every provider accepts this form.
- **Call.** The callee is entry-shaped: its only authority is the `Io` the provider
  projects from the runner's grant set, exactly as a launch projects `Io` for `main`
  ([PROG-3](programs.md#prog-3-entry)). Each `io.*` projection the grant set lacks returns
  `NotGranted()` inside the callee.
- **Transact (Reserved).** A transaction body `fn(tx: Tx, call: CallInfo, a: A) ->
  Result[R, E]` run inside a store at the place ([STORE-12](storage.md#store-12-transactions-at-a-runner)).
  Its outcome type is `Result[R, RunError[TxFailure[E]]]`: the body's error arrives as
  `Failed(Aborted(e))` and a storage outcome as another `Failed(..)`; `TxFailure.Canceled`
  never occurs inside `Failed`, because `RunError` reports cancellation itself.

`a` is borrowed and encoded once, at the call; the callee receives fresh owned data decoded
from those bytes, and the caller's `a` is unchanged. The result is fresh owned, unique
data. `runner.call` and `runner.compute` are blocking `Io` calls: the calling task
suspends until the outcome, and other tasks run.

`runner.spawn` encodes `a` before it returns, so `A` need not be sendable, then starts a
child task of `s` that performs `runner.call` with those bytes. Its handle obeys
[CONC-2](concurrency.md#conc-2-tasks) like any `task.async` handle. Compute calls spawn
through `task.async`.

**Check order.** Each call performs its effect-free checks in this order, then checks
cancellation ([CONC-3](concurrency.md#conc-3-cancellation)), and only then sends:

1. Argument validity: the argument encodes within the provider's request budget, else
   `NotRun(Rejected())`.
2. Authority at the caller: the provider accepts the call form and the definition is on
   the runner's allowlist, else `NotRun(Denied())`.
3. Cancellation: a cancelled scope returns `Canceled(cause)`, with no request sent.

A request is **sent** at the provider's commit point, which each provider contract names:
the moment after which the callee may run. Before it every outcome proves no effect.

### RUN-5: Placement independence

`A`, `R` and `E` must be `serializable` for every call form and every provider, local ones
included; a schema that does not qualify is [SER-1](serialization.md#ser-1-serializable-schemas)
at the call. Choosing a different provider in the run configuration therefore never
changes what the checker accepts or what a program means
([DET-3](determinism.md#det-3-no-switches)). Every provider passes the argument and the
result through the canonical codec. A provider in the caller's process may skip encoding
only where the skipped path is observably identical: the callee receives fresh storage
sharing nothing with the caller, every NaN is canonical, every `Set` is in canonical order,
and a value the codec's budgets would refuse is refused the same way, as
`NotRun(Rejected())`. v1 providers always encode.

### RUN-6: Outcomes

```text
RunError[E]   = NotRun(NotRunReason) | InDoubt(InDoubtReason) | Failed(E)
              | Faulted(Failure) | Canceled(CancelCause)
NotRunReason  = Denied() | VersionMismatch() | Unreachable() | Overloaded() | Rejected()
InDoubtReason = Lost() | TimedOut() | Abandoned()
```

Root core types, variant ordinals in the order written, `serializable` when `E` is. A
call ends in exactly one outcome:

- `Ok(r)`: the callee ran and returned `Ok(r)`.
- `Failed(e)`: the callee ran and returned `Error(e)`.
- `Faulted(f)`: the callee's run ended in the language failure `f`
  ([FAIL-1](failure.md#fail-1-kinds)), or exceeded a budget, which arrives as a failure
  with a budget cause. The callee's effects up to the failure happened. A provider may
  strip `site` and `cause` detail from `f` by administrator policy; it never invents one.
  A runner call is a failure boundary: the callee's failure never unwinds the caller
  ([FAIL-2](failure.md#fail-2-boundary)).
- `NotRun(reason)`: provably no effect. `Denied` (a grant, allowlist, call-form, admission
  or stale-epoch refusal), `VersionMismatch` (the place does not hold the caller's code,
  [RUN-8](#run-8-identity-and-arrival)), `Unreachable` (the provider could not deliver the
  request: no connection, no process), `Overloaded` (the place refused for capacity) and
  `Rejected` (a malformed or over-budget request, or a stream position that is out of
  order or reused with a different request digest).
- `InDoubt(reason)`: the request was sent and no outcome arrived. `Lost` (the connection
  or child process was lost), `TimedOut` (a bound in the provider's own configuration
  expired) and `Abandoned` (the place ended the callee without an outcome: environmental
  termination, a fenced store, or a callee Blocked outside this process). The callee may
  or may not have run, wholly or in part. A timeout that proves the request was never
  accepted is `NotRun(Unreachable())` or `NotRun(Overloaded())` instead.
- `Canceled(cause)`: no effect. Either the scope was cancelled before the request was
  sent, or, after it was sent, the provider proved that the callee never started: it
  acknowledged a forwarded cancellation before starting it ([RUN-9](#run-9-cancellation)).
  A sent call whose callee may have started never ends in `Canceled`.

`InDoubt` is a live provider outcome, recorded as a completion like any other and replayed
as one. It is distinct from the journal's InDoubt disposition
([recording and resuming](determinism.md#recording-and-resuming)), which describes a
request whose completion was never recorded. Each provider is a versioned contract that
names which variants it can produce ([providers](#providers)); a provider that returns a
variant outside its contract violates its contract, and a simulated provider produces only
the variants of the contract it models. The reason sets are part of each type: adding a
reason is a breaking change for exhaustive matches.

```hirsel
fn bill_once(r: Runner, req: Order) -> Result[u64, String] {
  match runner.compute(r, price, req) {
    Ok(cents) => Ok(cents),
    Error(NotRun(_)) => Error("not run; safe to call again"),
    Error(InDoubt(_)) => Error("may have run; reconcile before calling again"),
    Error(Failed(e)) => Error(e),
    Error(Faulted(_)) => Error("the callee failed"),
    Error(Canceled(_)) => Error("canceled before it ran"),
  }
}
```

### RUN-7: Retry classes

Whether a call may be repeated follows from its form, never from a flag or annotation:

- **Pure (compute).** The outcome is a function of the code identity and the argument
  bytes. A provider may run the callee more than once, hedge it, or return an outcome
  cached under (code hash, argument digest), when its configuration enables it; the caller
  observes one outcome. Compute calls are never journaled
  ([RUN-14](#run-14-recording-levels)), and recovery re-issues an unresolved one.
- **Transactional (transact, Reserved).** Each caller sends its transact calls to a place
  on an ordered **call stream** ([STORE-12](storage.md#store-12-transactions-at-a-runner)).
  The place commits each request's outcome and the stream's new applied position in the
  request's own transaction, so re-sending a request at the same stream position with the
  same request digest returns the recorded outcome instead of running again, and the call
  is exactly once.
- **Effectful (call).** At most once. After the request is sent, no provider re-sends it,
  and a lost outcome is `InDoubt`.

Any provider may re-send a request whose previous attempt ended in `NotRun`, within limits
its configuration states. Every other retry is program code or administrator policy;
nothing retries by default, and each attempt a program makes is its own recorded call.

### RUN-8: Identity and arrival

Every request carries the callee's identity: its full versioned definition path, its
signature fingerprint ([PS-6](proof-seam.md#ps-6-obligation-checksum)) and its **code
hash**, the name-independent definition hash of the callee
([LANG-6](language.md#lang-6-signatures)): a hash of its signature and body with every
reference written as the referenced definition's hash, so renaming a definition or a
local changes no code hash and any change to the callee or anything it reaches does.

Before the callee runs, the receiving place checks, in this order, and stops at the first
failure:

1. The request frame decodes and is within the place's budgets, else `NotRun(Rejected())`.
2. The caller is admitted at this place (transport identity, admission window), else
   `NotRun(Denied())`.
3. The place holds code for the path whose code hash and signature fingerprint equal the
   request's: its own revision when that has the path at that hash, else the verified
   closure from its code store ([RUN-13](#run-13-provider-seams)). Otherwise, or when the
   code store fails, `NotRun(VersionMismatch())`, and the place reports the missing
   hashes. It never runs code whose recomputed hash differs from the request's.
4. Authority: the place accepts the call form; the requested grant set is covered by the
   place's grant set for this caller ([RUN-2](#run-2-narrowing) coverage); the definition
   is on both allowlists; and the place's code policy admits the callee: under
   `admit.safe@1` ([CAP-6](capabilities.md#cap-6-safe-code)) a callee that reaches an
   extern or `Unsafe` operation is refused. Else `NotRun(Denied())`.
5. For the transact form, the stream position
   ([STORE-12](storage.md#store-12-transactions-at-a-runner)): an applied position with the
   same request digest returns its recorded outcome without running; any other position
   that is not the next one, or a stale caller epoch, is `NotRun(Rejected())` or
   `NotRun(Denied())`.
6. Capacity, else `NotRun(Overloaded())`.
7. A cancellation already forwarded for the request: `Canceled(cause)`.
8. The argument decodes against the callee's parameter schema
   ([SER-2](serialization.md#ser-2-canonical-codec)), else `NotRun(Rejected())`.

Authority is never resolved from the definition's name. The callee's capabilities come
only from the place's grant set, narrowed by the request; the request's grant set can only
reduce them. A signature states only the callee's root capability kind (`Io`, `Tx` or
none), never which projections it will use, so a projection the place lacks fails inside
the callee with `NotGranted()`, exactly as for `main`; a finer static check waits for
per-capability footprints in signatures, which are reserved. A place in the caller's own
process and revision passes checks 2, 3 and 8 by construction and still performs 4 to 7.

### RUN-9: Cancellation

Cancellation before the commit point is CONC-3 cancellation: the call returns
`Canceled(cause)` and sends nothing. Once the request is sent, the call is committed and
is not a cancellation point. Cancelling its scope (or the spawned task) forwards a cancel
to the provider and waits for its acknowledgement; no runner call ever leaves its callee
running unobserved. The provider cancels the callee run's root scope with cause `Parent`.
The call then ends in:

- `Canceled(cause)`, when the provider acknowledges that the callee had not started;
- the callee's own outcome (`Ok`, `Failed`, `Faulted`), when it completes, which a callee
  observing its cancellation usually makes `Failed` with its own `Canceled` error;
- `InDoubt(..)`, when the provider loses the place before it acknowledges.

A provider bounds the wait for an acknowledgement by its own configuration and reports
`InDoubt(TimedOut())` when it expires; a scope deadline
([CONC-3](concurrency.md#conc-3-cancellation)) cancels a call this way and never abandons
it. A place that loses its connection to a caller cancels that caller's running callees.

### RUN-10: Callee runs

Every call that passes arrival runs the callee as its own run, with its own validated run
record, admitted under the place's grants. Its record states:

- `program`: the place's semantic program revision; `entry`: the callee's path; `launch`:
  `call`;
- `call`: the form; the **cause**, which is the caller's run instance, the request ordinal
  and the request digest ([RUN-11](#run-11-linked-tapes)); and the exact argument bytes;
- `io`: for the call form, the grant set the request selected, with the place's providers;
  absent for compute;
- `inputs`: for an in-process or simulated place, each seed is the first eight bytes,
  little-endian, of `H("hirsel.runner.seed.v1", call key, seed name)`, where `H` is the
  domain-separated, length-framed SHA-256 the run record identity uses; a remote place
  chooses its seeds and records them. Derived seeds are deterministic and predictable, as
  `random.splitmix64@1` already is;
- `recording`: the level the runner entry or the host selects
  ([RUN-14](#run-14-recording-levels)).

The callee run's outcome is its result value, `Error` value or failure, recorded in its
trailer when it records. A callee run is isolated from its caller: a failure, Blocked state
or budget refusal in it ends only it, and the caller receives the matching outcome.

### RUN-11: Linked tapes

The caller's **run instance** is a 128-bit value fixed at admission, drawn from the host's
secure generator (or derived for a callee run as its seeds are), and recorded in the
validated record's header. It distinguishes two runs of one run record: without it, two
launches of one record would share call keys and call streams, and the second run's
requests would be answered with the first run's outcomes. A call's **request ordinal** is
its run-local request ordinal, the `Prepared` id under the durable level. Its **request
digest** is `H("hirsel.runner.request.v1", form, path, signature fingerprint, code hash,
requested grant set, allowlist, argument bytes)`, with the grant set and allowlist in
canonical codec bytes. Its **call key** is `H("hirsel.runner.key.v1", run instance,
request ordinal)`.

At the `record` and `durable` levels a call is recorded at the caller as one request and
one completion, like a network operation ([network recording](determinism.md#network-recording)):
the request holds the runner handle, the form, the callee identity, the requested grant
set and allowlist, the call key, the stream position for transact, and the argument
bytes; the completion holds the outcome, with the exact result or error bytes for `Ok`,
`Failed` and `Faulted`, and the callee run instance when the callee ran. A call that parks
records `blocked` and `ready` as a network request does, and a forwarded cancellation and
its acknowledgement are recorded events. Replay returns the recorded outcome without
contacting any place, and refuses a request that differs from the recorded one in any
byte.

Caller and callee tapes are **linked**: the callee's cause names the caller's run
instance, ordinal and request digest. A joined check over a set of tapes requires, for
every link, that the callee's argument bytes and digest equal the caller's request and,
when the caller's completion carries an outcome from the callee, that its bytes equal the
callee trailer's outcome; any mismatch refuses the set. Any one tape replays alone; any
subset replays with its internal links checked; no Lamport or vector stamp is recorded.
A link to a run recorded at `none` cannot be checked and is reported as unchecked. Tapes
hold the argument and result bytes, so they are as secret-bearing as those values.

**Reconciliation.** A crash between a durable `Prepared` and its `Completed` leaves the
request in doubt for recovery. For the call form, a host whose configuration declares
`reconcile` records each call key durably before admitting it and answers a status query
by call key with the outcome, "running" or "unknown", and "unknown" there proves the
request never ran. That is the provider recovery procedure
[recording and resuming](determinism.md#recording-and-resuming) allows; nothing else
resolves an in-doubt effectful call. For the transact form, recovery raises the caller's
stream epoch at the place and reads the stream's applied position
([STORE-12](storage.md#store-12-transactions-at-a-runner)). Compute calls are re-issued
(RUN-7).

### RUN-12: Quiescence across runners

Callee runs on in-process providers (`local.threads@1`, `sim@1`) belong to the caller's
execution for [CONC-8](concurrency.md#conc-8-global-quiescence): the execution is Blocked
when no task of any of its runs can make progress, so a call cycle that waits on itself
across runners is reported as Blocked, with each wait row also naming its run's cause
chain. An outstanding request to an out-of-process place is an external event of unknown
completion and prevents that conclusion. A callee that is Blocked at an out-of-process
place ends there, and its caller receives `InDoubt(Abandoned())`.

### RUN-13: Provider seams

Every costly or policy-bearing mechanism of runners and stores is a **seam**: a versioned
provider contract chosen and tuned in the run configuration by the host, never fixed in
the language or the program. A seam's choice is validated at admission and bound into the
validated record. It never changes what the checker accepts or what any outcome means
([DET-3](determinism.md#det-3-no-switches)); it changes only cost, latency, crash
survival, and which outcomes of the fixed types can occur. Defaults are the cheap choice,
and expensive guarantees are opted into. Each provider defines its own settings and their
defaults as part of its contract; there is no general setting, such as a durability level,
that every provider of a seam must offer. Every seam ships a default implementation and a
simulated one whose decisions are recorded provider outcomes, so a simulated run replays
from its tape without the simulator. A host may opt out of a seam's optional guarantee
only through a named implementation whose contract says so.

| Seam | Chosen by | Default | Simulated |
|---|---|---|---|
| Placement and transport | runner `provider` | `local.threads@1` | `sim@1` |
| Recording level | run and runner `recording` ([RUN-14](#run-14-recording-levels)) | the host's | the same levels |
| Re-sending `NotRun` requests | runner `retry` | `{"attempts": 0}` | the same |
| Compute caching | runner `cache` | `null` (none) | the same |
| Code store, fetch and signatures | runner and host `code` | `code.none@1` | `code.sim@1` |
| Limits (queues, request bytes, per-call budgets) | provider settings | each provider's stated defaults | the same |
| Scheduling | `inputs.scheduler` | the cooperative FIFO | seeded schedules |
| Execution width | `placement.cores` | 1 | simulators run on one thread |
| Fault injection | `faults` | none | `sim@1`, the simulated `Dir` |
| Store medium | the `dirs` mount a store is opened over ([STORE-11](storage.md#store-11-engine-and-storage-seam)) | a host directory | a simulated `Dir` |
| Store settings | the store provider's own configuration ([STORE-11](storage.md#store-11-engine-and-storage-seam)) | the provider's defaults | the same |

**Code store.** A code store holds immutable objects keyed by definition hash
([LANG-6](language.md#lang-6-signatures)). An object, `hirsel.code.object.v1`, is a
definition's hash preimage: the content texts of its whole recursive group, each member's
group references and the member's index, so a store holds groups whole. A store recomputes
the hash on every add and every read and trusts nothing else in an object; dependencies are
read from the texts. Its one write primitive is all-or-nothing
`add(bundle) -> Ok | Missing(hashes)`: it adds a bundle only when every dependency is
stored or in the bundle, and otherwise adds nothing and names exactly the dependencies that
are neither; an object already stored is success. Writes are create-if-absent, signatures
first, then code objects with dependencies before dependents, so an interrupted add never
leaves an object without its closure.

The providers are `code.none@1` (the default: only the place's own revision; it refuses
adds), `code.dir@1` (`{"path"}`), `code.object@1` (`{"objects": entry}`, an entry of the
host's object-storage seam, such as `objects.dir@1` or `objects.sim@1`) and `code.sim@1`
(`{"seed", "fail_one_in"}`). Each takes an optional `signatures` member: `sign.none@1`
(the default) or `sign.ed25519@1` with `{"keys"}`, a sorted, unique, nonempty list of
64-digit hexadecimal public keys. A signature covers `"hirsel.code.signature.v1\0"`
followed by the hash, is stored as `sig/<hash>/<key>`, and is checked again on every read.
A place never runs fetched code whose recomputed hash differs.

### RUN-14: Recording levels

Each run records at one of three levels, chosen by its run record's `recording` member;
when the member is absent, the host's launcher chooses, and the validated record states
the choice (callee runs: in their runner entry, or by the host). The record member binds
the level into the run identity, which is what makes `durable` admissible and
replay-checkable; launcher flags are shortcuts that write it.

- `none`: no tape. The run cannot be replayed; its outcomes and meaning are unchanged.
- `record`: a tape of every capability input and completion, written without durability
  barriers; after a crash it has an unknown suffix.
- `durable`: the journal of [recording and resuming](determinism.md#recording-and-resuming).
  Each effect's `Prepared` record is durable before the effect is issued and its
  `Completed` record before the outcome reaches the program. Several `Prepared` records
  may share one durability barrier (group commit), and every effect waits for the barrier
  that covers its record. Compute calls are recorded but never journaled, since recovery
  re-issues them (RUN-7).

The level of one run never constrains another's: a caller at `durable` may call a runner
whose callees run at `none`.

## Providers

A runner's provider is a versioned observable contract, identical for real hosts,
simulation and replay ([providers](determinism.md#run-configuration)). Runners are listed
in the `io` record under `runners`, sorted and unique by `name`. Each entry has `name`,
`provider`, the provider's settings, the seam members `recording` (absent: the host's
level), `retry` (default `{"attempts": 0}`), `cache` (default `null`) and `code` (default
`{"provider": "code.none@1"}`), `admission` (default `null`: the place admits any checked
code; `{"code": "admit.safe@1"}` refuses at arrival a callee that reaches an extern or
`Unsafe` operation, with `NotRun(Denied())`, as the run record's `admission.code` does for
a launch), and, where the provider runs Hirsel code with authority,
`grants` (the same closed schema as `io`, without `runners`) and `definitions` (`null`
for every definition, or a sorted unique list of full versioned paths). Examples are shown
as grants-template input; the canonical record spells every absent member as `null`,
except run-policy members, and has no whitespace.

| Provider | Place | Forms | Outcomes it can produce |
|---|---|---|---|
| `local.threads@1` (default) | separate runs in this process | compute, call | `Ok`, `Failed`, `Faulted`, `NotRun(Denied, Overloaded, Rejected)`, `Canceled` |
| `local.process@1` | a child process per call | compute, call | all except `NotRun(VersionMismatch)` |
| `sim@1` (simulated) | a node of this process's deterministic simulation | as the modeled contract | exactly the modeled contract's |
| `remote.tls@1` | another host, over TLS | compute, call | all |
| `db@1` (Reserved) | inside a store's writer | compute, transact | `Ok`, `Failed`, `Faulted`, `NotRun(Denied, Overloaded, Rejected)`, `InDoubt(Abandoned)`, `Canceled` |

**`local.threads@1`** runs each callee as a separate run with its own heap on a pool of
`threads` operating-system threads (default 1), admitting at most `queue` calls waiting
for a thread (default 64); a call beyond that is `NotRun(Overloaded())`. Its commit point
is admission to the queue. It shares the caller's revision and process, so it never
produces `VersionMismatch`, `Unreachable` or `InDoubt`; a crash of the process ends caller
and callees together.

```json
{"name": "pool", "provider": "local.threads@1", "threads": 4, "queue": 64,
 "recording": "record",
 "grants": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1",
            "dirs": [{"path": "cache", "host": "/var/cache/app", "access": "read_write"}]},
 "definitions": null}
```

**`local.process@1`** runs each callee in a child process launched from the same semantic
program revision, through the standalone launch path with the callee's explicit run record
and no environment fallbacks. Requests and outcomes travel over a private pipe in the
frames of `remote.tls@1` without TLS. Its commit point is the first byte of the request
written to the child. A child that cannot be started is `NotRun(Unreachable())`; a child
that exits without an outcome after the commit point is `InDoubt(Lost())`, or
`InDoubt(Abandoned())` when it reported its run Blocked or environmentally terminated. v1
starts one process per call; a process may serve later calls only where nothing of an
earlier call is observable.

```json
{"name": "sandbox", "provider": "local.process@1", "processes": 2, "queue": 16,
 "grants": {"tasks": "tasks.cooperative@1",
            "process": {"provider": "process.spawn@1",
                        "rules": [{"program": "/usr/bin/convert", "env": []}]}},
 "definitions": ["media@1/thumbs.render"]}
```

**`sim@1`** runs every callee of every `sim@1` runner of the run as a node of one
deterministic simulation in this process, under the run's seeded scheduler. Each runner
names its `node` and the runner contract it `models`, including `local.threads@1`,
`local.process@1` or `remote.tls@1`, and produces only that contract's outcomes.
Faults are provider outcomes drawn from
`inputs.fault_seed`: each named fault site is enabled once per run with probability
`enable` and then fires per evaluation with probability `fire`, both in parts per million.
A crash's point is the first eight bytes, little-endian, of `H("hirsel.runner.fault.crash.v1",
fault seed, runner name, request ordinal)` modulo 32: the callee stops before that request
to its providers, counted from 0, or, in a simulation fixture whose callees make no
requests, after that many suspensions; a callee that ends first ran to completion and only
its outcome is lost. A stopped callee's tape records the event `stopped` where it stopped,
and its replay stops there with the environmental outcome "its place stopped the run".
The sites are `refuse` (before the commit point: `NotRun(Unreachable())`), `overload`
(`NotRun(Overloaded())`), `lose_request` (after the commit point, before the callee
starts: `InDoubt(Lost())`), `lose_reply` (after the callee completes:
`InDoubt(Lost())`), `crash` (the node stops the callee at its crash point, every write to
its simulated directories that was not yet durable is lost, and the call is
`InDoubt(Abandoned())`, never `Lost`; the node restarts for its next call) and `partition` (calls to
the node are `refuse` until a later poll heals it). Every decision is recorded as the
completion it produced, so a simulated tape replays without the seed. Each node writes its
callees' tapes in the real format, and running the same record twice must produce
byte-identical tapes.

```json
{"name": "billing", "provider": "sim@1", "node": "b", "models": "remote.tls@1",
 "recording": "durable", "retry": {"attempts": 3},
 "grants": {"tasks": "tasks.cooperative@1"}, "definitions": null,
 "faults": {"lose_reply": {"enable": 250000, "fire": 20000},
            "crash": {"enable": 100000, "fire": 5000}}}
```

**`remote.tls@1`** sends calls to a runner host. The `endpoint` is `{"tcp": {"address",
"service"}}` with a canonical address and service as in network grants, or `{"unix":
path}` with a canonical socket path; TLS is required on both, so one authentication story
covers both, with `server_name`, `roots` (`"webpki"` or `{"pem": absolute path}`) and an
optional client `identity` (`{"chain", "key"}` absolute paths), all read and pinned at
admission. Its commit point is the first byte of the request frame written. Frames are a
`u64` length followed by one canonical codec envelope of the core `RunFrame` data type;
each carries the session epoch, and requests carry the call key; a frame for an unknown or
stale session is discarded. Requests carry the requested grant set and allowlist
([RUN-2](#run-2-narrowing)); the host admits them against its own configuration for the
authenticated client and performs every check of [RUN-8](#run-8-identity-and-arrival).
`grants` and `definitions` in the caller's entry are the ceiling the caller requests, not
the host's grants. `timeout_ns`, when present, bounds the wait for an outcome after the
commit point and yields `InDoubt(TimedOut())`. `reconcile: true` declares that the host
records call keys durably before admitting, so status queries can prove a call never ran
([RUN-11](#run-11-linked-tapes)). Recording follows TLS recording: plaintext frames, the
server name and the root-set identity, never key material.

```json
{"name": "billing", "provider": "remote.tls@1",
 "endpoint": {"tcp": {"address": "billing.internal", "service": "7443"}},
 "server_name": "billing.internal", "roots": {"pem": "/etc/hirsel/billing-ca.pem"},
 "identity": {"chain": "/etc/hirsel/client.pem", "key": "/etc/hirsel/client.key"},
 "recording": "durable", "retry": {"attempts": 3},
 "code": {"provider": "code.dir@1", "path": "/var/lib/hirsel/code",
          "signatures": {"provider": "sign.ed25519@1",
                         "keys": ["3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"]}},
 "grants": {"tasks": "tasks.cooperative@1"},
 "definitions": ["billing@3/charges.charge"], "timeout_ns": 30000000000,
 "reconcile": true}
```

`reconnect` (default 1) is how many times the provider reconnects after losing its
connection before it gives up on the calls it holds. Without TLS a runner host is not
reachable; `recording`, `code` and `admission` other than their defaults are refused on a
`remote.tls@1` entry, since the host chooses them for its callees.

A runner host is a launch of the Hirsel runtime whose configuration maps each admitted
client identity to a caller name, a grant set and an allowlist. It must perform the
arrival checks of RUN-8, run every callee as a callee run of RUN-10 admitted under that
mapping, forward cancellations and acknowledge them, cancel a lost caller's callees, and
answer status queries.

**Frames.** Every frame is one value of the root core data type `RunFrame`:

```text
RunFrame      = Open(SessionOpen) | Opened(SessionOpened) | Refused(NotRunReason)
              | Call(RemoteCall) | Outcome(RemoteOutcome) | Cancel(CallRef, CancelCause)
              | Acknowledged(CallRef) | Status(CallRef) | Running(CallRef) | Unknown(CallRef)
              | Close(u64, u64)
SessionOpen   = { stream: Bytes, instance: Bytes, epoch: u64 }
SessionOpened = { epoch: u64, incarnation: Bytes, applied: u64, reconciles: bool }
CallRef       = { epoch: u64, position: u64, key: Bytes }
RemoteCall    = { epoch: u64, position: u64, ordinal: u64, key: Bytes, form: CallForm,
                  path: String, fingerprint: Bytes, code: Bytes, grants: GrantSet,
                  definitions: Option[List[String]], argument: Bytes, acknowledged: u64 }
RemoteOutcome = { epoch: u64, position: u64, key: Bytes, digest: Bytes, end: CallEnd,
                  callee: Option[Bytes], missing: List[Bytes] }
CallForm      = Compute() | Call()
CallEnd       = Returned(Bytes) | Failed(Bytes) | Faulted(Bytes) | NotRun(NotRunReason)
              | InDoubt(InDoubtReason) | Canceled(CancelCause)
```

They are root core types with the `serializable` bound (`CallForm`, a plain enumeration,
has the bounds of `GrantAccess`), declared so that both ends take the frame schema from
their own checked core registry. A frame on the wire is a `u64` little-endian length and the canonical codec
envelope of its value; a frame that does not decode, or is longer than the reader's limit
(the host's `frame_bytes`), ends the connection. The request digest
([RUN-11](#run-11-linked-tapes)) frames the `grants` and `definitions` of a `Call` as
their codec bytes, so caller and host compute it alike.

**Call streams.** Each caller run holds one call stream per `remote.tls@1` runner, named
by `stream`, the hash of the runner's name and the caller's run instance; a stream belongs
to the caller identity that opened it. A session opens with `Open` at an epoch at least the
stream's current one; the host answers `Opened` with its incarnation, the highest decided
position `applied` and whether it `reconciles`, or `Refused(Denied())` and closes for a
stale epoch, another instance or an unadmitted client. A higher epoch fences the older
session. Within an epoch the caller numbers its calls 1, 2, ...; the host decides each
position exactly once and in order, and every decided position, `NotRun` included, keeps
its outcome until the caller acknowledges it (`acknowledged` on a later `Call`, or
`Close`). A `Call` for a decided position with the same digest is answered the stored
outcome without running; with another digest it is `NotRun(Rejected())`; a position past
the next one is refused and the stream refuses until the session reopens. The host decides
the position before the code and authority checks, so a call those refuse still consumes
its position.

The caller's commit point is the `Call` frame written, and it never sends a call twice.
After a lost connection it reconnects up to `reconnect` times; positions the host reports
applied are resolved with `Status` (answered with the stored outcome, `Running` or
`Unknown`), later ones complete `NotRun(Unreachable())`, which `retry` may resend; when
the host is another incarnation that does not reconcile, every sent call is
`InDoubt(Lost())`. A runner entry that claims `reconcile: true` refuses a host that does
not. `Cancel` of a queued call completes it `Canceled(cause)`; of a running call, the host
cancels the callee run's root scope with cause `Parent()` and answers `Acknowledged`; a
lost session cancels its queued calls and forwards cancellation to its running ones.

**Host configuration.** `hirsel serve <program> --host <json>` serves the callee entries
the program's own package defines:

```json
{"listen": {"unix": "/run/hirsel/billing.sock"},
 "identity": {"chain": "/etc/hirsel/host.pem", "key": "/etc/hirsel/host.key"},
 "clients": {"pem": "/etc/hirsel/client-ca.pem"},
 "callers": [{"name": "app", "identity": {"subject": "agent-7.hirsel.test"},
              "grants": {"tasks": "tasks.cooperative@1"}, "definitions": null}],
 "admission": {"code": "admit.safe@1"},
 "journal": {"provider": "journal.dir@1", "path": "/var/lib/hirsel/calls"},
 "recording": "record", "tapes": "/var/lib/hirsel/tapes",
 "runs": 1, "queue": 64, "frame_bytes": 67108864}
```

`listen` is an endpoint as in the runner entry. With `clients` trust anchors the host
requires a client certificate they issued, and a caller's `identity` is
`{"certificate": "<SHA-256 of its DER, hex>"}` or `{"subject": "<DNS name it names>"}`;
with `clients` null, the one caller is `"anonymous"`. A caller's `grants` (the `io`
schema without `stdin` and `runners`) and `definitions` bound what its requests may
select. `journal` is the stream seam: `journal.none@1` (the default; a restarted host is
a new incarnation and does not reconcile) or `journal.dir@1`, an append-only checksummed
file that keeps every stream, admission and decision durably before it is acted on, so a
restarted host reconciles, and a call it admitted but never decided is
`InDoubt(Abandoned())`. `recording` is `none` (the default) or `record`, each callee's
tape written to `tapes` under its call key. `runs` (default 1) bounds callees running at
once and `queue` (default 64) admitted calls waiting, beyond which a call is
`NotRun(Overloaded())`.

**`db@1`** (Reserved) runs compute and transact callees inside the writer of a store
([STORE-12](storage.md#store-12-transactions-at-a-runner)).

## Upgrade trials

`hirsel tapes trial <candidate> --grants <template> [--from <n>] <tape>` issues every
runner call the caller's tape recorded, from request ordinal `n` on, again and in order,
each as a live callee run of the candidate revision: the same entry, form, cause and
argument bytes, admitted under the grant set the template's runner of the request's name
holds, with seeds derived from the call key as for the original. It reports, per call,
whether the candidate's callee has the recorded code (its request digest, recomputed with
the candidate's code hash and signature fingerprint, equals the recorded one), the
recorded outcome and the trial's, rendered from their codec envelopes, and whether the
outcome bytes are equal; a recorded `NotRun`, `InDoubt` or `Canceled` holds no outcome to
compare. It exits 1 when any outcome differs. A trial contacts no other place and never
runs the caller; pointing its runner at a copy of a store's directory (a store's objects
are immutable, so a copy is a consistent fork) trials a change against recorded traffic
without touching the live store.

## Examples

```hirsel
@serializable
type Charge { account: u64, cents: u64 }

@serializable
type ChargeError = Declined() | Ledger(FileError)

pub fn charge(rt: Io, req: Charge) -> Result[u64, ChargeError] {
  let ledger = io.dir(rt, path.of("ledger")) ? Ledger
  let bytes = codec.encode(req)
  file.write_bytes(ledger, path.of("last"), take bytes) ? Ledger
  Ok(req.cents)
}

pub fn bill_both(tasks: Tasks, r: Runner, a: Charge, b: Charge)
  -> Result[(u64, u64), RunError[ChargeError]] {
  task.scope(tasks, fn(s: Scope) {
    let ha = runner.spawn(s, r, charge, a)
    let hb = runner.spawn(s, r, charge, b)
    Ok((task.await(s, take ha)?, task.await(s, take hb)?))
  })
}
```

Under `local.threads@1` the two charges run in parallel in this process; under
`remote.tls@1` they run on the billing host with that host's ledger grant. The program,
what it accepts and its outcomes' types are the same.

## Ledger

```text
// RUN-1..14, CAP-1/2, CONC-2/3/8, DET-1
N fn io.runner(rt: Io, name: String) -> Result[Runner, GrantError]
fn runner.grants(r: Runner) -> GrantSet
fn runner.narrow(r: Runner, keep: GrantSet) -> Result[Runner, GrantError]
fn runner.only(r: Runner, definitions: List[String]) -> Result[Runner, GrantError]
fn runner.entry_path[F](f: Entry[F]) -> String
fn runner.compute[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.call[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.spawn[A: serializable, R: serializable, E: serializable](s: Scope, r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> Task[Result[R, RunError[E]]] of s
fn test.runner(t: Test) -> Runner
```

`test.runner` returns a `sim@1` runner modeling `local.threads@1`, with empty grants and
no faults. A scripted variant that returns chosen outcomes (`InDoubt` on the third call),
mirroring `test.net`, is **Reserved**.
