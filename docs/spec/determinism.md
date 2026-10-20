# Determinism and replay

A program's observable behaviour is a function of its capability inputs and its run
configuration. Every source of nondeterminism (scheduling, clocks, randomness, the file
system, the network, the environment) is a capability, so a run can be recorded and
replayed, or executed under a deterministic simulation of `Io`.

### DET-1: Contract

Given the same inputs from every capability (including the scheduling decisions made
through `Io`: task interleaving, atomic order, mutex acquisition order, the acquisition
order of each `serial` package's foreign gate (S-13), the sole-holder outcome of each
`shared.thaw`, each task's observed cancellation at each check, and each deadline scope's
expiry-versus-exit outcome) and the same run configuration (selected program revision,
entry and launch kind, validated grants and provider contracts, snapshots, independent
seeds and the selected simulation model), a program's observable behaviour is identical.
Nothing else is nondeterministic: there are no observable addresses, pointer order, thread
or task ids, wall time or environment; freeing order, placement and allocator state are
unobservable; float, integer and map-order rules are fixed. Any program can run under a
deterministic simulation of `Io`.

For a closed simulated or replayed execution, the observable outcome distinguishes
completion, language failure and Blocked, after its effect prefix. A Blocked report records
waits by spawn path, source site and deterministic creation ordinal; host identifiers and
elapsed host time are not language observations. Missing replay input is invalid input,
not evidence of program quiescence.

Record and replay reproduce confirmed capability inputs and their specified outcomes; they
do not re-execute external effects to discover missing outcomes. An incomplete recording is
not a complete deterministic input stream. Tooling that continues real execution from a
recording identifies unresolved effects and never silently retries them. A recording binds
its events and observations to the canonical validated run configuration and semantic
program revision before replay. Replay refuses incompatible bindings before entry, consumes
recorded inputs without host effects, and never silently substitutes current host inputs or
grants. Each scheduling or capability input that affects execution is recorded or fixed
deterministically by that bound configuration.

### DET-2: Optimisation

Optimisation changes cost, never observable behaviour: checks (removed only where proven
unable to fail), float rules, evaluation order, resource destruction order and allocation
contracts hold at every optimisation level and in every topology. An implementation may
combine or reorder checks (for example, once per vector chunk) only where the first
failure, its site and every effect before it stay the same.

### DET-3: No switches

No flag, environment variable, build mode or topology changes what the checker accepts or
what a program means.

### DET-4: Outside

Outside the determinism contract: `Unsafe` code, foreign functions and environmental
failures.

## Run configuration

An explicit run record is selected by `program --hirsel-run /absolute/run.json [args...]`
or `hirsel run launch.json --run /absolute/run.json`. Argument snapshots, grants templates,
root admission and denial defaults follow [PROG-3](programs.md#prog-3-entry).
There are no environment-variable fallbacks for roots, seeds or grants, no build-mode
grants and no search of parent directories. The default empty-grant record has all seeds
zero. Process snapshots contribute to the admitted run identity like any other input.

| Field | Meaning |
|---|---|
| `schema` | Run-record format version (1). |
| `program` | Digest of the selected semantic program revision: transitive source and core implementations, selected constant dependencies and the runtime's semantic contract. |
| `entry`, `launch` | The entry's full versioned definition path, and `main` or `test`; planned: `call` for a callee run ([RUN-10](runners.md#run-10-callee-runs)). |
| `call` | Planned. Present exactly when `launch` is `call`: the call form; the cause (the caller's run instance, request ordinal and request digest); and the argument bytes as lowercase hexadecimal. |
| `inputs` | `hash_seed`, `random_seed` and `test_seed`, each an explicit `u64`; and, for a simulation, a separate scheduler algorithm, version and seed; planned: `fault_seed`, the seed of simulated providers' fault decisions. |
| `io` | Absent or null, or a closed grant record: stdout sink, Tasks provider, monotonic clock, wall clock, Random provider, directory mounts, network grant and process grant; planned: runners (`runners`, [providers](runners.md#providers)). Each absent projection is denied. |
| `args` | Absent or null, or the exact ordered UTF-8 snapshot, explicit empty included. |
| `env` | Absent or null, or exact key-to-value entries, with null for a granted key that is unset; denied keys are distinguishable from granted unset keys. |
| `unsafe` | Boolean explicit root grant, false by default. |
| `admission` | Optional launch-only constraints: not-before, expiry, an external revocation reference and the code policy `code`, `admit.any@1` (the default when absent) or `admit.safe@1` ([CAP-6](capabilities.md#cap-6-safe-code)). The resolved decision and its evidence are captured in the validated record. |
| `placement` | Execution placement, outside identity: core width and host placement settings; validated and reported as provenance, then stripped from canonical bytes. |
| `budgets` | Absent (`budgets.none@1`), or the host's resource budgets for the run (**Budgets** below). |
| `recording` | Run-policy member: `none`, `record` or `durable` ([RUN-14](runners.md#run-14-recording-levels), [Recording levels](#recording-levels)); absent, the host's launcher chooses and the validated record states the choice. |

**Directory mounts** are sorted, unique, portable virtual paths (the empty path is the
virtual root), each with an explicit absolute host root designator and `read` or
`read_write` access. Overlapping mounts, duplicates and traversal aliases are rejected
rather than given a precedence. There is no automatic working-directory resolution. Host
resources are opened and pinned before entry; recording their paths does not prove what
they continue to contain. Different mounts may deliberately resolve to the same host
resource, and their effects are still recorded.

**Network grants** name a provider and sorted, unique rules `{address, service,
operation}`: `address` is a canonical host name or IP literal
([CAP-1](capabilities.md#cap-1-authority)), `service` a canonical decimal port `0`–`65535`
without leading zeros, and `operation` one of `connect`, `connect_tls`, `listen`, `udp_bind`,
`udp_peer` and `listen_tls`.
The Unix operations `unix_connect`, `unix_listen` and `unix_listen_remove_stale` take a
canonical socket path as `address` and service `0`; a path never names a TCP or UDP rule.
`net.script@2` peers may name a socket path, as `test.net` peers do, and so may its
listeners, with port 0 and clients from `0.0.0.0` port 0, since Unix peers are unnamed.

**Process grants** name a provider and rules `{program, env}` sorted and unique by
`program`: `program` is a canonical absolute path ([processes](core-library.md#processes)),
and `env` a sorted, unique list of variable names, each nonempty and without `=` or NUL,
that the program's children may receive, passed through from the launcher's environment
or set by a `Command`; `env` may be omitted for none. For example
`{"provider": "process.spawn@1", "rules": [{"program": "/usr/bin/git", "env": ["HOME"]}]}`.

**Providers** name versioned observable contracts, identical for real hosts, simulation
and replay: `stdout.bytes@1`, `random.splitmix64@1`, `random.system@1`,
`clock.monotonic.ns@1`, `clock.wall.unix_ns@1`, `tasks.cooperative@1`, `tasks.free@1`,
`net.stream@1`, `net.script@2`, `process.spawn@1`, `process.script@1`, `budgets.none@1`
and `budgets.limits@2`. Planned: the runner providers `local.threads@1`,
`local.process@1`, `sim@1` and `remote.tls@1` and the code stores `code.none@1`,
`code.dir@1`, `code.object@1` and `code.sim@1`. Each is a seam under
[RUN-13](runners.md#run-13-provider-seams): its choice changes cost, latency and crash
survival, never meaning. `tasks.cooperative@1` supplies structured tasks with cooperative
scheduling at the suspension points of [CONC-1](concurrency.md#conc-1-runtime). Host
readiness waits suspend only their task; the provider waits for external readiness only
when no task is runnable. Cancellation is checked before blocking effects and when an
effect-free wait resumes, with destruction shielded as required by CONC-3. Its
observations are those of one cooperative scheduler at every execution width. An
implementation may run several tasks' code between scheduling points at once on several
threads. Scheduling points, synchronising operations, host requests, `shared.thaw` tests,
failures, stops and budget charges take effect in the order the selected scheduler gives.
Execution width is a placement member and never part of identity.

`tasks.free@1` supplies structured tasks with free scheduling. Suspension at a point
CONC-1 permits is allowed, not required; a task yields at least every 128 operations, and
a spawned child may run at once. A `Shared` release that leaves other holders takes effect
when the releasing task next enters an operation, or at its end; both engines release a
value's roots at its cleanup point (S-14), in DROP-2 order. Runs record the per-object
order of synchronising operations (each `serial` package's foreign gate included), atomic
load values and the scheduling inputs of DET-1 (S-3), keyed by (task spawn path, operation
ordinal). Host requests are ordered per logical host resource in shared and exclusive
epochs, and host handles are named by (creating task path, its handle ordinal). A replay
re-executes the recorded cut, which is causally closed. A record naming both
`tasks.free@1` and `inputs.scheduler` refuses.

`random.splitmix64@1` is a deterministic simulation of randomness
on every host: a draw of `n` bytes takes successive SplitMix64 outputs (state starting at
`inputs.random_seed`, advanced by `0x9e3779b97f4a7c15` per output), each little-endian,
concatenated and cut to `n`, consuming one output per started eight bytes. It is
predictable from the seed and is never cryptographically secure. `random.system@1` reads the host's
cryptographically secure generator (on Linux, `getrandom(2)` without flags, which waits
only once, for the boot-time pool, and is retried on interruption and short reads); any
other failure is `Unavailable()` with no bytes. A deterministic simulation never supplies
it, and its draws are never derived from a seed. `net.stream@1` is the host
network with the [network semantics of the core library](core-library.md#network).
`net.script@2` additionally takes `peers`, an array of scripted peer records in schema
order, and serves the program from them exactly as `test.net` does; `peers` is invalid under
any other provider. In each isolated scripted world (`net.script@2` or `test.net`),
port-0 listens allocate ports from 49152 upward in program order, skipping nonzero
ports reserved by the script or already bound in that world; narrowed Nets share
that sequence, outbound connects do not consume it, exhaustion is `AddrInUse()`,
and the chosen local address is recorded in the listen completion for replay.
A scripted listener with port 0 supplies clients for the first port-0 listen on
its IP, using the assigned port. Peer variants are external snake_case tags: `"plain"`,
`{"certificate": [names]}`, `{"fails": failure}`, for a listener's client only
`{"presents": [[names], [bytes]]}`, or for a peer only
`{"serves": {"identities": [{"chain": [bytes], "key": [bytes]}], "clients": auth}}` for TLS
(a `TlsFailure` tag such as `"unknown_issuer"` or `{"alert_received": 40}`; `auth` is
`{"request": [bytes]}` or `{"require": [bytes]}`, and an `"anonymous"` or absent `clients`
is `Anonymous()`, which canonical bytes omit, so it never changes a record's identity); `"refused"`, `"unanswered"` or
`{"open": [steps]}` for connections; `{"send": [bytes]}`, `{"expect": [bytes]}`, `"stall"`,
`"close"`, `"reset"` and `"truncate"` for steps; bytes are arrays of `u8`. Peer data grants
no authority beyond the explicit rules. An implementation that cannot enforce a requested
rule or provider refuses the launch; it never widens it.
`process.spawn@1` runs host programs with the [process semantics of the core
library](core-library.md#processes). `process.script@1` additionally takes `runs`, an
array of `{program, args, outcome}` script entries, and optionally `children`, an array of
`{program, args, steps}` streaming child entries (absent for none), and serves the program
from them exactly as `test.process` does; both are invalid under any other provider. An outcome is
`{"output": {"status": status, "stdout": [bytes], "stderr": [bytes]}}`, with status
`{"exited": code}` or `{"signaled": number}`, or `{"error": error}`, with error
`"not_granted"`, `"invalid_input"`, `"not_found"`, `"permission_denied"`, `"output_limit"`,
`{"fault": code}`, `"timed_out"` or `"input_closed"`. A step is `{"print": [bytes]}`,
`{"print_err": [bytes]}`, `{"expect": [bytes]}` or `{"exit": status}`. Script entries grant
no authority beyond the explicit rules.

**Budgets.** A host bounds the resources one run may use, for example to host untrusted
code, by choosing a budget implementation in the record; budgets are host policy and grant
nothing. `budgets.none@1`, the default, bounds nothing; its canonical spelling omits the
`budgets` member. `budgets.limits@2`, which retires `budgets.limits@1`, takes `fuel` and
`memory_bytes`, each a positive `u64` or null for unbounded, at least one present, for
example `{"provider": "budgets.limits@2", "fuel": 100000000, "memory_bytes": null}`.
**Fuel** is charged once at every function entry and at least once per iteration of every
loop; each engine places its loop charges (the interpreter at every jump to the same or an
earlier block, native code at the targets of the control-flow graph's feedback edges), so
counts differ between engines. A run whose next charge would exceed `fuel` stops before
it. Native code charges fuel only when compiled with fuel checks (`hirsel build --fuel`,
or `hirsel run --native` with a fuel budget); an executable without them refuses a fuel
budget and costs nothing when it has none. **Memory** is the run's logical storage: for
each live counted block, its size class; for each live large block, its request rounded up
to 4 KiB; for each open region, the canonical segment sequence its own requests would take
from an empty pool. A charge happens when a task allocates and a credit when a logical
lifetime ends, in the order the run's scheduler gives. The first charge, in that order,
that exceeds its budget decides the stop and its kind. The interpreter charges the same
quantities from its value model. A stopped run's outcome is **BudgetExceeded** with the
budget's kind, `fuel` or `memory`: like Blocked, it runs no user code, cancellation or
destruction after the stop, keeps the output it produced, and is a tooling outcome whose
position is comparable only within one engine and compilation ([the reference
interpreter](implementation.md#the-reference-interpreter)). A native executable records
`budget_exceeded <kind>` and exits with status 5. A budget changes no other outcome
([DET-3](#det-3-no-switches)). The budgets are part of the validated record. In ordered
mode a replay applies them again and stops where the recorded run stopped. Under
`tasks.free@1`: Fuel is apportioned to running tasks in chunks, returned when a task stops
running. The run stops when a charge finds no fuel left in its chunk or the run's pool, so
up to one chunk per running task may remain unspent. A replay does not re-enforce budgets;
it stops at the recorded cut. An engine that cannot enforce a requested budget refuses the
launch.

**Canonical form.** The record has one strict canonical representation: schema-order
object members; arrays ordered as specified (`args` order preserved, grant sets sorted by
normalized UTF-8 key); no duplicate or unknown fields; canonical unsigned decimal integers
without leading zeros; canonical JSON strings (escape quote and backslash, encode
U+0000–U+001F as lowercase `\u00xx`, emit other scalar values as UTF-8 without
normalization); explicit `null` for absence, except run-policy members; no whitespace. A
run-policy member (the recording level, budgets, the admission code policy, and any later
member that chooses how a run is executed rather than what it may do) is absent from the
canonical bytes when it equals its default: validation normalizes a default value to
absence, so adding a default policy changes neither record identity nor a recorded tape's
header. A **placement member** (`placement`) chooses where and how fast a run executes and
nothing it observes. Validation checks it, then removes it from the canonical bytes
whatever its value. The validated header states the resolved placement as diagnostic
provenance. Replay ignores it and never opens a path it names. Digests are 64 lowercase
hexadecimal SHA-256 digits; admission timestamps are signed decimal `i64` Unix
nanoseconds, zero spelled `0`. The record's identity hashes these bytes with a
domain-separated, length-prefixed SHA-256. A parser for human input may normalize member
order and whitespace before validation. Hash values bind bytes; they do not authenticate a
grant, establish type identity or replace host authorization.

`placement` accepts only `cores` (default 1: a positive integer, `"auto"`, or
`{"min": m, "max": n}` with 1 ≤ m ≤ n), `cpus` (a cpulist or null), `pin`
(`"none"`, `"compact"`, `"spread"`), `commit` (`"shared"`, `"dedicated"`),
`run_ahead` (0..64), `reorder` (`"auto"`, the default, or `"off"`),
`speculation_bytes` (bytes or null), and `metrics` / `control` (paths or null).
Unknown members refuse. `commit`, `run_ahead`, `reorder` and `speculation_bytes`
are ordered-mode admission policy; E1, the ordered executor, sets the
`run_ahead` default. `reorder` applies to fifo@1 only. `hirsel run --cores N`
overrides `placement.cores` without changing identity. `tasks.free@1` together
with `inputs.scheduler` refuses.

**Program identity.** The semantic program revision is a rooted dependency graph: stable
component names, ordered length-framed content and sorted unique edges identify the
transitive private implementations, constants, core and runtime selection, cycles
included. Missing or conflicting components refuse; unreachable nodes, source positions,
compiler and native artifact choices and instance numbering are excluded.

**Validated records.** The validated record states the effective grants, provider contracts
and normalized snapshots; a recording embeds those canonical bytes and their digest. Its
header also states the recording schema, the deterministic input model, the program
identity and the execution origin; planned: the run instance, a 128-bit value fixed at
admission that distinguishes runs of identical records and names the run in call keys and
call streams ([RUN-11](runners.md#run-11-linked-tapes)). A native artifact digest is diagnostic provenance:
replay agreement across native topologies and targets uses the semantic program digest
rather than identical machine code. Observations carry the same program and run identity.
Public signature digests alone are insufficient: a body change invalidates replay even when
request prefixes match.

**Revision swaps.** Revision selection and any future recorded swap points follow
[PROG-1](programs.md#prog-1-names). A runner's callee run (planned) executes the revision
its place selects for the callee, and the place refuses a call whose code hash differs
from the request's ([RUN-8](runners.md#run-8-identity-and-arrival)); a call never swaps the
caller's revision.

**Admission.** Optional time and revocation checks run once, before entry. The launcher
records the decision and the evidence it used. An accepted grant is an endowment for the
run's lifetime: "expiry" is a latest admission time, not a promise that a long operation
stops at that instant. A replay of accepted admission consults neither today's clock nor a
revocation list. Host permissions can still change later; operation errors then arrive as
recorded capability inputs. A policy that requires continuous expiry or revocation cannot
be expressed as a launch-only grant: the launcher rejects it, or uses an explicitly
specified event-capable provider whose events are recorded and obey cancellation's
no-effect rule. Resuming into real `Io` is a new admission that validates live host grants
again; a recording is never authority to reconnect ([PROG-6](programs.md#prog-6-grant-admission)).

**Seeds.** The hash placement seed affects only placement ([SEM-5](semantics.md#sem-5-maps)).
The Random and Test seeds are separate even when a user chooses equal numbers, and so is
the planned fault seed, which drives only simulated providers' fault decisions, each
recorded as the outcome it produced, so replay never needs it. A scheduler
seed describes simulator policy; the recorded event and scheduling trace is the
authoritative replay input. A user `Hasher` receives only an explicit seed from its caller,
never the hidden placement seed.

**Scheduler.** `inputs.scheduler` is absent or null, or `{"algorithm", "version", "seed"}`
naming one of three contracts, each version 1; any other algorithm or version refuses
the launch as unsupported. It chooses only which runnable task runs at each scheduling
point of `tasks.cooperative@1`, the queue order being that of
[CONC-1](concurrency.md#conc-1-runtime)'s cooperative runtime: a spawned or woken task joins
the back, and a yielding task joins the back before the choice. A scheduling point is each
choice made while at least one task is runnable. No contract changes what a program means
([DET-3](#det-3-no-switches)): every schedule it selects is one a cooperative runtime may
take, and the seed only selects among them. Each draw `w` is the next SplitMix64 output of
the scheduler seed, exactly as `random.splitmix64@1` advances its state.

- `fifo@1`, the default and the meaning of an absent scheduler: the front of the queue
  runs. It draws nothing.
- `random@1`: with `n` runnable tasks, candidate `w mod n` in queue order runs; with one
  runnable task it runs and nothing is drawn.
- `pct@1` (probabilistic concurrency testing, Burckhardt et al., ASPLOS 2010): at
  admission it draws the depth `d = 1 + w mod 3`, the horizon `k = 2^(2 + w mod 11)` and
  `d - 1` change points `c_i = 1 + w mod k` for `i = 1, ..., d - 1`; each task then draws
  its priority `d + (w >> 1)` when it is created, the root task first and every spawned
  child at its spawn. At scheduling point `p` (counted from 1), when `p` equals some `c_i`,
  the task that ran last takes priority `i` (the largest such `i`), and then the runnable
  task with the highest priority runs, the earliest in queue order on a tie. After point
  `k` the front of the queue runs, so every runnable task keeps running.

A choice under `random@1` or `pct@1` among two or more runnable tasks is a scheduling input:
a recording keeps it as a `schedule` event with the candidate count and the chosen index,
and replay takes the recorded choice, refusing a recording whose next event is not a choice
among as many candidates. The recorded schedule, not the seed, is the replay input.

**Snapshots.** `Args` and `Env` are validated UTF-8 launch snapshots. A non-UTF-8 host
argument, key or value refuses the launch with a structured launch-input error naming the
item. A denied `Env` key returns `NotGranted`; a granted unset key returns `Ok(None)`.
Invalid query names are expected `EnvError` values.

## Recording and resuming

Ordinary execution and non-durable recording cannot be resumed. The optional durable
recording profile uses the protocol below; the run-record member `recording`
selects `none`, `record` or `durable` ([RUN-14](runners.md#run-14-recording-levels)).
**InDoubt** is a tooling recovery disposition for a request whose completion was never
recorded, never a `Failure`, `Canceled` or a source `Result` variant. A runner call's
`InDoubt` outcome ([RUN-6](runners.md#run-6-outcomes)) is a different thing: a live
provider outcome, recorded as a completion and replayed like any other.

Bind and durably create the header first. Allocate a run-local request ordinal and append
`Prepared(id, anchor, task-path, request)`; make that record and its log location durable
before issuing the request. After the provider returns, append
`Completed(id, outcome, observed-partial-effect)` and make it durable before exposing the
completion or resume event to the program. Expected errors, deterministic refusals,
cancellation with no effect, failure cleanup operations and ordinary successful effects are
all recorded. Concurrent operations have preparation and completion ordinals plus the
recorded scheduling order; an id is not an exactly-once key at the external service.

Records are framed, with lengths, sequence checks and corruption detection, and a final
complete-run trailer. A durable run has one journal writer. Live continuation requires
exclusive journal ownership and evidence that the previous executor stopped or was fenced;
otherwise continuation is refused. A journal lock alone is not a distributed exactly-once
guarantee. Durable writes include the file and directory synchronization the storage
provider's durability model requires. Prepared work may be batched before its effects are
issued, and completions may share a barrier before their results are delivered; effects are
never issued unlogged in order to batch. A recording failure before preparation prevents
issuance. A recording failure after issuance is environmental and leaves the request in
doubt; it is never returned as `Canceled` or as proof of no effect. A torn completion leaves
its `Prepared` record in doubt; unrecoverable corruption refuses recovery rather than guess
that a log prefix is complete. A crash after a durable preparation but before issuance may
conservatively produce InDoubt although no effect happened. Planned exceptions: a compute
runner call is recorded but not journaled, and recovery re-issues it, because its
signature proves it has no effect ([RUN-7](runners.md#run-7-retry-classes)); and a store
commit's in-doubt object create is resolved by reading the object back
([STORE-10](storage.md#store-10-recording)).

A terminal trailer proves a complete recording. Its absence alone does not make durable
completions doubtful: recovery scans the valid records and distinguishes a completed
prefix, unresolved requests and unclean termination with no pending request. Non-durable
recordings cannot prove the absence of effects beyond their last durable bytes and are
labelled as having an unknown suffix. Completed operations replay only in simulation.
Reconciling InDoubt requires external evidence or an explicit application or provider
recovery procedure; it never defaults to a retry, even for apparently idempotent
operations. Call-key status queries and call streams
([RUN-11](runners.md#run-11-linked-tapes)) are such provider procedures. An explicit live continuation is newly admitted and records its recovery
decision. Generic resurrection of host file descriptors, channel state or arbitrary
foreign resources is not promised; a provider must support reacquisition and reconciliation
or continuation is refused.

### Runner and store recording

Runner calls are recorded as [RUN-11](runners.md#run-11-linked-tapes) specifies:
one request and one completion at the caller, parked and readied under
`tasks.cooperative@1` as network requests are, and replayed without contacting a place.
Stores are Hirsel code over a `Dir`, so their effects are recorded as file
effects ([STORE-10](storage.md#store-10-recording)), and replay never opens a real
directory.

### Recording levels

The run record's run-policy member `recording`
([RUN-14](runners.md#run-14-recording-levels)) selects how a host run is recorded:
`"none"`, `"record"` or `"durable"`. It is absent by default, and absent means the host
decides, as before the member existed: a launch with a recording destination (`--record`)
records at level `record`, and one without records nothing. The level is a host
implementation choice behind one seam: it never changes what a program may do or observe
(DET-3), and only the recording, its cost and its recovery differ. A record that names a
level and a launch that disagrees with it refuse before entry: `record` or `durable` without
a destination, or `none` with one. The destination is host input, never part of the record's
identity, and lies outside every granted mount and output file.

- `none` records nothing.
- `record` writes the ordinary tape: its header before entry, then each completion flushed
  before delivery. It cannot be resumed and is labelled as having an unknown suffix until its
  `complete` line.
- `durable` writes the durable journal below.

Only capability requests reach the journal. A pure call, including every core operation
that issues no request, is never journaled.

### Durable journal

A durable journal starts with the 17 bytes `hirsel-journal 1` and a newline, then a sequence
of records. Each record is framed as a 32-bit little-endian payload length, a 32-bit
little-endian CRC-32C (Castagnoli) of the sequence number, kind and payload, a 64-bit
little-endian sequence number counting records from 0, a kind byte and the payload. Payloads
are UTF-8 text that reuses the tape's request and event lines:

| Kind | Payload |
|---|---|
| 1 Header | the tape header of an empty recording, `complete 0` included: format, run identity, seeds, the bound run record, admission evidence, arguments and the mounts' initial tree |
| 2 Prepared | `id task issue request`: the run-local request ordinal from 0, the issuing task's spawn path (`-` for the root task, else ordinals joined by `.`), the task's issue ordinal (its k-th request from 0) and the tape's request text |
| 3 Event | `id event`, or `- event` for a scheduling input: one tape event line, a completion, a `blocked` park, a `ready` event or a `schedule` choice |
| 4 Withdrawn | `id`: a prepared request ended with no effect, as when its task was cancelled while it waited to be issued |
| 5 Trailer | `count outcome`: the number of events and the run's outcome |

The spawn path and issue ordinal identify a request to an operator and are the same in
both engines, so an interpreted and a native journal of the same run agree. Replay checks
the request itself, not these fields. No source position is recorded.

The header record, the file and its directory entry are durable before entry. Every request
is prepared before it is issued, and its completion is durable before it is delivered.
Completions, parks and ready events follow in the order the run observed them: the tape's
events in the tape's order, so the journal's events replay as a tape.

**Group commit.** A prepared request that may park its task (a stdin read, a timer wait, a
process or child request, or a network request) under cooperative scheduling is held: its
task parks as for a host wait, recorded as an uncommitted `blocked` park, and other tasks
run and prepare theirs. One barrier (a single write and `fdatasync`) makes the batch durable
and releases every held request. A batch closes at the first of: the run would otherwise
wait for the host (no runnable task); 64 records; 1 MiB of record bytes; a scheduling
point 1 ms or more after the batch's first record. A released request is ready at the next
scheduling point after its task parked, and its `ready` event is recorded like any other,
so replay parks and resumes it at the same points. Any other request
is covered by a barrier before it is issued, and a completion's barrier also covers every
record appended before it. An effect is never issued before its `Prepared` record is
durable, and a barrier that fails before issue prevents issuance.

**Recovery.** A reader takes the longest valid prefix. A last record that is short, runs past
the end of the file, fails its checksum or breaks the sequence is a torn tail and is cut; it
was never durable, so nothing was delivered on it. Damage to any earlier record, a second
header, a record after the trailer, or records that disagree (an event of an unprepared
request, a request completed twice) refuse recovery as `Corrupt`. A journal with no durable
header was never entered. The recovered journal reports the replayable events, every
`Prepared` request, the requests in doubt (prepared with neither a completion nor a
withdrawal, in preparation order) and the trailer's outcome when present. A complete journal
(a trailer and nothing in doubt) replays exactly as a tape does, in either engine. An
incomplete one refuses replay and names its requests in doubt. This release offers no live
continuation: continuation is refused, since no provider supports reacquisition.

**Simulated disk.** Every simulated `Dir` and the simulated journal share one crash model
([Durability](core-library.md#durability)): a crash keeps each file's last synchronized bytes and each
directory's last synchronized listing, and the last unsynced write survives as a prefix
chosen by the crash seed when everything before it in its file is durable.

### Mapped file recording

`file.map` is recorded like `file.open`: the Dir and path, and the Mapped handle or the
error. Each `mapped.with` records its view (exactly the bytes the body was lent) and its
check (`Ok(())` or `Truncated()`); the body's own operations are recorded between them.
Replay lends the recorded bytes and returns the recorded check without mapping anything,
so a truncation or in-place change by another process replays exactly. A host run that
records copies each view into the recording; an unrecorded run copies nothing.

### Watch recording

`file.watch` is recorded like `file.open`. Each `watcher.next` on a host Dir records the
event it returned, an input like a recorded read, and a park while none had arrived
records the cooperative `blocked` and `ready` events of any host wait. Replay returns the
recorded events at the recorded polls without watching anything. A `test.dir` tree's
watcher events are derived: replay repeats the `test.dir_event` deliveries, and a
`watcher.next` on such a watcher with nothing queued records nothing. Its task parks and
retries at each poll, in recording and replay alike, until a delivery or a cancellation;
a run in which nothing delivers ends Blocked.

### Network recording

Network operations are recorded at the core API: the request (operation, handles, host,
port, timeout, `max` and every byte written) and the completion (handle, addresses, exactly
the bytes a read delivered, accepted counts, or the error). Elapsed host time is never
recorded: a timeout is the outcome `TimedOut()`, and a read's chunking is the exact count it
delivered. Under concurrency, the order in which network completions resume tasks is part
of the recorded scheduling order. Under `tasks.cooperative@1` the scheduler's only host
inputs are whether an issued request must wait and when a waiting request becomes ready,
and both are recorded, for network requests, stdin reads and timer waits alike. A request
whose task parks completes `blocked`, noting whether it had already made part of its effect,
which keeps it from completing `Canceled`. A scheduling point that finds parked requests
ready records, before re-issuing any of them, one `ready` event per such wait in creation
order, naming the wait by its creation ordinal (waits are numbered in the order their
requests first parked) and the number of scheduling points it waited through since its task
parked or it was last ready; scheduling points that find nothing ready are not recorded.
A request parked on a scripted world, which no host event readies, is ready at the first
scheduling point after another network request completes, and parks again if the script
still holds it. Replay parks a task where its request recorded `blocked`, finds a wait ready exactly at its
recorded scheduling point and never waits for the host, so a recorded schedule replays in either engine. Cancellation is derived from
the schedule and is not recorded.

UDP requests record the socket and Net identities, bind/destination address,
every sent byte, and the receive limit. A receive completion records exactly one
admitted datagram's delivered bytes, sender address and truncation flag, or its
error. Empty datagrams are distinct from absent input; datagram boundaries never
merge. Discarded bytes beyond `max` and datagrams from ungranted senders are not
recorded. Replay re-derives peer authority and reproduces the admitted prefix and
truncation flag without binding or sending on the host. A canceled UDP operation
records no provider request or completion; its outcome follows the schedule after
argument and authority checks. Scripted UDP uses the same recording protocol.

Unix requests record the Net handle and the socket path, and their completions the stream or
listener handle (with the listener's `0.0.0.0:0` local address) or the error, exactly as a
TCP connect or listen. Removing a stale socket file and removing a listener's socket file at
close are host effects of those operations, never separate events; replay neither binds nor
removes anything. Scripted Unix peers use the same recording protocol.

TLS is recorded as plaintext and as the `tls.connect` outcome, never as TLS records. The
request records the server name and the identity of the root set: `webpki:` and the
hex-encoded version of the compiled root store for `WebPki()`, or `pem:` and the SHA-256 of
the original PEM bytes for `Pem(b)`, so replay under a different trust configuration
refuses instead of succeeding; a `tls.connect_as` request also records the SHA-256 of its
identity's certificate-chain PEM bytes, and a `tls.connect_with` request records its
identity's digest when it has one and the ALPN protocols it offers. TLS records, keys,
randomness and verification time are internal to the provider and never recorded. A
`tls.accept` request records the SHA-256 of each identity's certificate-chain PEM bytes, in
order, its client policy with the SHA-256 of the trust set's PEM bytes, and its ALPN list; a
private key, and any digest of one, is never recorded. The completion of every handshake is
its outcome: the negotiated ALPN protocol or none, or the error. Replay refuses a recorded
protocol that the request's own list could not select. Replay re-derives the server's validity from
the material the program supplies again, and a different chain or trust set refuses. A
`tls.peer` request records the stream, and its completion the DER chain it returned, which
replay returns without cryptography. A `tls.negotiated_protocol` request records the stream,
and its completion the protocol, which replay re-derives from the recorded handshake. A
`net.split` request records the stream, and its completion the two half handles, read
first; an operation on a half records the half's handle, never the stream's; a `net.join`
request records both halves, and its completion
the stream or `InvalidInput()`. Replay re-derives each half's stream and direction and
refuses a half operation of the other direction. Recordings are secret-bearing: they hold plaintext
and granted `Env` values.

Replay returns recorded completions without sockets, name resolution or cryptography. Each
request must equal the next recorded request; a mismatch, a missing event and an unconsumed
event are replay refusals. Replay re-derives every grant decision from the bound run
configuration and refuses a recorded completion that contradicts it. A replayed read
returns exactly the recorded bytes, so behaviour that depends on chunk boundaries replays
identically. A crash between a durable `Prepared` and its `Completed` leaves the request
InDoubt; continuing from an InDoubt write never retries it.

### Process recording

A process run is recorded at the call: the request (the `Proc` handle, the command's
program, arguments, working-directory handle or none, exact environment or none and
`own_group` (present only when `true`), every stdin byte, `max_output`, and the deadline's
clock handle and ticks or none) and the completion (the exit status with exactly the
captured stdout and stderr bytes, or the error, `TimedOut()` included). Elapsed time and the
order in which the child's streams were drained are never recorded. A `proc.pipeline` is
recorded the same way: its request holds every stage's command in stage order, and its
completion every stage's status with exactly the captured stdout and shared stderr bytes. Under
`tasks.cooperative@1` a run that parks records `blocked` and a `ready` event like a network
request, and its completion when the poll that finds it ready completes it; a cancelled run
records nothing more, since cancellation is derived from the schedule. A completed
`TimedOut()` run, like a completed timer wait, refuses a later recorded monotonic sample
below its deadline.

A streaming child is recorded at the stream API, as a network stream is: `proc.spawn` (the
`Proc` handle and the command; the `Child` handle or the error), each `child.write` (every byte; success or the error), each read (the stream and
`max`; exactly the bytes it delivered, end of stream or the error), `close_stdin`, `kill`
(the signal), `wait` (the deadline's clock handle and ticks or none; the `ExitStatus` or
the error, `TimedOut()` included) and the close of a dropped `Child`. The bytes `wait`
discards are never recorded. Reads, writes and waits park and are readied as recorded.
A streaming pipeline is recorded the same way: `proc.spawn_pipeline` (the `Proc` handle,
every stage's command in stage order and `max_stderr`; the `ChildGroup` handle or the error),
`group.write`, `group.close_stdin`, `group.read_stdout` and `group.kill` as the Child
operations they are on the group's handle, `proc.wait_group` (the deadline; every stage's
`ExitStatus` with exactly each stage's captured standard-error bytes, or the error) and the
close of a dropped `ChildGroup`.

Replay returns the recorded completions without spawning, reading or writing anything, and
re-derives the program check from the bound run configuration: a recorded completion that
contradicts it is a replay refusal, like any other mismatch. Recordings hold whatever the
child wrote, so they are as secret-bearing as the child's output.

### Random and timer recording

A draw is recorded at the call: the request (the `Random` handle and the count) and the
completion (exactly the drawn bytes, or `Unavailable()`). Replay returns the recorded bytes
without consulting any generator. A `random.system@1` draw is secret material: a tape that
holds one is as secret-bearing as its granted `Env` values, and no tool prints a tape's
drawn bytes. A completed `clock.sleep_until` is recorded with its deadline ticks and its
outcome; replay returns that outcome at once, never waits, and refuses a later recorded
monotonic sample below a completed deadline. A cancelled wait issues no provider request. A
scope deadline is recorded as the timer wait it arms: entering `task.scope_until` issues a
`clock.sleep_until` request for the deadline, which completes at once for a reached
deadline and otherwise parks (`blocked`) under its own wait ordinal; the poll that finds it
ready is a `ready` event, and its completion cancels the scope. A scope that exits first
removes its timer and records nothing more. Each poll looks at the scope deadlines before
the parked tasks, in deadline order with ties in entry order, so replay expires every scope
at its recorded poll.
In deterministic simulation, virtual monotonic time advances to the earliest pending
deadline only when no task is runnable. When several timer waits are ready at one
poll, the scheduler resumes them in deadline order, with ties in suspension order;
this applies to real host and native runs as well as virtual time.

## The Blocked observation

`Blocked { waits }` is separate from language exits and from tooling resource limits. Each
wait records the stable spawn path, the suspended operation's source anchor, the wait kind
and the logical creation ordinals of its target. Reports keep the output and effect prefix.
Wait rows are ordered by spawn path, source anchor, wait kind and target ordinals. Host
identifiers and elapsed host time are excluded. Abandonment runs no speculative drop.

### Host input streams and environment

Stdin bytes and environment values are run inputs under DET-1. Each stdin read
records its handle, maximum and completion (bytes, end of input or error); each
environment read records its exact name and completion. Replay consumes these
inputs without reading a host stream or ambient environment, and independently
checks the admitted stream grants and exact Env name grants. Stderr writes record
the accepted prefix and errors just like stdout, on a distinct stream. Simulation
JSON supplies `stdin` as UTF-8 text or an array of bytes, and `env` as an object
whose exact keys are grants and whose values are UTF-8 strings or null for unset.

### Offline observation views

`hirsel view <recording> --capture <archival-id> -o <view.json>` inspects an
ordinary recording or a durable journal without executing the program or admitting
its grants. `--perfetto` writes a Perfetto-compatible Chrome trace-event JSON file.
The versioned `hirsel-observation-view` v1 document binds the tooling-supplied
capture identity to the SHA-256 of the exact input bytes. Independent captures must
receive distinct archival identities even when their bytes match. Program/config
identity alone is not execution identity. These diagnostic formats do not change
the tape, journal, language, or application-visible inputs.

Events retain zero-based tape order, canonical request/completion text (including
outcomes and faults), origin, and accepted byte counts when the completion proves
them. A blocked write has unknown accepted length; requested bytes are not counted
as successful bytes. Ordinary recordings have no task/issue anchors. Durable events
carry only their recorded Prepared association; scheduling inputs have none. Source
and original time are null unless separately evidenced. Pure calls are not tape
observations. A complete tape is not a complete business trace.

An ordinary recording without its complete trailer is a prefix with an unknown
suffix. The viewer cuts only a final unterminated line, retaining its byte boundary
and marking `torn_tail`; terminated malformed lines, interior damage, bad counts,
and bytes after a complete trailer are refused. A missing header is refused.
Durable inspection uses the journal recovery rules above, retains requests in doubt
and any outcome, and marks completeness and torn-tail status separately. Inspection
never authorizes resumption.

`--source <source-or-manifest> --analysis <analysis-id>` additionally replays a
complete recording against its bound program. Every request must match and every
event must be consumed. Replay uses recorded providers only: it performs no
application host effects or exports. The normalized view stores existing semantic
effect observations, source coordinates, and analysis identity under
`replay_observer`, retaining the original evidence identity. This separately ordered
sequence does not infer joins to tape ordinals or become new production traffic.
A tooling limit or program mismatch refuses enrichment. This is effect observation,
not arbitrary source-local probes or pure-call tracing.

The default timing sidecar path is `<recording>.timing.json`; `--timing <path>`
selects an explicit required sidecar. Missing default sidecars leave time unknown.
The diagnostic `hirsel-observation-timing` v1 JSON format contains `schema`,
`version`, `evidence: {capture, sha256}`, `clock: "host-monotonic-ns"`, and
`samples: [{ordinal, ns}]`. Both numeric fields are unsigned 64-bit integers;
ordinals strictly increase, timestamps never decrease, and each ordinal names an
existing event. Sparse samples leave other timestamps null. Evidence mismatch,
duplicate samples, invalid clocks, and invalid values refuse the sidecar atomically.
Sidecar timestamps are original execution evidence and include runtime and
instrumentation overhead; replay duration is never original latency. Generating
sidecars requires a runtime observer; the offline viewer itself reads no host clock.

Logical Perfetto exports always label the track and category as logical ordinal
order, retaining evidence identity and completeness in metadata. Each event is a
thread-scoped instant with its ordinal as a synthetic coordinate, without duration.
No same-track span nesting, task ancestry, parent/link edges or measured nanoseconds
are invented from request order. Canonical requests can contain secrets: views and
exports need the same archival access controls as their source recordings.

### Separate OTLP export

`python3 tools/obs_export.py prepare <view.json> -o <bundle.json>` converts an
[offline view](#offline-observation-views) without executing its source, opening a
transport or reading a clock. By default every original event becomes an Info log
with canonical request/completion text, accepted byte count, ordinal, origin,
evidence identity and prefix completeness. Replay observer effects are excluded;
analysis provenance remains separate. Missing time remains missing. This is an
explicit export of original evidence, never an automatic consequence of replay.
Apply application redaction/allowlists before capture and protect these archives
like their source recordings.

`--projection <json>` selects explicit typed analysis records instead. The v1
`hirsel-export-projection` object contains `schema`, `version`, exact view
`evidence`, `analysis` identity and `records`. This tooling input does not claim a
structural effect was a business span. Logs carry `kind: "log"`, `ordinal`, `level`,
`message`, optional `trace_id`/`span_id` together and scalar `attributes`. Spans
carry `kind: "span"`, `start`/`end` event ordinals, `name`, both IDs, `span_kind`
(Internal/Server/Client/Producer/Consumer), optional `parent_span_id`, explicit
`links: [{trace_id, span_id}]`, and `status` (Unset/Success/Error, default Unset).
IDs must be nonzero 16/8-byte hex. No outcome is inferred from abandonment or a
missing end. Replay observer records are never production traffic.

Metric records carry `kind` (counter/updowncounter/gauge/histogram), `name`,
explicit ASCII `unit`, `start`/`end` ordinals and label `attributes`. Number points
carry integer `value`; histograms carry sorted unique nonnegative integer `bounds`,
`bucket_counts` including overflow, and nonnegative integer `sum`. Counts sum with
checked u64 arithmetic. Histograms are cumulative and bounds include equality;
counters/up-down counters are cumulative sums with monotonicity true/false.
OTLP's signed `asInt` rejects counter values above i64::MAX; histogram sums/bounds
above 2^53 are refused to avoid loss in double fields. Units are preserved without
implicit conversions; measured durations use `ns`. Incompatible kinds/units for
one scope/name are refused. Tooling never adds ordinal/request/span IDs to metric
labels. Derive exact request counts from complete measurements, not sampled spans;
prefixes retain incompleteness. Attributes are bounded string/bool/i64 scalars;
`hirsel.*` keys are reserved. The fixture projection is a complete input example.

Spans and metric points require original timing at both ordinals and explicit
`--correlation <json>`. The v1 `hirsel-original-clock-correlation` object contains
`schema`, `version`, exact `evidence`, `monotonic_ns`, `unix_ns`, and nonempty
`provenance` describing the original execution's clock correspondence. It requires
a view with validated host-monotonic sidecar samples. The original Unix timestamp
is `unix_ns + sample_ns - monotonic_ns`, checked as nonzero u64. A monotonic sidecar
alone has no Unix epoch. Correlations must be measured during original execution,
not fabricated from the exporter's current time. The tool consumes supplied
archive evidence; it does not measure or authenticate this correspondence.
Runtime/instrumentation overhead remains included. Projection/correlation digests
are attached as provenance. Untimed logs omit Timestamp; preparation never invents
collector ObservedTimestamp. Ordinals and replay duration never become source time.

`python3 tools/obs_export.py send <bundle.json> --endpoint <http(s)-base-url>
--export-id <id> --receipt <new-file>` is a separate host operation with explicit
transport destination and storage grants. It uses OTLP/HTTP JSON `/v1/logs`,
`/v1/traces`, `/v1/metrics`, pinned to opentelemetry-proto
`09769935deb8b2ebcf73ad1e2142b691d17bf70b`. The mapping uses scope
`hirsel.observation` version 1, Debug/Info/Warn/Error severities 5/9/13/17,
lower-camel fields, integer enums, decimal-string 64-bit integers and hex IDs.
No ambient proxies or redirects expand the endpoint grant. Request/response limits
are 64/4 MiB; empty signal envelopes are omitted. `--timeout` (default 10 seconds)
is exporter policy, never application clock authority.

The exclusive-create JSONL receipt (`hirsel-otlp-export`, v1 header) binds export
identity, original evidence, completeness, endpoint, mapping and bundle digest.
Each attempt flushes and syncs intent with a request digest before transport;
results distinguish success, partial success, HTTP errors, malformed responses and
unacknowledged delivery. Interrupted intent remains ambiguous. Existing receipts
are never overwritten. `--attempts` defaults to 1, permits 1..10, and retries only
lost transport acknowledgements and HTTP 429/502/503/504, with Retry-After or
exponential backoff and jitter. Partial success is terminal, even a warning with
zero rejection; rejected subsets are never whole-batch retried. Malformed or
oversized responses are terminal and can conceal acceptance. Acknowledgements do
not provide end-to-end or exactly-once delivery.

Explicit re-export adds `--reexport-of <previous-export-id>` with a new export
identity and receipt. Payload resource attributes and receipt mark provenance
while preserving original capture identity. Lost ack retries use identical bytes
and may duplicate accepted data; receipts mark that ambiguity. Operator re-export
can duplicate data too. Replay never invokes the exporter or reconnects to a
collector; source has no global exporter or exporter-controlled enabled branch.

### Object-store grants

A run record or grants template may declare a sorted object `object_stores` of
named S3-compatible endpoints. Omission and the empty object normalize to no member.
Names are lowercase ASCII letters and digits starting with a letter. Each entry has
`endpoint` (an HTTP or HTTPS origin URL, canonical host and nonzero port, with no
userinfo, query or fragment), `bucket`, `region`, `prefix` (canonical relative path,
default empty), `cache_bytes` (u64, default zero), and `credentials` with `mount`
(the virtual path of an explicitly granted Dir) and `path` (a canonical relative file
within that mount). Unknown fields, duplicate names, inline secrets and references
outside a granted mount refuse before entry. File access follows ordinary pinned
Dir confinement, including physical symlink checks; validation reads no credential file.

```json
{"object_stores":{"data":{"endpoint":"https://objects.example",
 "bucket":"hirsel-data","region":"auto","prefix":"service/db","cache_bytes":0,
 "credentials":{"mount":"credentials","path":"data.json"}}},
 "io":{"dirs":[{"path":"credentials","host":"/srv/hirsel/credentials","access":"read"}]}}
```

Normalization projects each declaration into the existing grants: its endpoint's
exact Net rule (`connect_tls` for HTTPS, `connect` for HTTP), a wall clock for SigV4
(default `clock.wall.unix_ns@1`), the referenced existing directory mount, and Env
snapshots `HIRSEL_S3_<UPPERCASE_NAME>_ENDPOINT`, `_BUCKET`, `_REGION`, `_PREFIX`,
`_CACHE_BYTES`, `_CREDENTIALS_DIR` and `_CREDENTIALS`. The last two name the mount and
relative file, never secret bytes. An explicit Env value that differs from a projection
refuses; equal values normalize idempotently. Multiple stores share only identical Net
rules. The pure Hirsel `s3.from_grants(io, env, name)` reads these explicit snapshots and
opens the JSON credential reference through its granted Dir. No new core capability,
ambient environment lookup or host S3 client is involved. The expanded grants determine
ordinary admission, subset checks, run identity, recording and replay.
