# Deterministic core operations

`Library` implements Operations by exact core@1 definition paths. Unknown paths
return None without touching context. The driver owns declared signatures; this
crate never constructs signatures from executable code. `SimulationConfig`
explicitly supplies root grants, args/env, fake files/directories, clock and queued
faults. RunConfiguration supplies entry, random/hash seed and optional strict
ordered event replay. No host file, network, clock or entropy access occurs.

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
those rights return FileError::NotGranted before faults or filesystem effects. Narrowing never enlarges a grant. Walk order is
UTF-8 byte order. Fault scripts use explicit codes, never OS errno; canceled
operations produce no effect, other output/write faults can commit a prefix.
Bytes are preserved exactly, including UTF-8 stdout without a newline.
Atomic writes append to a fake temporary and commit replaces destination contents.
File narrowing consumes its handle on success and failure.

Integer kernels cover all ten sized types. The library division family rejects
MIN/-1 even for mod; the ordinary `%` kernel retains SEM-1's zero exception.
Float kernels separately round binary32/64 and canonicalize NaNs. The four
bits/from_bits operations preserve all finite IEEE patterns, signed zeros and
infinities. Numeric text and concrete reductions execute ordinary std source;
there is no core formatter, float reduction dispatcher or shared host accumulator.

String views preserve UTF-8 spelling. Grapheme segmentation and NFC normalization
execute ordinary std@1 source over vendored Unicode16 tables. Their official
oracles live in hirsel-driver/tests/stdlib_graphemes.rs and stdlib_nfc.rs.
Clock sampling and Instant access execute the split core library contracts. Only
random_u64/env_get remain unpublished typed simulation kernels.

Collections retain insertion/replacement order, semantic copies and IEEE data
equality. Updates use OperationResult::Complete, so the scheduler publishes only to
explicit Mut operand places. Hash-table traversal never determines iteration. Scalar conversion, SipHash-1-3 and pinned Unicode algorithms live in hirsel-kernels; runtime and interpreter retain independent value/collection/Io semantics.
The linear map representation is the reference oracle; keyed SipHash placement
is backend-owned and cannot affect results.
`map.insert`, `set.insert`, `map.get` and `set.contains` select their entry
through one lookup (`collection::find`), which first walks the key through the
structural depth limit as a hash would: a key past it is MEM-3 Depth whatever
the container holds. `map.get` returns a semantic copy of the stored value.

Slots use generation-checked entries. Removal increments the generation; wrapping
into reserved zero retires the vacancy. Insertion reuses an earlier vacancy before
appending, and stale ids return None after removal and reuse.

Task scopes, cooperative children, joins, cancellation and isolation execute through
`tasks.rs` and the evaluator scheduler. `channels.rs` owns moved FIFO buffers,
independent duplicated sender endpoints and suspended mutable receiver loans.
Dropping the last receiver schedules buffered finalizers through the evaluator.
`synchronization.rs` runs mutex bodies synchronously, publishes mutable payload
updates on every exit, kills failed mutexes with their original cause, and stores
sequentially consistent scalar atomics. A payload with observable destruction makes
its mutex noncopyable through the recursive Copyable bound; its single owner drops
the payload through the evaluator. Copyable payload snapshots may remain in the
library until the run ends, as with the reference model's other allocation costs.
Shared, bounded arenas, regions and slots still report Error::Unavailable. Property traversal and test helpers execute ordinary
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
Read and read_into share a canonical byte-delivery event, and explicit/implicit
close share the same handle event. Listener execution remains M2-owned.
