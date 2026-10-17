# hirsel-lir

Native data schemas: the staged LIR family, target
descriptors, the physical layout table, the cross-unit ABI, `hirsel_rt_v2`
declarations, effect flags and deterministic snapshots.
`layout.rs` holds physical arithmetic, `abi.rs` holds physical ABI operations,
and `layout_validate.rs` checks layouts and ABIs independently. The type-to-layout
bridge, instance keys/views and declaration ABI derivation live in
`hirsel-layout`. `validate/` holds the independent stage validators. `hirsel-graph` supplies pure traversal, dominance and loop-forest algorithms;
the validators derive their own normal and failure graph from the IR.
`persistent.rs` is the structure-sharing vector that
dataflow states are built from, here and in `hirsel-rc`: a state costs what it
changes, so an analysis stays near-linear in the size of a function.

Allowed dependencies: the dependency-free `hirsel-graph` utility and shared data crates (`hirsel-syntax`, `hirsel-core-contract`, `hirsel-codec`); no external production
dependency. Never an implementation crate or `hirsel-driver`: the driver will
depend on the native implementation crates, which depend on this crate. Production and build dependencies on hirsel-types, hirsel-tir or hirsel-layout
are forbidden. Test-only: `hirsel-layout`, `hirsel-types`, `hirsel-tir`, `hirsel-diag`,
`serde_json` (`cargo metadata`). The manifest checks against the driver's
registry inventory and interface version live in the driver's
`tests/it/lir_schema.rs`. `tests/it/dependencies.rs` enforces the crate layering for the
whole workspace. Buck2 supplies Cargo metadata generated and freshness-checked
by `tools/buck2/sync.py`, so this test needs no Cargo on a worker.

Invariants:

- Input identity. Native artifacts consume the driver's `ValidatedNativeInput`
  by `InputIdentity { interface_version, digest }`, which the driver computes;
  this crate never redefines `NativeInput`. `INTERFACE_VERSION` must equal
  `QueryEnvironment.interface_version` (a test checks it); a unit naming another
  version is stale. Receipt and object-artifact digests include the input
  identity, so an interface change invalidates them.
- Raw versus validated. `LirUnit<S>`, `LayoutTable`, `RawReceipt` and
  `ObjectArtifact` are plain public data that tests may forge. `ValidatedUnit<S>`
  and `NativeReceipt` have crate-private constructors for the stage validators;
  no other crate can build one, and no snapshot decodes into one. A pass that
  rewrites a validated unit gives up its proof (`into_raw`).
- Stages. `Value | Placed | Rc | Reuse | Ready` are sealed marker types; the
  header stage must equal `S::KIND`, and a snapshot decodes only as its own
  stage. `StageKind::admits` is each stage's opcode vocabulary; validators
  check the rest of the invariant.
- Indices. Every ID is local to its unit's arenas or, for `LayoutId`/`FnAbiId`,
  to the layout table whose digest the unit header names.
  `LirUnit::check_identity` rejects a wrong stage, stale versions or digests,
  and any index or pool range outside its arena. It is referential integrity
  only, never a substitute for a stage validator. LIR holds no `TypeId`; the bridge owns the substitution stores.
- Rows. An instruction row is at most 32 bytes on 64-bit hosts (asserted at
  compile time; currently 28). Variable operands live in typed `PoolRange`s and
  call, construction, closure, constant and failure data in side tables.
- Failure edges are explicit: every potentially failing instruction names a
  `FailureEdge` (cleanup entry, rule, anchor, token-disposal list).
- Effects. A call site carries its callee's declared `Effects`; `effect.rs`
  also names every row's `Opcode`, the vocabulary each stage admits.
- Arenas. `LayoutKind::Arena` and
  `ValueRepr::Arena` are the one-word token. `ArenaOpen` (Value stage,
  `arena.with`'s and `arena.bounded`'s library bodies only, entry block)
  becomes `RegionEnter` at the unit's start plus `ArenaToken` in its place at
  placement; `Fixed(Arena(i))` placement reads the token in
  `tables.arenas[i].slot`.
- Instance keys keep the definition, body index, binders, complete type structure,
  size actuals, declared callable convention and environment layout, and erase
  each actual's relation, brand, uniqueness and sendability at every depth.
  The implementation revision is not identity: it travels as the unit
  header's `implementation_revision`, so a callee body edit changes no
  caller's call or symbol. Equality compares the full encoding; hashing uses
  its digest.
- Snapshots. One canonical S-expression text is the normalized dump, the
  round-trip format and the digest input. It contains no TypeIds, addresses or
  host sizes, and re-encoding a decoded value is byte-identical.
- `runtime_abi` transcribes `hirsel-rt/include/hirsel_rt_v2.h`; a test requires
  every declaration, typedef and constant verbatim in that header. It holds
  the count word contract of compiled drop glue (`COUNT_OFFSET`,
  `COUNT_UNIQUE`, `COUNT_IMMORTAL`, `LAYOUT_OFFSET`), which the same test ties
  to the header's defines and offset assertions.
  `statics` is the static data a `ReadStatic` names (CT-2): a constant as
  immortal blocks, block 0 its own, whose pointer-free bytes name other
  blocks through `(offset, block)` reference cells; the heap checker installs
  them as handles and emission as relocations.
  `rt_program` is the only producer of the runtime's layout descriptors:
  it lowers each LayoutRecord's nested drop program to the flat `hirsel_rt_v2`
  step program, which emission writes as static data.
- Native schemas never read `OwnershipFacts`.
- Layout authority. `hirsel_layout::layout::LayoutBuilder` is the only producer of
  `LayoutRecord`s and `FnAbi`s. It reads type structure, size actuals and
  `hirsel_layout::layout::Owners`: one `hirsel_tir::RepresentationRecord` per source nominal,
  handle owners, and the library's container records (Map, Set, SortedMap,
  Slots, SlotId). The key encoding never writes a relation, brand, uniqueness
  or sendability; a callable's multiplicity stays because it selects the copy
  recipe. A nominal's private revision is folded into every containing key.
  Allocation facts (capacity and physical alignment) live in slab/prefix/extent metadata; the header contains count and descriptor index/storage kind.
- Variants carry an explicit u32 ordinal tag, except a null-pointer niche
  (layout version 2): a non-`@packed` variant of exactly two variants, one
  without fields and one with a counted block reference field, has no tag
  word; the first such field's word is null exactly when the fieldless
  variant is active (`TagForm::Niche`). Every tag reader goes through the
  form; `rt_program` lowers the niche's arm to `STEP_NONNULL`.
- Unavailable, never guessed. Open attribute combinations (#47), C interop
  beyond plain `@layout(c)` records (#6) and M2 builtins return
  `LayoutError::Unavailable` with their issue. A dynamic extent (Packed,
  variable flat) is never inline in an ordinary aggregate or sequence.
  Failed requests roll the table back.
- Recursion. Block payloads are reserved by id and derived once the inline
  layouts they read are complete, and a Box is complete before its payload is
  derived, so the records do not depend on request order; an inline cycle is
  `InfiniteSize`.
- ABI. Passing classes read only the declared convention and the public
  structure of each parameter type. Every nominal travels by address, so a
  private representation change moves layout keys and never the ABI digest
  (`abi::abi_digest`, which omits table-local layout ids).
  `abi::resolve_symbol` names the runtime symbol of a family template
  (`hirsel_rt_v2_{F}_sum`) from a call's scalar kinds: `{I}` the first scalar
  argument's integer kind, `{J}` the result's, `{F}` the float kind of the
  first scalar argument, else of the result; anything else resolves to
  nothing. The Ready validator and emission both use it.
- Validation. `layout_validate::validate_table` never calls the derivation. It
  checks each record locally against its fields and child records (scalar
  facts, alignment, offsets, overlap, tag, scan coverage, drop order, copy,
  size programs, SoA columns, inline acyclicity) and each FnAbi against its
  layouts. `native-tests/layout/` holds golden tables and
  the C mirrors that `tools/check-native.sh` probes with clang on both targets.
- Ordinary inline fields reserve `SizeSpec::storage()`: a bounded Packed
  value's largest variant extent, including inside tuples and closure
  environments. C checks containment and overlap against that bound. Variable
  flat encodings remain unbounded; their byte format and active extents are
  unchanged.
- A direct List or Array of a dynamic `Packed[T]` is sequential (MEM-13):
  `LayoutKind::Sequential` stores a u64 count, a u64 count of used bytes and
  the elements back to back from the first element-aligned offset, each at its
  active extent; nothing reserves the largest variant. `Packed[T]` is Flat
  (MEM-12), so an element holds no reference or resource and the sequence
  scans and drops nothing. C checks the size program, the words, constant
  aligned tag-selected extents and the empty programs. `packed_extents` reads
  the extents for consumers. An `Index` walks to its element past each earlier
  extent; a whole Packed element is read or written within its active extent.
  `SeqAppend` (LIR schema revision 19, library units only) appends an owned
  element at the used-byte end of the sequential block at its place, which it
  writes like `SeqReserve`; C requires the place's block payload to be
  sequential and the value to have its element layout (`Type("sequential
  append")`). `list.push` of such a list is that append.
  A flat list builder (LIR schema revision 22, `LayoutBuilder::flat_builder`)
  is a counted block of a sequential sequence whose elements are prefixed
  variable flat records (a u64 content length, then the content), each sized
  by its own prefix; C's layout check accepts exactly that element besides
  Packed ones. `FlatAppend { address, construct, version }` appends one such
  element built in place from a flat construction site of exactly its
  content record: fixed fields are consumed and variable ones only read, as
  for a flat `Construct`, the site's placement names no storage, and the
  write is `SeqAppend`'s (`Type("flat append")` otherwise). A builder is a
  source of a variable flat list field whose elements are its element
  (`encodes`): its count and used bytes are the field's count and content.
  A builder's `SeqReserve`, the one sequence write C admits outside the
  library and on a sequential block, asks for room for `capacity` of its
  least element (`layout::least_size` of the content behind the prefix),
  the bytes saturating; it keeps the count and used bytes.
  `hirsel-pipeline-tests/tests/it/rc_flat_builder.rs` forges an append of
  another record and a builder of another element; C and H reject both.
  A builder holds at most one open element (revision 24): `FlatField
  { address, field, op, version }` with `Open` begins one past the used
  bytes whose field `field` of the builder's content record, a byte sequence
  after only fixed fields (`Layouts::open_field`), is written in place; its
  length word lies at the element's start plus 8 plus the field's offset
  and the bytes follow it. `Reserve { capacity }` makes room for that many
  more bytes and `Push { value, spare }` appends one u8, `spare` when a
  proof (FACT-3) shows the room is held. `FlatClose { address, construct,
  field, version }` completes the element from a construction site of the
  content record lacking exactly that field, then counts it; until then the
  count and used bytes exclude it. C types the operands (`Type("flat
  field")`, `Type("flat close")`) and replays open elements over slot
  versions (`validate/flat_open.rs`): a reservation, a push and a close
  need the version reaching them open at their field, an opening one open
  at none, and every other write of the slot defines a version open at
  none (`Type("flat field of no open element")`, `Type("flat field opened
  twice")`). So a write between the opening and the close that could move
  or overwrite the element's bytes leaves the close without its element.
  `hirsel-pipeline-tests/tests/rc/rc_flat_field.rs` forges such a write, a
  close of another field and one of another record, a second opening, a
  pushed u64 and an unproven spare push.
  Inline of a dynamic Packed stays unavailable; a fixed-size Packed keeps the
  ordinary stride.
- Validators. `validate::validate::<S>(unit, &Inputs { target, layouts, manifest, operations })`
  seals a `ValidatedUnit<S>` only when every check passes, else returns the unit
  with every violation. `structure` checks vocabulary, arity and types against
  layouts and `FnAbi`s, SSA definitions and dominance, block arguments, address
  projections and preparation (an origin event's addresses are names it never
  accesses, so they need none), edge use and block kinds, and placement, with
  `Compare` only on comparable non-scalar layouts, `IsUnique` only of counted
  blocks, `Weaken` only from a many-call to the once-call callable of one
  environment, each closure capture with its environment field's layout (a
  `mut` capture an address field), and no write into a variable-size part of
  a flat value (a field of a flat record, or an element of a flat sequence,
  lies at its cursor). A flat record's construction takes each variable
  field's source value (a byte slice, or a block of a variable record's
  bytes) and only reads it; `FlatView` shows a String or Bytes encoding
  inside a block as a byte slice under a covering loan, and `FlatLength`
  counts a flat sequence (LIR schema revision 4). The scalar library
  instructions (LIR schema revision 6) are typed here: `Convert` by its mode's
  kind pairs and flag (`Type("conversion")`), `IntArith` as `Add` of one
  integer kind with a Bool flag exactly in `Checked` mode
  (`Type("integer arithmetic")`), `MulAdd` over one float kind
  (`Type("fused multiply-add")`), and the named `Binary` operations by domain
  (`Type("binary operands")`: divisions over integers, minimum, maximum and
  total order over floats, `TotalOrder` into I32).
  The native library's shapes (LIR schema revision 7; `tests/it/revision7.rs`
  holds each rule's control and twin): `SeqLen` of a sequence block or slice and `SeqCapacity`
  of a sequence block into a u64 (`Type("sequence length")`); `SeqReserve`
  and `SeqSetLen` through a place holding a sequence block by a u64
  (`Type("sequence write")`), writes of that block like a store through
  `Index` (never through borrowed storage), defining the slot's next
  version; `Hash` of a hashable layout into a u64, failing MEM-3 Depth like
  `Compare` (`Type("hash")`); `Recast` between a block view and the cursor's
  opaque source under the view's loan (`Type("recast")`); `FlatAt` and
  `FlatNext` walk a flat sequence of variable elements by content offset, or
  a sequential Packed sequence by offset from its first element (MEM-13),
  `FlatNext` naming only a whole element (`Type("flat element end")`);
  and an iterator's step thunk takes only its environment and returns
  `Option` of the element (`Type("iterator closure")`, `Type("iterator
  call")`). A `Slot` step, `SeqReserve`, `SeqSetLen` and `Recast` are
  library vocabulary: only a `UnitOrigin::Library` unit names them
  (`Vocabulary`). Drop flags (LIR schema revision 8, `tests/it/revision8.rs`):
  each `SideTables::init_flags` entry is a plain bool block parameter named
  once (`InitFlag`), true exactly when its slot holds content on entry; an
  argument is a bool constant matching the slot's content on its edge or the
  slot's current flag (`InitFlag`). A change to the slot's liveness or
  initialization ends its flag, a whole `Replace` of a live slot hands it to
  the displaced slot, and a `Branch` on it makes the slot initialized on the
  yes edge and empty on the no edge. A version join on an edge where the slot
  holds nothing names no content, and a read's reaching version defined in
  another block need not dominate it: the ownership check pins it. Bound
  calls (LIR schema revision 9, `tests/it/revision9.rs`): a `Callee::Bound`
  names function code and takes a `Callable` value's environment through a
  closure `FnAbi` (`CallShape("bound callee")`); a closure site without code
  (a generic nested function's value, spec #42) is followed through whole
  slot stores, reads and edges and is never an indirect callee, a call
  argument, a return value, a field, a capture or part of a place
  (`CallShape("a code-less closure outside a bound call")`).
  `ownership` solves slot liveness, initialization and moved-part masks,
  reaching versions (block parameters as version joins; stores, replaces
  (and a replace's `displaced_version` for its displaced slot),
  slot-backed parameters, `LirUnit::captures` entries and a `mut` call's
  write-back version, which reaches both of its edges, name a slot's
  content; a read needs exactly one, so two or more are one state and a
  block many failure sites reach joins in constant space), prepared places (a
  dynamic address, one that dereferences a counted block or checks a tag or a
  bound, is located at its preparation; a write to its root slot at or above
  one of those anchors may move the place, so the address is unplaced on that
  path until it is prepared again, and a use of it is `StaleAddress`; a write
  beneath every anchor, such as one through the address itself, keeps it, and
  preparing a derived address locates it afresh from the root, so it is no use
  of its base; validator version 6, `tests/it/invalid_ir.rs`; a sequence write,
  `SeqReserve`, `SeqSetLen`, `SeqAppend` or `FlatAppend`, rewrites the block at
  its place and so unplaces every address located through that block,
  validator version 7, `tests/it/invalid_ir.rs` and `tests/it/packed_storage.rs`), loans, stale
  views
  (a write that may copy-on-write a counted parent stales views through it),
  regions and the nested cleanup order (read from the `LirUnit::scopes`
  parent forest, never from ordinals) to a fixpoint, with a borrowed block
  parameter joining views under several loans covered by their union (stale
  once any of them ends or a part of a slot under any of its views is
  written), then replays holder
  credits exactly over normal and failure edges: joins must agree, nothing
  leaks at an exit, a holder outlives its loans, and a destructible value is
  released only at Ready. A slot holds nothing owned once its content is
  uninitialized or moved out whole, or once, on every path where it is
  initialized, every owning part of it (through inline fields, and through
  the fields of the variant a `VariantField` take showed active) is moved out
  and each inline resource among them has run `FinalizeEnd`; storage may then
  end and the function exit. Moved parts and finalization phases describe
  initialized paths only, so a join with a path where the slot is
  uninitialized keeps them.
  Each inline resource place goes `Live -> Finalizing -> FieldsPending`: a
  second `FinalizeBegin`, a `FinalizeEnd` without one, a field take from a
  resource with a finalizer before its `FinalizeEnd`, and storage ending
  while Finalizing are `FinalizeState`. `tokens` checks token provenance, donor eligibility,
  reuse compatibility by size class, alignment and heap domain, and token use;
  the credit replay counts tokens and forbids one on a loop-nesting-forest
  feedback edge or missing from a failure's disposal list. `Release` drops an owned value by its layout: a block at its payload
  layout, an owning inline value (Aggregate, Callable or Handle repr) at its
  own layout when no finalizer is reachable from it without crossing a block.
  `ready` rejects
  inferred call destinations, operations without a native disposition (a
  `Instruction` operation is an instruction at Ready, never a call; a
  `RuntimePrimitive` whose template names no symbol for the call) and a
  `Copy` whose recipe is Bits or FlatBytes (a Dup or Fields copy is the Ready
  primitive).
  Validators never read a producer's liveness, availability or shape answer,
  nor `OwnershipFacts`; destruction comes from layout drop programs. Worklist
  convergence is a fixpoint with no iteration cutoff, and the checks run in
  release builds.

Library string views are StringView aliases of String's slice layout. Unicode
ViewCursor is a four-word record (borrowed slice, relative position), derived
from its own owner representation; collection Cursor stays three words. Its
uncounted scan, empty drop and Bits copy recipe follow ordinary borrowed-field
composition, with no new validator rule. The reserved core view parameter types
pass their slice words by value, and an `Arena` parameter passes its token word by
value; unrelated nominal types retain address passing.

LIR revision 13 adds `InitFlag.path`: an empty projection range keeps the
whole-slot revision 8 flag; a finite inline field, variant field or constant
inline index names an independently guarded part. C proves each incoming bit
against its own absence masks, invalidates stale flags on overlapping writes,
and refines only that part on a branch. An inactive variant's fields are absent;
actual transfers remain separate so tag and sibling reads stay checked.

`FinalizeBegin` and `FinalizeEnd` belong only to Ready. Rc and Reuse bodies are
rejected with `FinalizationStage`, regardless of marker placement. Thus input
cleanup bodies never join a live resource with one already finalized in place.
Ready flag parameters independently summarize finalization phases on their true
inputs; a true guard recovers only those phases. No finalizer-live bit is needed.
`tests/it/revision13.rs` contains hand-built rejected controls and accepted twins.

Collection consuming-cursor acquisition records exactly the conjunction of outer
and entries uniqueness in its private flag, for Map and SortedMap alike.

`validate/flat_walk` independently replays every prepared FlatAt offset through
scalar transports and correlated block parameters: zero at index zero, or the
FlatNext of the preceding element of the same reaching sequence. Sequence identity
and reaching versions are substituted on each incoming edge too: a loop header's
reused SSA name cannot conceal changed content. A forged offset, different
sequence, wrong predecessor or changed loop-carried sequence is rejected as
`Type("flat walk offset replay")`. Source-free controls and accepted twins live
in `hirsel-pipeline-tests/tests/rc/rc_flat_walk.rs` (flat) and
`rc_packed_walk.rs` (sequential) beside the producer's rewrite tests.

Revision-7 amendments: `AbiResult.borrowed_parameters` declares the Borrow parameters a returned borrowed holder may name. C traces holder construction, reads, stores, joins, recasts and calls; an ended loan, a local or undeclared parameter return is rejected. `layout::field_is_borrowed` is the common scan/drop evidence for fields and captures, and a borrowed cursor may be noncopyable without owning a holder. `Terminator::InvariantFault` is a fatal implementation guard with no successors or source cleanup, distinct from a language `Check`. `tests/it/revision7.rs` pins these rules with hand-built controls and twins.

Non-owning edge arguments carry invalidity without reading their view. Actual
reads, calls and returns reject that invalidity even after a loan is reopened;
owned-credit edge transfers remain checked when transferred.

C preserves a proven owned-empty slot across joins of initialized paths, even
when one moved a resource and another finalized it in place. Content writes
clear that proof; initialized plain bytes and finalization phases stay separate.

C derives absent variant fields from current borrowed tag reads and successful
variant address checks. Its independent whole owned-empty must proof survives
joins with different exhausted masks; content writes clear it. Initialized
plain bytes, actual transfers, and finalization phases remain separate.

Revision 11 reuses `SideTables.init_flags` for persistent environment live words:
`InitFlag.environment: Some(layout)` names the trailing u64 capture slot and its
entry version; `None` retains the bool join flag. An `Environment { movable }`
layout guards the moved captures' scan and drop spans with `TagForm::LiveBit`.
`UpdateMask` also accepts a u64 word. C requires each owning capture take to be
followed by `Read Bits`, `UpdateMask(false)`, and a Store to that word; only a
proven clear may overwrite it. Ordinary slot replay rejects duplicate takes and
reads after a take. The caller retains the environment and releases it wholesale.

String/Bytes Slice elements, string views and the FlatFormatV0 String/Bytes encodings use the same canonical physical u8 key as source u8 (`LayoutBuilder::byte`). This keeps byte indexing, including of a flat Bytes part, and List[u8] descriptor compatibility independent of how the layout was requested.
`ProjectionStep<I = ValueId>` shares one field/variant/element vocabulary between
LIR operand paths and H's resolved `ProjectionStep<u64>` paths. Its exhaustive
`try_map_index` resolves operands without giving ValueId a runtime ordinal meaning.
The default LIR form and its snapshot encoding are unchanged.

Runtime descriptor field partitioning consumes each declaration's exact ordered
drop contribution, with independent borrowed-scan evidence for absent borrowed
fields and the existing LiveBit guards. Byte ranges do not decide ownership:
zero-sized fields still contribute finalizers under their own field ordinals.

Cleanup order applies within one cleanup activation. A failing statement cleanup
starts a separate frame unwind at its FailureCleanup edge; retained statement
temporaries participate in that new reverse order. FailureCleanup continuations
retain their existing order history.

LIR revision 16 retains `LayoutKind::Box` alongside `Block` with identical
physical representation and ownership recipes. `ProjectionStep::Payload`
dereferences exactly one Box, including a scalar or another Box payload.
C rejects Payload on every other kind, independently of source typing;
implicit field/index dereferences and explicit payload paths share the same
ownership path. A transfer of a partially moved Box retains its exact live
field mask when the remaining pointer is released.

Flat sequence construction accepts an ordinary sequence block exactly when its
element layout recursively encodes the declared flat element. Byte slices,
fixed scalar/flat bytes, nested sequences and variable flat blocks retain the
existing layout rules. Source credits are read, never transferred. Hand-built
element-mismatch and leaked-source controls are in `tests/it/flat_sequences.rs`.

A fixed flat nominal has one layout identity for its ordinary value and flat
body, including deferred Box payloads. Prefixed flat sequences mark wider
scalar fields unaligned at their existing one-byte alignment.

`LayoutBuilder::environment_data` derives ordinary capture records from concrete
field layouts, including nested borrowed environment records. It shares the
existing closure environment scan/drop and live-mask derivation.
`with_capture_parameters` derives a Definition ABI with explicit data parameters;
borrowed results may retain those Borrow roots, while owned results retain no
synthetic loan. Both are additive derivation helpers, with unchanged schemas and
independent validation.

SER-2 canonical wire bytes are independent of native layouts. The conservative
fixed FlatFormatV0/codec equality subset has both-target offsets and byte evidence
in `tests/it/canonical_wire.rs`; ordinary padding, variable flat framing, Box, Simd
and variants do not acquire a memcpy serialization path from a layout flag.

Flat part materialization extends `FlatCopy`: `layout` always names the address's
flat encoding. The result may have that same layout, or the ordinary source
layout that encodes it: a variable flat record's counted block (copy its payload
past the prefix), or a String/Bytes slice over a fresh counted byte sequence
(copy the length and bytes, with start zero). The source retains its credit;
the result owns one independent credit and participates in failure cleanup.
The byte-sequence storage layout must be present in the table.
`tests/it/revision4.rs` contains source-free controls, unrelated-layout mutants
and a missing-storage mutant.
Sequence element decoding into ordinary List/Array storage is not admitted yet.

LIR revision 18 adds `ConvertMode::IntToFloat`: any sized integer to f32/f64,
round-to-nearest, ties-to-even, with no flag or failure. C rejects other
operand/result kinds or a flag; `tests/it/revision6.rs` retains the hand-built twins.

LIR revision 23 adds `ConvertMode::Reinterpret` for `f32.bits`/`f64.bits` and
`f32.from_bits`/`f64.from_bits`: the bits of an f32 as a u32, of an f64 as a
u64, or the reverse, with every NaN read as the float kind's one observable NaN
(SEM-2). It replaces the four runtime entries. C admits exactly those four
kind pairs and no flag; `tests/it/revision6.rs` retains the hand-built twins.

LIR revision 19 adds diverging calls: `Terminator::Call.normal` is
None exactly when the callee's `FnAbi.diverges`, the ABI of a Never result, which
has no result or result destination. C rejects a returning call of a diverging
callee, a diverging call of a returning callee, a result on a diverging call and a
`Return` in a function whose own ABI diverges; `tests/it/invalid_ir.rs` retains the
hand-built twins.

LIR revision 20 adds body-proved `FnAbi.result_arena` and `ConstructSite.arena_no_drop`: C independently checks all returned roots and same-arena child credits; the complete-link-set check binds direct calls to their callee ABI.

LIR revision 21 binds every `Callee::Operation` call to its named registry
declaration. `CallSite.operation_arguments` carries concrete sizes and only the
type structure layout/ABI instantiation reads (including callable multiplicity).
It carries no TypeId, source binder or erased qualifier. `Inputs.operations`
supplies the registry signatures and owner representations independently of the
unit. C instantiates the named declaration, derives its FnAbi and compares every
ABI field plus the rooted layout graphs, independent of table numbering.
Missing declarations, wrong actual kinds/arity and mismatching logical or
physical ABIs reject as `CallShape("operation declaration ABI")`. Per-unit
memoization reuses only this independently computed answer for identical calls.
The controls in `tests/it/operation_abi.rs` include the physically identical
net.read/NetReadInto forgery, wrong size and type instances, a copied ABI key,
accepted twins and snapshot round trips; all four forgeries validate with the
declaration-ABI rule removed.

The dependency guard classifies hirsel-check-protocol as shared data and
hirsel-signature, hirsel-check and hirsel-lower as independent implementation
crates. Each consumes the shared plans and declaration helpers directly;
production and build dependencies between the implementations remain forbidden.

Flat-walk replay follows unchanged reaching versions through forwarding joins
with identical incoming SSA values. Different incoming content remains distinct.
`tests/it/flat_walk_forward.rs` is a source-free accepted forwarding control and
forged-offset twin; the changed-sequence controls retain their rejection.

A whole byte-sequence block may supply a Slice constructor's owner field.
C independently requires a u8 sequence payload, the whole sequence's start
(`layout::sequence_slice_start`, 8 under runtime ABI 5) and the SeqLen of that
exact owner SSA value as length. This transfers its existing credit
without allocating or inspecting a runtime descriptor. Hand-built controls in
`tests/it/byte_freeze.rs` reject other payloads and forged intervals.

Type-store test fixtures use `hirsel_types::testing::Types` through the test-only
`testing` feature; layout derivation and validation remain in this suite.

The opt-in `hirsel_lir::testing::UnitBuilder` buffers hand-built LIR blocks,
instructions and terminators for validator, backend and heap-checker tests.
It also assembles values, slots, addresses and side-table entries, and can bind
a finished unit to a layout table. Helpers use anchor, scope and ABI zero;
callers provide those entries. Suite-specific layout tables, identities, cleanup
recipes and assertions remain in test support.

Operation actuals use LIR-owned Primitive/BuiltinType tags. Validators receive
`operation::OperationDeclarations` and compare the derived declaration ABI
with the candidate call, including the rooted layout graph.

`ValueRepr::Uncounted` at an inline layout describes data whose block fields
refer into one compiler-local region without holder credits. Its physical layout
stays the logical aggregate layout, including its pointer encoding. The emitter
rebases headerless SSA references at field boundaries. C reconstructs region
provenance through construction, storage versions and projected reads; regional
constructions with elided drops must hold trivial fields or uncounted children
of the same region. Other region payloads retain scheduled cleanup.
User finalizers cannot be omitted, and nested regional values cannot return.

LIR revision 25 adds `ConstantValue::Heap`, the distinguished inert Arena token.
C admits that exact literal at Arena layout; integer bits and call results still
cannot forge tokens. Scoped Arena tokens retain their open-region obligations.
Runtime ABI 6 passes the destination token to both deep-copy entries.

LIR revision 26 (MEM-6) adds an optional u64 `budget` operand to `ArenaOpen`
and `ArenaToken`, the bounded arena's capacity in logical bytes (the table is
in the runtime README; `layout::logical_size` and `layout::block_charge` read
it from layouts, and `tests/it/logical.rs` holds them to
`hirsel_types::logical`). `Terminator::Recover { arena, caught, failed }` is
the one way from failure cleanup back to ordinary code: when the active failure
is exactly the `OutOfSpace` of a bounded arena this unit opened, with no later
failure, it is forgotten and control resumes on `caught`; otherwise it continues
on `failed`. C admits it only in a library body's failure cleanup, with a
`Body` target for `caught`, naming an arena opened with a budget in the unit;
`tests/it/revision26.rs` pairs each rule with its valid twin.
