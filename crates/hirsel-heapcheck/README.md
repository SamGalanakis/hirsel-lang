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
inputs, configuration, snapshot format), `build` (fixture builders for layout
tables and units), `violation`. Tests: `tests/it/heap.rs` (the acceptance fixtures
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
  Full recounts independently cross-check both tally and incoming-cell index at
  steps 1, 2, 4, 8, ...; language/injected failures; any incremental violation;
  and terminal audit. Disagreement is `InternalAudit`, an H defect.
  `Config::full_recount_every_step` or `HIRSEL_HEAPCHECK_FULL_RECOUNT=1` runs
  the old per-step recount as the reference too. `cargo test` runs the existing
  fixtures/campaigns and all rc-trace vectors in both modes and compares the
  complete output, trace, violation and step count. The existing 30k-node
  small-stack stress retains its disabled per-step audit; a 543-node twin audits
  every step in both modes. Five hand-built count corruptions and corrected
  commands are in `exec/holders.rs`: missing dup, extra release, overwritten
  credited cell, silently dropped Held credit, and stale-generation cell.
  Each fails at step 3 with the old violation (`CountMismatch`, or
  `UseAfterFree` for the stale cell).
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
  construction reads it from its slot, requires the region open, and allocates
  an immortal block the region frees at exit.
- Terminal audit is compulsory on success and on language failure: observe the
  result, release harness roots, drain, then require no live heap allocation,
  raw storage, unconsumed token, open resource or handle, or live region.
  An owned entry result becomes a harness holder at return; both holder audits
  count it until it moves into the terminal cleanup drain. This generic-call
  return path is H-only: the driver's native source launcher admits typed
  Io/Args or Test entries returning an inline Result, and the runtime observes
  those result bytes before task exit. It does not launch arbitrary
  block-returning functions or release an arbitrary returned layout.
  Static data, saturated counts, `forget` subgraphs and harness context
  handles are named exemption records, never a blanket exemption.
- Fault injection: `fault_campaign` fails each deterministic failure-capable
  site of a clean run in turn (checked arithmetic, checks, checked address
  steps, copies, frame admission, finalizer entry) and audits each failure
  path. `Replace` is not one: native code exchanges the contents with
  nothing between the two stores that can fail.
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
  callee was entered with. A `mut` or address argument arrives as a checked
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
- `ReadStatic` allocates its constant's static blocks once (`Owner::Static`,
  immortal, published, each a named exemption) and installs every reference
  cell as an uncounted handle to its target block. A result that refers to
  block 0's payload is that block; any other result is read from block 0.
- A borrowed capture is stored in its environment field as the view itself:
  no credit, still a borrow in persistent storage, its resources left to the
  creator. A capture slot must have its environment field's layout.
- `Index` projections are supported on byte `Slice`, `Inline`, `Simd` and flat sequence
  layouts and on sequence payloads (the u64 count, then the elements from
  their alignment). A String or Bytes literal block is such a payload, as
  native literals are. Fixed inline repeats drop high to low. Map/Set construction is an ordinary private record over entries and index sequences. Structure-of-arrays payloads remain unsupported.

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
initialized-byte and resource ledger. Inactive variant fields are absent;
completed inline finalizers count only remaining owners. Static inline indices
are decoded from their constant declarations, not from possibly unavailable SSA
runtime values. The producer's moved masks and C's proof are never consulted.
The cleanup join controls exercise both incremental and per-step recount modes.

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
