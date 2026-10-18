# Determinism and replay

A program's observable behaviour is a function of its capability inputs and its run
configuration. Every source of nondeterminism (scheduling, clocks, randomness, the file
system, the network, the environment) is a capability, so a run can be recorded and
replayed, or executed under a deterministic simulation of `Io`.

### DET-1: Contract

Given the same inputs from every capability (including the scheduling decisions made
through `Io`: task interleaving, atomic order, mutex acquisition order) and the same run
configuration (selected program revision, entry and launch kind, validated grants and
provider contracts, snapshots, independent seeds and the selected simulation model), a
program's observable behaviour is identical. Nothing else is nondeterministic: there are no
observable addresses, pointer order, thread or task ids, wall time or environment; freeing
order, placement and allocator state are unobservable; float, integer and map-order rules
are fixed. Any program can run under a deterministic simulation of `Io`.

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

A native program is launched as `program --hirsel-run /absolute/run.json [args...]`; a
source run is `hirsel run launch.json --run /absolute/run.json`. The file selects the run
record. There are no environment-variable fallbacks for roots, seeds or grants, no
build-mode grants and no search of parent directories. For a source run, the program's
`Args` come only from the record. For a standalone executable, arguments following the
record path replace its Args snapshot, including an empty snapshot, before validation and
admission; the record may omit Args. Without a record, `program -- [args...]` explicitly
supplies only Args. These process snapshots contribute to the admitted run identity like
any other input. Without either option the run uses the fully specified empty-grant record
with all seeds zero. An explicit record may grant empty `Args` or empty `Env`, which differs
from omitting the root. A requested but absent root is a launch refusal under
[PROG-3](programs.md#prog-3-entry), not a language failure.

| Field | Meaning |
|---|---|
| `schema` | Run-record format version (1). |
| `program` | Digest of the selected semantic program revision: transitive source and core implementations, selected constant dependencies and the runtime's semantic contract. |
| `entry`, `launch` | The entry's full versioned definition path, and `main` or `test`. |
| `inputs` | `hash_seed`, `random_seed` and `test_seed`, each an explicit `u64`; and, for a simulation, a separate scheduler algorithm, version and seed. |
| `io` | Absent or null, or a closed grant record: stdout sink, Tasks provider, monotonic clock, wall clock, Random provider, directory mounts, network grant and process grant. Each absent projection is denied. |
| `args` | Absent or null, or the exact ordered UTF-8 snapshot, explicit empty included. |
| `env` | Absent or null, or exact key-to-value entries, with null for a granted key that is unset; denied keys are distinguishable from granted unset keys. |
| `unsafe` | Boolean explicit root grant, false by default. |
| `admission` | Optional launch-only constraints: not-before, expiry and an external revocation reference. The resolved decision and its evidence are captured in the validated record. |

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
without leading zeros, and `operation` one of `connect`, `connect_tls` and `listen`.

**Process grants** name a provider and rules `{program, env}` sorted and unique by
`program`: `program` is a canonical absolute path ([processes](core-library.md#processes)),
and `env` a sorted, unique list of variable names, each nonempty and without `=` or NUL,
passed through from the launcher's environment; `env` may be omitted for none. For example
`{"provider": "process.spawn@1", "rules": [{"program": "/usr/bin/git", "env": ["HOME"]}]}`.

**Providers** name versioned observable contracts, identical for real hosts, simulation and
replay: `stdout.bytes@1`, `random.splitmix64@1`, `random.system@1`, `clock.monotonic.ns@1`,
`clock.wall.unix_ns@1`, `tasks.cooperative@1`, `net.stream@1`, `net.script@2`,
`process.spawn@1` and `process.script@1`.
`tasks.cooperative@1` supplies structured tasks with cooperative scheduling at the
suspension points of [CONC-1](concurrency.md#conc-1-runtime). Host readiness waits
suspend only their task; the provider waits for external readiness only when no task
is runnable. Cancellation is checked before blocking effects and when an effect-free
wait resumes, with destruction shielded as required by CONC-3. `random.splitmix64@1` is a deterministic simulation of randomness
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
`{"certificate": [names]}` or `{"fails": failure}` for TLS (a `TlsFailure` tag such as
`"unknown_issuer"` or `{"alert_received": 40}`); `"refused"`, `"unanswered"` or
`{"open": [steps]}` for connections; `{"send": [bytes]}`, `{"expect": [bytes]}`, `"stall"`,
`"close"`, `"reset"` and `"truncate"` for steps; bytes are arrays of `u8`. Peer data grants
no authority beyond the explicit rules. An implementation that cannot enforce a requested
rule or provider refuses the launch; it never widens it.
`process.spawn@1` runs host programs with the [process semantics of the core
library](core-library.md#processes). `process.script@1` additionally takes `runs`, an
array of `{program, args, outcome}` script entries, and serves the program from them
exactly as `test.process` does; `runs` is invalid under any other provider. An outcome is
`{"output": {"status": status, "stdout": [bytes], "stderr": [bytes]}}`, with status
`{"exited": code}` or `{"signaled": number}`, or `{"error": error}`, with error
`"not_granted"`, `"invalid_input"`, `"not_found"`, `"permission_denied"`, `"output_limit"`
or `{"fault": code}`. Script entries grant no authority beyond the explicit rules.

**Canonical form.** The record has one strict canonical representation: schema-order object
members; arrays ordered as specified (`args` order preserved, grant sets sorted by
normalized UTF-8 key); no duplicate or unknown fields; canonical unsigned decimal integers
without leading zeros; canonical JSON strings (escape quote and backslash, encode
U+0000–U+001F as lowercase `\u00xx`, emit other scalar values as UTF-8 without
normalization); explicit `null` for absence; no whitespace. Digests are 64 lowercase
hexadecimal SHA-256 digits; admission timestamps are signed decimal `i64` Unix nanoseconds,
zero spelled `0`. The record's identity hashes these bytes with a domain-separated,
length-prefixed SHA-256. A parser for human input may normalize member order and whitespace
before validation. Hash values bind bytes; they do not authenticate a grant, establish type
identity or replace host authorization.

**Program identity.** The semantic program revision is a rooted dependency graph: stable
component names, ordered length-framed content and sorted unique edges identify the
transitive private implementations, constants, core and runtime selection, cycles
included. Missing or conflicting components refuse; unreachable nodes, source positions,
compiler and native artifact choices and instance numbering are excluded.

**Validated records.** The validated record states the effective grants, provider contracts
and normalized snapshots; a recording embeds those canonical bytes and their digest. Its
header also states the recording schema, the deterministic input model, the program
identity and the execution origin. A native artifact digest is diagnostic provenance:
replay agreement across native topologies and targets uses the semantic program digest
rather than identical machine code. Observations carry the same program and run identity.
Public signature digests alone are insufficient: a body change invalidates replay even when
request prefixes match.

**Revision swaps.** A run executes the program revision selected at launch; no feature
replaces it during a run today. Any future live-swap feature must obey this rule, so that
replay reproduces which revision every call ran. A definition's selected revision may advance only at recorded swap points. A named
function value follows its definition, so later calls use the current revision. A closure
keeps the revision that created it. A run's tape records every swap point. See
[PROG-1](programs.md#prog-1-names).

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
The Random and Test seeds are separate even when a user chooses equal numbers. A scheduler
seed describes simulator policy; the recorded event and scheduling trace is the
authoritative replay input. A user `Hasher` receives only an explicit seed from its caller,
never the hidden placement seed.

**Snapshots.** `Args` and `Env` are validated UTF-8 launch snapshots. A non-UTF-8 host
argument, key or value refuses the launch with a structured launch-input error naming the
item. A denied `Env` key returns `NotGranted`; a granted unset key returns `Ok(None)`.
Invalid query names are expected `EnvError` values.

## Recording and resuming

Ordinary execution and non-durable recording cannot be resumed. The optional durable
recording profile uses the protocol below. **InDoubt** is a tooling recovery disposition,
never a `Failure`, `Canceled` or a source `Result` variant.

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
conservatively produce InDoubt although no effect happened.

A terminal trailer proves a complete recording. Its absence alone does not make durable
completions doubtful: recovery scans the valid records and distinguishes a completed
prefix, unresolved requests and unclean termination with no pending request. Non-durable
recordings cannot prove the absence of effects beyond their last durable bytes and are
labelled as having an unknown suffix. Completed operations replay only in simulation.
Reconciling InDoubt requires external evidence or an explicit application or provider
recovery procedure; it never defaults to a retry, even for apparently idempotent
operations. An explicit live continuation is newly admitted and records its recovery
decision. Generic resurrection of host file descriptors, channel state or arbitrary
foreign resources is not promised; a provider must support reacquisition and reconciliation
or continuation is refused.

### Network recording

Network operations are recorded at the stream API: the request (operation, handles, host,
port, timeout, `max` and every byte written) and the completion (handle, addresses, exactly
the bytes a read delivered, accepted counts, or the error). Elapsed host time is never
recorded: a timeout is the outcome `TimedOut()`, and a read's chunking is the exact count it
delivered. Under concurrency, the order in which network completions resume tasks is part
of the recorded scheduling order. Under `tasks.cooperative@1` the scheduler's only host
inputs are whether an issued request must wait and when a waiting request becomes ready,
and both are recorded, for network requests, stdin reads and timer waits alike. A request
whose task parks completes `blocked`, noting whether it had already made part of its effect,
which keeps it from completing `Canceled`. Each poll that finds a parked request ready is a
`ready` event naming the wait by its creation ordinal (waits are numbered in the order their
requests first parked) and the number of polls that found it not ready since it parked or
was last ready; polls that find nothing are not recorded. Replay parks a task where its
request recorded `blocked`, finds a wait ready exactly at its recorded poll and never waits
for the host, so a recorded schedule replays in either engine. Cancellation is derived from
the schedule and is not recorded.

TLS is recorded as plaintext and as the `tls.connect` outcome, never as TLS records. The
request records the server name and the identity of the root set: `webpki:` and the
hex-encoded version of the compiled root store for `WebPki()`, or `pem:` and the SHA-256 of
the original PEM bytes for `Pem(b)`, so replay under a different trust configuration
refuses instead of succeeding. TLS records, keys, randomness and verification time are
internal to the provider and never recorded. A replayable ciphertext recording would have to
contain the key material, so recording plaintext loses no confidentiality; recordings are
secret-bearing in any case, since their run record holds granted `Env` values.

Replay returns recorded completions without sockets, name resolution or cryptography. Each
request must equal the next recorded request; a mismatch, a missing event and an unconsumed
event are replay refusals. Replay re-derives every grant decision from the bound run
configuration and refuses a recorded completion that contradicts it. A replayed read
returns exactly the recorded bytes, so behaviour that depends on chunk boundaries replays
identically. A crash between a durable `Prepared` and its `Completed` leaves the request
InDoubt; continuing from an InDoubt write never retries it.

### Process recording

A process run is recorded at the call: the request (the `Proc` handle, program, arguments,
every stdin byte, the working-directory handle or none, and `max_output`) and the
completion (the exit status with exactly the captured stdout and stderr bytes, or the
error). Elapsed time and the order in which the child's streams were drained are never
recorded. Replay returns the recorded completion without spawning anything, and re-derives
the program check from the bound run configuration: a recorded completion that contradicts
it is a replay refusal, like any other mismatch. Recordings hold whatever the child wrote,
so they are as secret-bearing as the child's output.

### Random and timer recording

A draw is recorded at the call: the request (the `Random` handle and the count) and the
completion (exactly the drawn bytes, or `Unavailable()`). Replay returns the recorded bytes
without consulting any generator. A `random.system@1` draw is secret material: a tape that
holds one is as secret-bearing as its granted `Env` values, and no tool prints a tape's
drawn bytes. A completed `clock.sleep_until` is recorded with its deadline ticks and its
outcome; replay returns that outcome at once, never waits, and refuses a later recorded
monotonic sample below a completed deadline. A cancelled wait issues no provider request.
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
