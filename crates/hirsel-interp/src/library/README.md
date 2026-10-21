# Deterministic core operations

`Library` implements Operations by exact core@1 definition paths. Unknown paths
return None without touching context. The driver owns declared signatures; this
crate never constructs signatures from executable code. `SimulationConfig`
explicitly supplies root grants, args/env, fake files/directories, clock and queued
faults. RunConfiguration supplies entry, random/hash seed and optional strict
ordered event replay. No host file, network, clock or entropy access occurs.

`library::runners` implements the runner operations. `io.runner` projects a runner
`SimulationConfig.runners` names (`NotGranted()` otherwise); `test.runner` mints a
`sim@1` runner modeling `local.threads@1` with empty grants; `runner.narrow` and
`runner.only` mint narrower runners over the same place (RUN-2 coverage; an unsorted
list refuses). Each call takes the next request ordinal. A call encodes its argument
through the canonical codec (a budget refusal is `NotRun(Rejected())`), checks the
allowlist and, for a runner entry admitting only safe code, that the callee reaches no
extern or `Unsafe` operation (`SimulationConfig.callees`; both `NotRun(Denied())`), then
cancellation (`Canceled(cause)`, nothing sent), then the place: capacity (`threads`
plus `queue`, `NotRun(Overloaded())`) and a `sim@1` node's fault sites, each enabled
once per run and fired per call from `SimulationConfig.fault_seed` by domain-separated
hashes, re-sending a `NotRun` attempt up to the runner's `retry`. A sent call's callee
world is the runner's grant set as root grants, with seeds and run instance derived
from the call key (RUN-10), and its arguments are decoded fresh from the request bytes;
the outcome crosses back the same way. The host adapter reuses the same state: it takes the
sends (`take_runner_sends`), starts each callee as its own admitted run, and completes the
caller's wait with `runner_completed`; `callee_outcome` encodes a callee's end as the bytes its
tape records. `runner.entry_path` spells a FunctionReference's
definition.

`launch` validates a pub monomorphic complete root-capability function returning
the nominal core Result[(),E], validates every grant before entry, and returns
LaunchedExecution. Ok exits 0; Error exits 1 regardless of E tags and emits no
implicit output. Runtime unavailable/provider/tool errors remain distinct. Its
semantic observation projects stdout, outcome and ordered capability events and
excludes evaluator steps/instruction trace. Tooling limits remain #34/#16's
separate outcome; no frame-charge constants are invented.

The fake tree retains relative links and rejects absolute paths, NUL/backslash paths
and traversal above each granted root. `SimulationConfig.mounts` selects named
virtual roots and their read/read_write rights. Missing mounts and writes outside
those rights return FileError::NotGranted before faults or filesystem effects. Narrowing never enlarges a grant:
`dir.read_only` lowers a Dir's access to read, `env.narrow` grants a subset of the
configured Env names (`Grant::Env`; the root grants them all) and `proc.narrow` a Proc of
rules within its parent's, in the same world. Walk order is
UTF-8 byte order. Fault scripts use explicit codes, never OS errno; canceled
operations produce no effect, other output/write faults can commit a prefix.
Bytes are preserved exactly, including UTF-8 stdout without a newline.
Atomic writes append to a fake temporary and commit replaces destination contents.
File narrowing consumes its handle on success and failure.

Integer kernels cover all ten sized types. The library division family rejects
MIN/-1 even for mod; the ordinary `%` kernel retains SEM-1's zero exception.
`integer::arith` gives the named wrapping, saturating and checked add, sub and
mul, and `integer::bits` the bit counts and reversals of the word at its width.
Float kernels separately round binary32/64 and canonicalize NaNs; `float::unary`
is Rust's exact `sqrt`, `floor`, `ceil`, `trunc`, `round` and `abs`. The four
bits/from_bits operations preserve all finite IEEE patterns, signed zeros and
infinities; `f64.from_f32` widens exactly and `f32.from_f64` rounds to nearest,
ties to even. `char.to_u32` and `char.from_u32` convert between `Value::Char` and
u32 scalar values, `None` for surrogates and values above U+10FFFF. Numeric text and concrete reductions execute ordinary std source;
there is no core formatter, float reduction dispatcher or shared host accumulator.

String views preserve UTF-8 spelling. Grapheme segmentation and NFC normalization
execute ordinary std@1 source over vendored Unicode16 tables. Their official
oracles live in hirsel-driver/tests/stdlib_graphemes.rs and stdlib_nfc.rs.
Clock sampling and Instant access execute the split core library contracts.
`clock.sleep_until` runs under virtual time (`timers.rs`): the simulated
monotonic reading starts at the configured value and advances only when no task
is runnable, to the earliest pending deadline; the waits due then resume in the
order they suspended. Pending timers are the simulation's outstanding event
source (`Operations::wait`), so sleepers never make a run Blocked. A suspended
wait records `clock.sleep_until.suspend`, and its completion (Ok, Canceled or an
injected Clock fault) records `clock.sleep_until`; cancellation drops its timer.
Scripted signals (`signals.rs`, `signals.script@1`): `io.signals` projects a root
`Signals` handle when `SimulationConfig::signals` grants one and marks the handlers
installed; `Operations::wait` delivers the script's next arrival before advancing virtual
time. An observed kind becomes pending (one per kind; a second pending Interrupt, an arrival
before the projection or of an unobserved kind is `Error::Signaled`, which ends the run
`Outcome::Signaled`). `signal.wait` checks an empty set, then an unobserved kind, then
cancellation, returns the earliest pending arrival among its kinds, or parks recording
`signal.wait.suspend`; its completion records `signal.wait`. A host adapter supplies a host
arrival through `supply_signal` and completes a parked wait through `wait_signal`.
`random.bytes` draws `random.splitmix64@1`: successive SplitMix64 words of the
run's Random seed, little-endian, the last cut short; a zero count touches
nothing. A host adapter supplies its own provider's draw through `supply_random`.

Collections retain insertion/replacement order, semantic copies and IEEE data
equality. Updates use OperationResult::Complete, so the scheduler publishes only to
explicit Mut operand places. Hash-table traversal never determines iteration. Scalar conversion, SipHash-1-3 and pinned Unicode algorithms live in hirsel-kernels; runtime and interpreter retain independent value/collection/Io semantics.
Map and Set retain an insertion-ordered entry block and a private lazy hash
index beside it. Index traversal never determines iteration. Insertion extends
the index, value-only Map callbacks preserve it, and other writes invalidate it;
a detached copy rebuilds its own index on demand. `map.insert`, `set.insert`,
`map.get` and `set.contains` use the same hashed lookup and equality candidates.
A key past the structural depth limit is MEM-3 Depth whatever the container holds. `map.get` returns a semantic copy of the stored value.

Slots use generation-checked entries. Removal increments the generation; wrapping
into reserved zero retires the vacancy. Insertion reuses an earlier vacancy before
appending, and stale ids return None after removal and reuse.

Task scopes, cooperative children, joins, cancellation and isolation execute through
`tasks.rs` and the evaluator scheduler. `channels.rs` owns moved FIFO buffers,
independent duplicated sender endpoints and suspended mutable receiver loans.
Dropping the last receiver schedules buffered finalizers through the evaluator.
Runnable task selection is an explicit task.resume replay input; child completion
and await completion have separate events. A task set's members are task ids in add order;
children record their completion order, from which `task.await_any` picks, and its choice is an
event of its own. Cancellation applies before blocking
Io and channel effects, while destruction shields cancellation. Blocked network
operations park their typed arguments and mutable loans until ready.
`synchronization.rs` runs mutex bodies synchronously, publishes mutable payload
updates on every exit, kills failed mutexes with their original cause, and stores
sequentially consistent scalar atomics. MutexRoot counts owned semantic copies
separately from evaluator snapshots, like SharedRoot. The last release removes the
cell and schedules any present payload for destruction through the evaluator;
a failed body releases the updated payload before the dead cell is closed.
`copy_to` uses the interpreter's iterative semantic value copy; checked execution
uses the declared destination relation to retain the target arena lifetime.
The evaluator charges a `copy_to` into a bounded arena before it runs (MEM-6).
`shared.freeze` wraps the value in a root sharing one graph and its root count
(`value::SharedRoot`): a snapshot shares the count, only an owned copy adds a root,
a release gives one up and the last release releases the value. `shared.thaw`
returns the value when the count is one and the root otherwise. `shared.with` calls
its body with a borrowed snapshot of the frozen value, adding no root.
Sealed regions still report Error::Unavailable. Property traversal and test helpers execute ordinary
std@1 source; core Test check_case reports its recorded seed and the supplied
case index. Core test.fail records LANG-9 and only fails; std expect_error
transfers the Error payload and destroys an Ok value before test.fail. Pull iterators own state/callbacks and lazily apply adapters;
sorted maps use the supplied dictionary comparator. Library continuations use
Operations::Call/resume on the evaluator frame stack. The handle cleanup hook closes
File and aborts/removes AtomicFile temporaries, recording ordered cleanup events.

`launch_with_runtime` takes nominal facts supplied by the driver from declared type
signatures. `launch` supplies only core Result facts; application ADT constructors
require explicit runtime metadata. It never guesses a type shape from its spelling.

Generated/pulled callback inputs transfer ownership in Call.owned while callback
arguments remain borrowed snapshots. Before mapping a pulled item, Call.updates
publishes the advanced iterator to the saved mut argument; failure and abandonment
restore that updated state rather than resurrecting an already-dropped item.
Tests verify one finalizer after the law/mapper on return, failure and abandonment.
Ordinary emitted File/AtomicFile drops likewise close/abort with ordered events.

Compiler-only `$intrinsic.<protocol>_open/next` operations implement map insertion
order, Set members, sorted-map comparator order, Unicode scalars, scalar positions
through the existing Option layout. Cursor values are opaque borrowed
closure snapshots with explicit ownership masks; dropping one cannot finalize the
source's elements. Their paths and declared signatures are supplied by the driver
collection protocol table and nominal Unicode `loop_source` declarations. They produce no capability events.

The consuming Array/List/Inline protocols instead own their source capture. Each
next removes one element slot before publishing its new cursor state; cursor
cleanup releases only the remaining slots. Normal exhaustion, continue, break,
return and runtime failure exercise the same ownership path.

Native completions explicitly state incoming ownership transfers in consumed and
displaced values in released. The evaluator owns incoming Take values until that
commit, so a failed comparator releases the incoming value before map/caller
cleanup. The library never runs resource finalizers directly.

Io stdout/tasks/net/random acquisitions return Ok(capability) or
Error(NotGranted()). SimulationConfig.projections is independent of its root
grants; an explicit empty set permits Io entry and denies every projection.
A denied acquisition records its normal Result without creating a handle.
The directory acquisition uses FileError::NotGranted for a denied projection.

`net.rs` implements independent scripted-peer worlds for `test.net` and
`SimulationConfig.net` (`net.script@2`). Explicit configuration rules remain the
only root authority; test.net derives its own rules. Narrowed Nets share peer
connection queues but retain their own stream brands. Reads preserve Send chunks,
writes accumulate across Expect steps, and resets persist. Timeouts never read a
clock; unbounded unanswered connects and stalled reads stop as CONC-8 Blocked, retaining
the effect prefix and wait source/target ordinals without running destruction.
Blocking operations consume cancellation only before effects, and partial write
faults preserve the accepted prefix. TLS scripts use certificate names without
crypto; a `Fails(f)` peer fails every handshake with `Tls(f)`, and a `Truncate()`
step makes reads of a secured stream `Tls(Truncated())`; request events retain the verification name and SHA-256 PEM root digest.
`tls.accept` loads its `TlsServer` with the shared `hirsel_run_schema::tls::server` check
and answers from the accepted client's `tls` with `tls::scripted_accept` (SNI selection,
leaf names, client-chain verification); its request event records each chain's and the
trust set's SHA-256, never a key. A verified `Presents` chain is what `tls.peer` returns.
`tls.connect` and `tls.connect_as` answer from the peer's `tls` with `tls::scripted_connect`:
a `Serves` peer's presented chain is what `tls.peer` returns, and its client policy checks
the `tls.connect_as` identity, whose chain digest the request event records.
Read and read_into share a canonical byte-delivery event, and explicit/implicit
close share the same handle event. Scripted listeners (test.net's third argument or
`listeners` of `net.script@2`) accept their clients in list order, then return
ConnectionAborted. A client's Expect is an assertion: a contradicting write, or a close
or drop that leaves it unmet, fails LANG-9 after its Test report on stdout; a drop's
failure reaches the evaluator through `Operations::take_drop_failure`.

`file.rs` implements incremental read/seek/write/append over retained inode
contents. File grants preserve position and read/write mode across narrowing;
create truncates the current inode, while replacement/unlink leaves already-open
handles on their prior inode. Read events record delivered bytes and publish the
caller's list through an explicit mutable writeback.

File path failures use typed portable conditions. `Fault::File` injects the same
conditions, with a committed prefix for write errors; generic Fault/Canceled
scripts remain available. Confinement failures retain the explicit invalid-path
code, while missing entries use NotFound. File-specific faults are rejected by
the stdin, stdout and network script domains.

`type_shape.info` (CT-3) reads its type actual's checked declaration from the same
driver-supplied snapshots and builds core `ShapeInfo` data with canonical type
spellings; the `shape(T)` rvalue it reads is a payload-free token (`Value::Unit`).

`codec.encode` and `codec.decode` prove concrete schemas from driver-supplied checked declarations and convert values iteratively through hirsel-types::wire. All format validation stays in that shared codec; only complete decoded values become evaluator data. Malformed input returns DecodeError, while codec budgets remain operational refusal.

Scripted port-0 listeners bind their client queue to the first ephemeral listen on
that IP. Each world allocates from 49152 upward, skips script-reserved and bound
ports, and shares the sequence through narrowed Nets; independent worlds restart
it. Outbound connects do not consume ports. Listen completions retain the assigned
address for replay.

Filesystem metadata includes low-nine-bit permissions without WallClock authority.
`file.set_permissions` and `dir.remove_all` use the ordinary Dir access check before
fault consumption. The independent model removes sorted descendants in postorder,
unlinks symlinks as entries, preserves opened file contents, and detaches removed
issued directories. Recursive-removal fault prefixes count deleted entries.
`file.copy_file` and `file.list_recursive` execute ordinary std source.
Each `test.dir` tree keeps a durable state (`hirsel_kernels::disk::Durable`, shared with the
runtime's test trees): writes note the last write, `file.sync` and `file.put_new(Durable())`
record the file's bytes and its directory chain, and `test.crash` rebuilds the tree from it
and marks the tree's open File and AtomicFile handles stale.

`map.upsert_with` and `map.update_with` select once through `collection::map_index`.
A hit schedules a mutable value callback with an explicit field writeback,
which preserves its mutations on success, failure and abandonment. A missing
upsert initializes and appends a copied key; a missing update returns false.
Bounded-arena growth and shared-entry copies charge before scheduling.

`file.map` opens a `Mapped` grant over the path's identity like `file.open`.
`mapped.with` records `mapped.view` (the identity's current bytes, zero past the
mapped length), calls its body with that view and, on resume, records
`mapped.check`: `Truncated()` once the identity is shorter than the mapping, sticky,
and then the body's result is released after publication. Both yield like other
capability operations; neither is a cancellation point.

`file.watch` opens a `Watcher` grant over the resolved path (`library/watch.rs`).
`watcher.next` records the next queued event, relative to its Dir, or parks its task
until `test.dir_event` delivers one into its `test.dir` tree or the task is canceled. A
simulated tree derives no events from the program's own effects.
