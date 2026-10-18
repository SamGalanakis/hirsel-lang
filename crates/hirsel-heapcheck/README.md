# hirsel-heapcheck

The independent checked post-RC heap executor.
It executes generated Rc/Reuse/Ready LIR over a ledger of checked abstract
allocations and audits every normal and failure exit. It is a test oracle: no
production crate depends on it.

`program::DEFAULT_INSTRUCTION_LIMIT` is the executor's default step budget.
This is tooling under ratified spec issue #34, never a language outcome; reaching
it reports Unsupported. Numeric source programs require more post-RC steps than
source-interpreter steps. This single global constant is a power of two with
at least four times the largest measured numeric H run, including injected
failure paths. Explicit fixture budgets
retain their meaning; a finite/runaway twin pins exhaustion at budget plus one.

Allowed dependencies: the shared data crates `hirsel-lir`, `hirsel-core-contract`
and `hirsel-syntax`. Never `hirsel-rt` (it must not call the production
allocation, count or drop helpers), `hirsel-interp` or its checked mode,
`hirsel-rc`, `hirsel-llvm`, `hirsel-kernels` or `hirsel-driver`. The
`hirsel-lir` dependency test enforces the layering.

Modules: `ledger` (allocations, extents, resources, tokens, regions, handles,
exemptions, counters, trace), `exec` (the machine), `scalar` (checked integer
and IEEE arithmetic), `observe` (the observation), `program` (link set, harness
inputs, configuration, snapshot format), `build` (fixture layout tables and unit identities; block and side-table
assembly uses `hirsel_lir::testing::UnitBuilder`), `violation`. Tests: `tests/it/heap.rs` (the acceptance fixtures
and their snapshots in `native-tests/heap/`), `tests/it/programs.rs` (fault
campaigns, failure audits, exemptions, closures, tokens, COW, deep release),
`tests/it/vectors.rs`.

`rc_trace` (`exec/vector.rs`) runs an `rc-trace v1` vector from
`native-tests/model/` on this crate's ledger and prints the vector output
format; `tests/it/vectors.rs` requires every vector to print exactly its checked
`.expected`, which `proofs/rc/check-vectors.sh` requires of `lake exe rc-trace`.
Counts, saturation (`count_max = max - 1`), holder cells, dying blocks, raw
tokens, region storage and forget records are the executor's own, and the
independent holder tally is checked after every operation. A model field `k` is the
pointer cell at offset `16k` of a `16 (arity + 1)`-byte payload, so distinct
arities are distinct size classes. A vector of an unknown version is refused.

Invariants:

- Pointer values are checked handles `(allocation, generation, offset)` with an
  interned logical projection path, never addresses. In memory a handle is a cell in pointer-sized bytes; bytes without
  a cell read as a handle are a forged pointer. Freed, raw and uninitialized
  bytes are poisoned with `hirsel_rt_v2`'s poison bytes. Reuse increments the
  generation, so a handle from before the reuse is stale.
- Every SSA value carries holder credits. An owning use consumes one; a value
  that still holds one at frame exit is an omitted decrement. After every step
  each changed counted block's count is compared with its independent physical
  holder tally (value credits, counted cells, harness and forget roots), never
  derived from RC operations. Allocation extents journal cell writes; credit
  writes update the tally at binding/transfer/drop seams. An incoming-cell index
  checks existing pointers when target liveness or generation changes. Candidate
  cells and counts retain the full recount's diagnostic order and messages.
  Independent physical cross-checks cover both tally and incoming-cell index at
  steps 1, 2, 4, 8, ...; every campaign checkpoint; language/injected failures;
  any incremental violation; and terminal audit. They read allocation cells and
  register credits directly, without consuming the fast tally's write journals.
  A retained snapshot proves unchanged tree branches; differences rebuild the
  changed physical holders and incoming positions. Header changes revisit all
  incoming cells, including holders inherited unchanged by a failure fork.
  Tokens, resources, regions, handles and named allocation exemptions likewise
  retain independently derived terminal obligations. A separate physical reverse
  index includes borrowed cells for region escape checks; region exit visits
  incoming references to its own allocations in the original source/offset
  order, rather than scanning unrelated dead storage. Nothing is discharged by
  dropping an audit candidate. Candidate order and violation text remain the
  full recount's. Disagreement is `InternalAudit`, an H defect.
  `Config::full_recount_every_step` or `HIRSEL_HEAPCHECK_FULL_RECOUNT=1` runs
  the old per-step recount as the reference too. `cargo test` runs the existing
  fixtures/campaigns and all rc-trace vectors in both modes and compares the
  complete output, trace, violation and step count. The existing 30k-node
  small-stack stress retains its disabled per-step audit; a 543-node twin audits
  every step in both modes. Five hand-built count corruptions and corrected
  commands are in `exec/holders.rs`: missing dup, extra release, overwritten
  credited cell, silently dropped Held credit, and stale-generation cell.
  Each fails at step 3 with the old violation (`CountMismatch`, or
  `UseAfterFree` for the stale cell). `exec/fork.rs` also plants a leaked holder,
  double release and stale count during injected cleanup, with corrected twins;
  the stale-count control bypasses the fast journal in inherited storage.
- Every access is checked before use: liveness and generation, extent bounds,
  initialized bytes, writes only through unique counted blocks and never into
  published or static storage, borrowed values only while their loan is active
  and its address still resolves to the same storage (a detach changes it).
- A prepared address is used where its preparation located it, as native code
  uses it: `Address` records the place's block handle and offset (or, in frame
  storage, its offset), and a use that resolves elsewhere, after a growth,
  detach or replaced block, is `StaleAddress` (`tests/it/revision7.rs`).
- Non-owning SSA edge arguments transport their provenance without reading it.
  Expiration is still checked at an actual read, call or return; owned edge
  arguments still consume a checked credit. `tests/it/revision3.rs` covers an unused
  expired edge value and its rejected read twin.
- Owning leaves in persistent storage (heap, region, static) never receive an
  uncredited reference to a counted block. Declared uncounted view leaves retain
  their borrow permission. Drop and reset programs that reach
  uninitialized bytes fail; a block freed while it still holds a counted cell
  leaks.
- Resources follow `Live -> Finalizing -> FieldsPending -> Dead`; a second
  finalizer entry, or storage ending before a declared finalizer ran, fails.
  Tokens are linear and install only into the exact `(capacity, alignment,
  heap)` class. Region exit releases the region's outgoing holders and fails
  if any surviving storage references the region. Regions exit innermost
  first. An arena token is a checked cell naming its region; an arena
  construction reads it from its slot, requires a scoped region open, and allocates
  an immortal block the region frees at exit. The distinguished heap token has no
  region and allocates counted heap storage. Deep copies traverse headered arena
  owners as well as heap/static owners and allocate every copied block at the
  construction's destination, preserving sharing within the graph.
  A bounded arena (MEM-6, `ArenaToken` with a budget) keeps its remaining
  logical bytes, the failure count at its token and its own `OutOfSpace`
  record. A construction, closure environment or deep copy into it is charged
  (`hirsel_lir::layout::block_charge`; a copy charges every block once per
  reference, a sequence by its length) before anything is placed or consumed;
  an excess records `OutOfSpace` (rule MEM-6) and takes the instruction's
  failure edge. `Recover` forgets that record and resumes ordinary code only
  when it is the one failure since the token; a flat payload in a bounded arena
  is `Unsupported`.
- Terminal audit is compulsory on success and on language failure: observe the
  result, release harness roots, drain, then require no live heap allocation,
  raw storage, unconsumed token, open resource or handle, or live region.
  An owned entry result becomes a harness holder at return; both holder audits
  count it until it moves into the terminal cleanup drain. This generic-call
  return path is H-only: the driver's native source launcher admits typed
  Io/Args or Test entries returning an inline Result, and the runtime observes
  the typed Result, reports an Error, and consumes that Result before task exit.
  It does not launch arbitrary block-returning functions. Opaque observations
  of variable payloads use their checked allocation extent; terminal cleanup
  still traverses their ordinary drop programs and audits every owner.
  Static data, saturated counts, `forget` subgraphs and harness context
  handles are named exemption records, never a blanket exemption.
- Fault injection: `fault_campaign` fails each deterministic failure-capable
  site of a clean run in turn (checked arithmetic, checks, checked address
  steps, copies, frame admission, finalizer entry) and audits each failure
  path. `Replace` is not one: native code exchanges the contents with
  nothing between the two stores that can fail.
  The base refreshes each prefix's independent proof once before forking,
  retaining discrepancies until the original audit boundary. Allocation tables,
  register storage, holder indexes, paths, retained iterator results and arena
  flags, terminal obligations and event and
  exemption histories use shared immutable snapshots. A fork copies only the
  branches its suffix changes; it neither clones nor drops the historical
  allocation table. Indexed snapshots use radix trees, and path interning uses
  an ordered AVL tree; updates take logarithmic work and never rely on hashes
  for audit equality. Campaign bookkeeping scales with the base and executed
  failure suffixes, rather than their product with dead allocation history.
  Required failure cleanup and observation still execute in full.
  `History::iter` streams shared report prefixes; `History::as_slice` explicitly
  materializes one lazily for consumers that need contiguous storage. Large
  iterator, token and explicit-forget payloads share their immutable contents
  too, including when a neighboring snapshot entry changes.
  `HIRSEL_HEAPCHECK_PROFILE_CAMPAIGN=1` prints clean steps, injected sites,
  allocation entries visited by holder/terminal audits and reference-index
  maintenance, and campaign wall
  time. Instrumentation does not determine any verdict.
- Allocation counters and the trace are instrumentation only; no check reads
  them.

Conventions the executor assumes (each is a reading of the frozen LIR schema; a
producer that differs changes this crate or the producer):

- A counted block's header is ledger metadata; the allocation's bytes are the
  payload, whose layout is the `Block { payload }` layout's payload. A
  block-reference layout's own drop program is `[Release { offset: 0 }]`.
- A `Construct` whose result is a block reference allocates its payload per
  its placement; the site names the payload layout, as C checks. `Box(v)` takes
  one operand, the whole payload; other constructors write the payload's fields.
- Preparing a slot or field-only address is frame arithmetic: liveness is
  checked where the address is used, not where it is prepared.
- Parameter slots are live and initialized at entry under the parameter's name
  as version: a by-address argument aliases the caller's place, a by-value one
  moves into frame storage. A version passed along an edge renames the slot
  content it names (a version join).
- `StorageLive` of live storage that holds nothing ends it first (a loop body's
  slot), and semantic `Drop` of a slot skips fields already moved out. H reads
  the active variant's tag and derives its field mask from its own initialized
  cells, as it does for ordinary records. Explicit Release/reset scans and
  reads still reject uninitialized cells.
- The entry unit of a definition excludes the closure bodies that share its
  path.
- A `Release` or `Reset` mask gates the fields of the released block's
  payload (or of the value itself for an inline layout); for a variant, the
  active variant's fields.
- A variant's tag is read through its `TagForm`. A null-pointer niche's word
  is the dataful variant while it holds a data cell, or after a move took
  exactly that one cell out (the native move leaves the bits, so the variant
  stays readable), and the fieldless variant when its bytes are zero;
  constructing the fieldless variant writes the null word.
- A `SlotKind::Capture(i)` slot names field `i` of the closure environment the
  callee was entered with; a zero-sized environment is no block, so each of its
  (zero-sized) capture slots gets empty frame storage. A `mut` or address argument arrives as a checked
  pointer, borrowed for the callee's lifetime; a pointer-typed address base whose
  layout is not a block layout points at that layout itself.
- A finalizer unit's header names the finalizer's definition and takes its
  borrowed self as its one parameter. A borrowed parameter passed as a value
  is a view in the callee's frame: the caller keeps the resources it shows,
  as when cleanup lowering calls a drop body explicitly. A `Copy` by the Dup
  recipe of a slice (owner word and bounds) duplicates its counted cell, as a
  Fields copy duplicates each of its cells.
- `Store`/`Replace` `version` values are storage versions, never operands; a
  `Read` whose root slot holds a different version is malformed.
- `Compare` runs the interpreter's structural order over Unit, scalar,
  record, variant, `Inline` and Box payload layouts, and String and Bytes
  slices bytewise (a proper prefix first, no depth below the value). A List
  or Array compares its common elements one level below the value, in
  order, then its length, as emission's comparator does; its hash walks the
  elements at that depth too. Maps compare ordered pairs and sets compare structurally sorted elements (spec #93, internal ordering). Flat comparison remains unsupported. `IsUnique` reads the ledger count (finite 1). No
  `hirsel_rt_v2` entry is a LIR call.
- A field of a flat record lies at its cursor: its static offset plus each
  earlier variable field's u64 length prefix value, read where the address
  resolves.
- A `Callee::Bound` call (LIR schema revision 9) runs the named instance with
  its closure value's environment. A generic nested function's closure has no
  code unit (`Val::Callable { code: None, .. }`, a null code cell); an
  indirect call of one is malformed. Units a bound call names are closure
  bodies, never entry candidates.
- A `Packed[T]` value occupies its largest variant in a slot or value image
  (`SizeSpec::storage`), as an ordinary variant does; only its active
  payload's bytes are written. A size that reads its own lengths (variable
  flat) has no such bound. A whole Packed value is read and stored within its
  active extent, then widened to that storage, since an element of a
  sequential sequence (MEM-13) ends at its extent; `Index` into one walks past
  each earlier element's extent, and its construction sizes the block by them.
- Flat values (LIR schema revision 4): a dynamic extent is its size program
  evaluated over the stored bytes, for flat copies (`FlatCopy` and a semantic
  `Copy`: a variable flat block's payload bytes into a new heap block) and for drops. A flat
  construction writes its fields' FlatFormatV0 bytes into a block sized to
  them, a String or Bytes slice behind its length prefix, a variable record's
  block behind its payload extent. A slice's bytes lie at its owner's payload
  from `8 + start`; a String or Bytes literal at a `Slice` layout is a static
  allocation laid out as a byte sequence payload (count, then bytes).
  Regional flat storage has an immortal owner that expires at region exit;
  its views may be copied and released as immortal shares. Headerless uncounted
  storage still cannot be copied as an owner. `FlatView` is a slice image of
  the block holding the encoding, starting at the encoding's offset; `FlatLength` reads the count; `Index` on a flat
  sequence walks to the element's cursor, past the count a language failure.
- LIR schema revision 5: a move leaves its bits behind as a ghost of the
  extent until that range is written again, as native memory does; a reset
  token records the payload bits native memory holds (initialized bytes, else
  ghosts), and a `Reuse` with `kept` fields writes every field and then
  requires each kept field's bits to equal those (`SkippedStore`), so the
  skipped native store is unobservable. `Reset` of a callable resets its
  environment block; `Read Take` through a block value moves a field out of
  it (a write: the block must be unique).
- LIR schema revision 6: `Convert`, `IntArith`, `MulAdd` and the named
  divisions, minimum, maximum and total order execute in `scalar.rs` from
  their definitions (SEM-1, SEM-2, docs/spec/core-library.md), independent of the emitter;
  `hirsel-llvm/tests/scalar_library.rs` compares them with the interpreter's
  kernels at every width and edge.
- LIR schema revision 7: a `Slot` step addresses an element slot below the
  capacity (`OutOfExtent` past it) whose bytes keep their own initialization;
  `SeqSetLen` writes a unique private heap block (`WriteShared`,
  `WritePublished`); `SeqReserve` of one moves the elements bitwise into a
  larger allocation and frees the old one shallowly, and of a shared, published
  or static block (the empty list's) detaches it into an allocation of the
  requested capacity, its counted elements gaining a holder, and gives back the
  place's credit of the original; a length
  must cover exactly the written slots (`UninitializedRead`, `LeakedSlot`);
  `SeqAppend` (revision 19) writes its element's active extent past a unique
  sequential block's used bytes, first moving them into an allocation of at
  least twice the held bytes when they do not fit; `FlatAppend` (revision 22)
  grows the same way and writes its construction's FlatFormatV0 bytes behind
  their length, checked against the record's size program, and a builder
  encodes as its count and used bytes once each element's prefix matches its
  content's extent and the elements fill the used bytes exactly (`Malformed`);
  a builder's `SeqReserve` grows its unique block for `capacity` of its least
  element, keeping the count and used bytes; a builder records its open
  element (revision 24): `FlatField` opens one past the used bytes, grows
  for reservations and pushes keeping its bytes, and a `spare` push past
  the allocation fails (`OutOfExtent`); `FlatClose` builds the element
  around the field's bytes and counts it; any other write of a builder with
  an open element, a second opening and a write of another field fail
  (`Malformed`);
  `FlatAt` checks its offset against the element's cursor (`Malformed`);
  `Recast` back from the opaque source names the layout the block was built
  at (`TypeConfusion`); `Hash` hashes structurally with the same depth bound
  as comparison and is a fault-injection site. A by-address parameter's
  value is a slot version, so a body's version joins execute. `tests/it/revision7.rs`
  holds each check's failing program and clean twin.
- `Encode` and `Decode` (LIR schema revision 29, `exec/codec.rs`) reimplement the
  SER-2 codec from the spec over ledger values, at the offsets of the layout
  records the plan names, never the runtime's codec program. Decoding validates
  the whole input in the reference's detection order before it builds fresh,
  zeroed, counted blocks (a Map or Set with an empty index), so a malformed
  input yields its `DecodeError` and allocates nothing; past the reference's
  value budget the run is `Unsupported`.
- `ReadStatic` allocates its constant's static blocks once (`Owner::Static`,
  immortal, published, each a named exemption) and installs every reference
  cell as an uncounted handle to its target block. A result that refers to
  block 0's payload is that block; any other result is read from block 0.
- `Shared[T]` roots (MEM-11, LIR schema revision 27) are modeled apart from
  ordinary credits. `Freeze` allocates a heap root whose count is
  `Count::Root(n)`, the number of roots, and marks every counted heap block the
  value reaches `Count::Frozen` (static and region blocks and nested roots
  excluded); a block found at another count than one is shared. Only roots dup,
  release and test a root count; a dup, release or write that reaches a frozen
  block is `WriteShared`, and the holder audit holds each frozen block to its
  one parent cell. `Thaw` requires `Root(1)`, restores the graph to
  `Finite(1)`, moves the value out and frees the root; the dying root's
  `DropStep::Thaw` restores the graph before its value is released. A deep
  `UniqueCopy` that reaches a root adds a root per new holder instead of
  copying it, and a root is never a reset donor.
- A borrowed capture is stored in its environment field as the view itself:
  no credit, still a borrow in persistent storage, its resources left to the
  creator. A capture slot must have its environment field's layout.
- `Index` projections are supported on byte `Slice`, `Inline`, `Simd` and flat sequence
  layouts and on sequence payloads (the u64 count, then the elements from
  their alignment). A String or Bytes literal block is such a payload, as
  native literals are. Fixed inline repeats drop high to low. Map/Set construction is an ordinary private record over entries and index sequences. Structure-of-arrays scan and drop walks use stored capacity for column
  placement, with reverse row/field cleanup. SoA construction scatters rows into
  capacity-based columns; element reads gather their fields, and field places
  address their columns directly. Reserve moves every column, transferring
  private holders or duplicating shared ones. Length commits audit every slot.

An operation model may take and return capability handles and plain inline
values: `Arg::Handle` passes a context handle to the entry, and `OpValue` has
`Handle` (an operand, or a plain capability result that is never closed),
`Bytes` (the bytes of a String slice operand), `Elements` (the scalars of an
`Array` or `List` block operand) and `Plain` (an inline result
image, size-checked against the result layout). A `Switch` may scrutinize a
variant value held in a register; its tag is read from the value's image.

A runtime-entered Library finalizer has a Mut self parameter; a Source finalizer
has Borrow self. Both alias the finalized storage. The convention is checked
before entry, and ordinary writable-place checks apply to library mutations.

A `Slot` address has no language-check failure site, like a field address.
Its actual read/write still resolves the path and audits capacity and initialized
bytes. Checked `Index` addresses retain fault-injection sites. The revision-7
controls compare those site counts and verify out-of-capacity Slot rejection.

Iterator closures use the same checked callable value as function closures,
with environment at word zero, code at word one and a zero reserved third word.
Storage reads and writes preserve that layout; indirect steps bind capture slots
to the environment fields and environment drops drain their owned contents.

`exec/tasks.rs` runs structured tasks (CONC-1, CONC-2, FAIL-2) on the runtime scheduler's
schedule (`hirsel-rt/src/tasks.rs`), with one activity stack per task in place of its fiber:

- A switch an operation requests is made once the running frame is back on its stack.
- A task blocked in a task operation leaves its call terminator to run again when it resumes.
- A waiting task parks; parked tasks are polled in park order at every yield and whenever the
  running task stops, and a task due to park after its operation's yield parks at the front.
- A capability operation (`Operations::yielded`) is a scheduling point.
- A canceled, unshielded task's cancellation point (`Operations::canceled`: a stdin read, a print
  or a network request that can wait, as `hirsel-rt/src/host_waits.rs` lists them) completes
  `Canceled` with no effect instead of being called.
- A task running destruction (a drain, a finalizer frame, or an inline drop body between
  `FinalizeBegin` and `FinalizeEnd`) is shielded, as the runtime's finalizer shield makes it:
  no cancellation check (check_canceled, send, receive, spawn) observes its cancellation. The
  shield lives in the task's own stack, so it swaps with the task.
- H grants no clock (`io.monotonic_clock` is `NotGranted()`), so no H run holds a `Deadline`
  and none reaches `task.scope_until`'s `$intrinsic.TaskScopeEnterUntil` or
  `TaskScopeExitUntil`; a hand-built unit calling either is Unsupported. Scope deadlines are
  checked by the interpreter under virtual time and by the native runtime on the host clock.
- When every task waits and none is parked on a source that can complete, the run is Blocked
  (CONC-8): it stops with no cleanup and no terminal audit, and its observation says `blocked`.
- While the scheduler holds a spawned closure it is a harness root. Its environment is
  released after the child's frame returns.
- Each task's failure chain is swapped with it, and merged into the parent's at a join or a
  scope exit.
- The holder audits read every task's stack.

`exec/channels.rs` is the runtime's channel table (CONC-6). Each queued message is a harness
root, a send and a receive move it out of and into the bodies' `Option` slots, and the last
receiver's close drops the messages nobody received, oldest first.

`exec/sync.rs` is the runtime's mutex and atomic cells. An atomic holds its payload's `Some`
image; a mutex's payload carrier is a harness root while no body holds it, and the run's finish
drops what the mutexes still hold, as the runtime releases it at the run's end. A dead mutex
keeps the failure chain of the body that killed it.

`exec/isolate.rs` is `task.isolate` (FAIL-2): the `Isolate` terminator moves the running task's
failures since the body's record count into the caught chain, and the `$intrinsic.Isolate*`
entries read each record's fields, a site's line and column through
`Operations::failure_position` (the driver's model holds the sources).

`exec/views.rs` marshals Unicode cursor operations to `Operations` as slice bytes
and relative positions, retaining the original checked owner and borrow. Open
writes an uncounted view; next installs its exclusive position update and returns
plain Option bits or a SomeSlice range with exactly one credited share. No model
receives raw pointers or calls the runtime's cursor/count helpers. The driver
provides the independent byte-level Unicode model.

Sequence length commits validate the element layout's logical fields, tags and
active payload, leaving ordinary padding unspecified. The hand-built padded-pair
control requires every real field. Copying a FunctionRef with no environment
copies its code with no holder action; captured callables still duplicate their
environment (the SortedMap comparator controls include both and failure audits).

Variable-element and sequential Packed (MEM-13) FlatAt checks use
allocation-owned boundaries computed only from H's checked bytes (length prefixes,
or tag-selected extents from the first element). Mutable allocation access clears them before any possible
write, including writes that retain the allocation and generation; no IR offset
enters the cache. FlatNext reuses H's independently resolved end boundary.
`Counters::flat_prefix_reads` counts real element-prefix and sequential extent
reads; ordinary Index continues to scan. The flat module's control rejects wrong cached offsets and a
stale offset after a same-allocation write, with accepted twins. Long count probes
may disable the optional per-step holder recount while retaining the terminal audit.

Borrowed record fields use their layout's uncounted scan and absent drop, the same predicate as borrowed closure captures. Return provenance names declared Borrow parameters in `AbiResult.borrowed_parameters`; the executor checks it independently and rebinds returned cells to the caller's argument loans. Only covering loans of returned parameters end at return. Sequence and field initialization checks follow logical field bytes, with padding unspecified and unwritten fields rejected.

Inline copies identify owning cells from the layout scan. They duplicate finite
targets and clear source borrow provenance on owning cells even for immortal
targets. A crediting copy whose owning scan cell targets uncounted storage fails
with TypeConfusion; uncounted scan cells keep their original permission.

Shallow detach and deep block copies preserve the full allocation extent's bytes
and initialization, including sequence elements appended into spare capacity.
The original allocation request is not the current initialized extent. Hand-built
revision-7 controls cover reads/copies of plain entries, grown sequences and
nested copies, with unwritten-entry twins in both audit modes.

A callable or iterator without captures stores an initialized null environment word beside its checked code word. Logical initialization checks therefore accept it inside a nested record; environment destruction recognizes that null and releases no credit. An unwritten scalar sibling remains an uninitialized read.
Text operation models return `ByteSequence` for fresh Sequence[u8] storage,
`Slice { argument, start, length }` for a checked input-relative range, and
`Variant { tag, value }` for a one-field inline result arm. H allocates using the
layout table's byte payload, shares borrowed inputs through its own checked copy
and transfers taken input credits once without copying. Invalid argument/range
or variant tags are violations; terminal audits cover all four result forms.
A pointer-free scalar wrapper (StrPos) marshals its scalar bits.

Failure campaigns fork deterministic simulation checkpoints immediately before
failure-capable executor steps. Each fork injects its one site and runs its
failure path and terminal drain; the clean machine advances once. The executed
site delta checks the classifier, and a missed site is an InternalAudit fault.
Terminal returned-value finalizers belong to the same campaign. Allocation IDs,
generations, dirty queues, independent holder accounting, suspended frames and
operation state are copied exactly. Allocation records and byte/initialization buffers use copy on
write; mutations and moves cannot alter another run's state.

Operations defaults to the Host domain, which is never cloned. A deterministic
simulation opts into OperationDomain::Simulation and fork_simulation; clones
must also be simulations. Models without this capability retain re-execution.
Config::reexecute_campaign (HIRSEL_HEAPCHECK_REEXECUTE_CAMPAIGN=1) keeps the replay
reference. Config::compare_campaigns (HIRSEL_HEAPCHECK_COMPARE_CAMPAIGNS=1)
compares complete campaigns, including site indices, every Run and audit result.
The fixture suite compares both campaigns with both audit modes in cargo test.

Operation address operands use checked storage and borrow/exclusive permissions.
Taken operands are destroyed after either completion or failure unless the
semantic model explicitly consumes or transfers them. Closed handles and stale
taken addresses remain invalid. Walk models return semantic iterator items; H
allocates and audits their counted environment and implements its checked step.

`tests/it/moved_variant.rs` runs the regions-on/elision-on Result[String, Bytes]
repro from `native-tests/heap/moved-variant-payload.hir` as captured Rc and Ready
program snapshots, with both audits. Hand-built Rc controls cover both active
ordinals, niche tags, remaining counted siblings and a whole read whose explicit
release still fails with UninitializedScan after a payload move.

Persistent environment masks are ordinary initialized u64 data. Closure
construction sets their bits through the ledger, and a mask clear overwrites
only that word through the same journaled writes. Environment drain follows the
live-bit scan/drop guards and rejects either a set bit over moved cells or a
clear bit over live cells. Source-free controls and both native targets share
`hirsel-llvm/tests/once_capture.rs`.

Revision 13 field flags are checked on every incoming edge against the physical
initialized-byte and resource ledger. A plain part is present when any of its bytes
is initialized. Any other part must be flagged present while an owner remains in its
active storage (a data handle not yet taken, static ones included, a run-context
handle's initialized word, or a resource not yet finalized), and absent once taken
or uninitialized, so taken fields and completed inline finalizers count as absent. A
part that was never taken and holds no owner, an inactive variant's field or an
inner variant whose active case owns nothing, may carry either value: C counts a
variant no test proved as present (a failure edge before the test), and its drop
releases nothing. Static inline indices are decoded from their constant
declarations, not from possibly unavailable SSA runtime values. The producer's moved
masks and C's proof are never consulted. The cleanup join controls exercise both
incremental and per-step recount modes.

Slice indexing resolves the checked owner, accumulated start and selected length
before reading its byte. Slice constants record their byte-sequence payload
layout on the static allocation, so deep uniqueness copies and terminal drops
use the backing payload descriptor, rather than the three-word Slice image.
Driver wave-1 controls pin nested ranges, bounds and counted unique copies.
Resource occurrence identity is an allocation extent and its logical projection
path, independent of byte offsets and layout identity. `ProjectionStep<u64>`
resolves the LIR `ProjectionStep<ValueId>` vocabulary to concrete element ordinals;
Slot and FlatAt canonicalize to Index for identity. Handles carry an interned path,
while extent resource maps key on those logical paths. A layout walk selects every
resource in a moved value, including zero-sized roots and siblings, and re-roots
it in the destination image. Physical byte ranges move only bytes and handle cells.
Defuse marks precisely its parent's occurrence FieldsPending without entering its
finalizer; child paths retain their own obligations. The controls in `tests/it/defuse.rs`
cover parent/child offset collisions, zero-sized siblings, empty moves across calls,
equal indices from different SSA values and duplicate child finalization. Empty
resource occurrences make storage live for stores and replacement even without
initialized bytes. Completeness checks follow the active variant's fields and tags,
including variants whose layout also declares a resource finalizer.

Payload projections independently require `LayoutKind::Box`; each step
checks and dereferences one ledger pointer without adding a logical field.
The Box kind otherwise follows Block's storage and ownership recipes.

Flat construction also encodes ordinary sequences into their declared flat
layout. H reads each element through checked initialized storage at its source
stride, recursively encodes nested sequences, and leaves every source credit
untouched. Static variable flat data remains one immortal pointer-free extent.

Closure live-word initialization uses its logical Field projection, just like
capture initialization. A captured resource at a sibling path cannot make that
scalar field initialized, including when a zero-sized resource shares its byte
offset. `tests/it/capture_mask.rs` covers both extents and holder audits, and retains
a genuine store-over-initialized rejection.

Persistent aggregate stores classify each image cell using the destination's
active scan. Already-borrowed nested leaves remain uncounted views with their
original loan; an owning leaf still requires a holder credit. Source-free
`tests/it/nested_views.rs` controls cover the accepted nested view, a missing-credit
owning destination and an expired source loan under both holder audits.

`FlatCopy` also materializes a prefixed part at its ordinary source layout:
copy the checked initialized pointer-free extent into a fresh flat-record block,
or a counted byte sequence owned by a String/Bytes slice with start zero.
The address's encoding layout and the result layout remain distinct.

A construction flagged `arena_no_drop` schedules no payload cleanup at arena close. H honors that omission without trusting result-placement summaries: freeing its physical cells still rejects a hidden heap holder with `LeakedCredit`. `tests/it/arena.rs` includes ordinary and forged-summary leaks in both holder audit modes.

Registry operands use generic `OpValue::Aggregate` and `Sequence` trees for
nested records, variants, inline arrays and lists; scalar lists retain `Elements`.
The former test.dir-only entry transport is removed. Every nested pointer is
read through checked storage, and only semantic data enters a driver model.
Aggregate/sequence results are materialized under H's ledger. Scalar buffer
updates allocate or replace checked storage, and `TakenHandle` transfers a Take
operand without closing or reopening its identity (TLS upgrade).

A whole-byte Slice constructor can take a Sequence[u8] block as its owner.
H writes its checked handle with the block's owning layout into the owner word,
then audits the Slice's existing counted scan and drop. Other raw scalar pointer
stores retain their existing type checks. The pipeline byte-freeze controls audit
static, shared and grown owners and every injected failure in both placements.

Compiler-local region constructions marked for drop elision omit payload drops, including nested
inline `Uncounted` fields. H still records every actual cell and frees the
region through its ledger: a hidden heap holder fails the count or leaked-credit
audit, and an outside reference fails the region escape audit. The source-free controls
in `tests/it/nested_regions.rs` also check C's independent field and lifetime
proofs, with accepted twins.

The harness's terminal drop can raise a language failure after the entry frame
has gone. Handle-close failures retain the entry definition with no source site,
join the failure observation, and keep their report in the terminal output.
The source-free terminal handle control covers LANG-9 and a successful close
under both holder audit modes.

PS-13 unchecked arithmetic and projection guards do not take language failure
edges. H still audits the domain of unchecked division/shifts and the validity
of unchecked indices/tags; violations are model errors. Signed MIN % -1 remains zero.

The SER-2 executor walks variable flat field offsets, sequential Packed active
extents and SoA columns from the layout table, independently of runtime codec
program words. Reconstruction validates wire data first, then patches private
flat framing and allocates fresh counted storage with exact sequence headers.

Flat sequences of zero-size elements read their stored count independently
of their byte extent and index at zero stride past both header words.

Failure observations retain and compare the original operation site independently
of the primary attribution. Tooling reports render both complete locations.
