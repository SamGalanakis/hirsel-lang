# hirsel-lir

Native data schemas: the staged LIR family, target
descriptors, the physical layout table, the cross-unit ABI, `hirsel_rt_v2`
declarations, effect flags and deterministic snapshots.
`layout.rs` holds physical arithmetic, `abi.rs` holds physical ABI operations,
and `layout_validate.rs` checks layouts and ABIs independently. The type-to-layout
bridge, instance keys/views and declaration ABI derivation live in
`hirsel-layout`. `validate/` holds the independent stage validators. `discharge.rs` is the PS-13
check-removal plan (TIR sites by definition, each with its removal form), and
`validate::validate_discharge` compares a discharged unit with its reference
import: their failure-site inventories by anchor, opcode and rule may differ in
exactly the planned sites. `hirsel-graph` supplies pure traversal, dominance and loop-forest algorithms;
the validators derive their own normal and failure graph from the IR.
`emission.rs` is the backend-neutral emission interface both native backends
implement: `EmitInputs`, failure codes, link names, site codes, descriptor names,
the immortal prefix, the register-result rule, the receipt check and the object
manifest; `object.rs` reads an emitted ELF object's symbol interface.
`fuel.rs` names a unit's fuel points, its entry block and the targets of its control-flow
graph's feedback edges, where code emitted with `EmitInputs::fuel` charges one unit of a
run's fuel budget (`CTX_FUEL_OFFSET`); the mode changes no LIR and is part of the object's
options.
`profile.rs` shares immutable source ownership and source/cleanup anchor selection
between native backends.
`persistent.rs` is the structure-sharing vector that
dataflow states are built from, here and in `hirsel-rc`: a state costs what it
changes, so an analysis stays near-linear in the size of a function.

Allowed dependencies: the dependency-free `hirsel-graph` utility and shared data crates (`hirsel-span`, `hirsel-syntax`, `hirsel-core-contract`, `hirsel-codec`); no external production
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
  every declaration, typedef and constant verbatim in that header, including
  the host exit-report registration and its typed report-node pointer. It holds
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
  without fields and one whose inline payload holds an aligned non-null
  pointer word, has no tag word; the first such word is null exactly when the
  fieldless variant is active (`TagForm::Niche`). The word is found depth
  first in declaration order: a counted block reference, Ptr or FnPtr (pointer
  scalars since layout version 4), a String or Bytes slice's owner word, or
  such a word in an aligned field of an inline Aggregate (nested since layout
  version 5, so `None | Some((String, u64))` is 32 bytes). Variants,
  environments, callables, sequences and `@packed` fields are never entered.
  The validator re-proves the word along the same path and rejects a nullable,
  uninitialized or misaligned one. Every tag reader goes through the form;
  `rt_program` lowers the niche's arm to `STEP_NONNULL`.
- Externs have the `Foreign` FnAbi shape and only they do (PROG-4): no run context,
  environment or destination, and a C scalar/nullable-pointer result or none.
  `Ptr` and `FnPtr` are non-null pointer scalars; their Options use the existing
  null-pointer niche and one C pointer register. The internal aggregate ABI stays
  unchanged. A `FunctionRef` at a pointer scalar is a definition's C callback address.
  Runtime ABI 18 adds foreign source sites, caller-owned scope save storage,
  ForeignLeave and ForeignViolation; no instruction or TIR form is added.
- Unavailable, never guessed. Open attribute combinations (#47), C interop
  beyond plain `@layout(c)` records (#6) and M2 builtins return
  `LayoutError::Unavailable` with their issue. A dynamic extent (Packed,
  variable flat) is never inline in an ordinary aggregate or sequence.
  Failed requests roll the table back.
- Recursion. Block payloads are reserved by id and derived once the inline
  layouts they read are complete, and a Box is complete before its payload is
  derived, so the records do not depend on request order; an inline cycle is
  `InfiniteSize`.
- Content keys. A record's `LayoutIdentity` names the request it answers; its
  `LayoutKey` (`layout_keys.rs`) names its content. `hirsel_graph::scc_keys` keys
  the layout graph, whose edges are a record's references in record order (kind,
  fields, variants' fields, scan, drop): a layout outside every cycle is a Merkle
  hash of its record with each reference replaced by its target's key, and a
  member of a cycle (`Cons`, `Even`/`Odd`) hashes the rooted DFS of its
  component. Keys never depend on `LayoutId`s, request order or unrelated
  layouts, and an edit changes the key of every layout that reaches it. FnAbi
  keys replace parameter and result layouts by their keys. Rooted keys visit only
  a completed layout's reachable graph and use the same encoding; intrinsic empty
  sequence and boxed nullary statics use these keys before import.
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
  kind pairs and flag (`Type("conversion")`), `IntArith` over one integer
  kind with wrapping operations listed under revision 28 below, named Add
  modes, and a Bool flag exactly in `Checked` mode
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
  released only at Ready. RC-ROOT (S-14) keeps root-bearing content (whose
  drop program reaches a `Shared` root, derived from the layout records
  alone) at its cleanup anchor before Ready: a drop or release of it under a
  TIR cleanup anchor sits in a `Cleanup` or `FailureCleanup` block, one under
  a disposal or state-release anchor in a failure cleanup
  (`RootOutsideAnchor`; a library body, which has no TIR cleanup chain, keeps
  its drops where its operation's contract places them, and a displaced slot's
  drop ends content that already left its place); a semantic copy of it
  (`Share`) is never a moved-out value, i.e. a Dup cancelled against its
  holder's release (`RootCopyCancelled`); and it is never region-placed
  (`RootInRegion`). A slot holds nothing owned once its content is
  uninitialized or moved out whole, or once, on every path where it is
  initialized, every owning part of it (through inline fields, and through
  the fields of the variant a `VariantField` take showed active) is moved out
  and each inline resource among them has run `FinalizeEnd`; storage may then
  end and the function exit. Moved parts and finalization phases describe
  initialized paths only, so a join with a path where the slot is
  uninitialized keeps them. A part inside a part moved out on one path is
  moved on that path too, so a join keeps it moved where every path moved it
  or an enclosing part.
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
An initialized variant alone does not prove a payload present: every incoming
path must observe its tag or successfully prepare that variant's address before
passing a constant payload flag. Joining an observed path with an unobserved one
retains unknown presence. A true validated payload flag preserves that evidence
when a later edge passes a constant true flag.

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

Within one scope, cleanup releases temporaries before names. A Replace's
`Displaced` slot is a TIR temporary and releases as one.

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

LIR revision 38 adds `ConvertMode::FloatResize` for `f64.from_f32` and
`f32.from_f64`: an f32 into an f64 exactly, or an f64 into an f32 rounded to
nearest, ties to even, with every NaN read as the result kind's one observable
NaN (SEM-2). C admits exactly those two kind pairs and no flag;
`tests/it/revision6.rs` retains the hand-built twins.

LIR revision 39 adds `CodecNode::Checked { layout, check, value }` and
`CodecPlan::checks` (codec program version 4, node 17): a checked opaque type
(SER-1) encodes as `value`, its record or variant representation at the same layout,
and decoding calls `checks[check]`, a unit function symbol at a `fn(value: T) -> bool`
FnAbi borrowing that layout, on every reconstructed occurrence, inner ones first. The
first `false` releases the value and decodes to `DecodeError.Invalid`, the eleventh
`DECODE_ERRORS` arm; a failing check ends the run (FAIL-3). C holds the node to a
record or variant child at its own layout and the check to that exact FnAbi, and an
encode plan to no checks; `tests/it/revision29.rs` retains the hand-built twins.

LIR revision 41 adds `Inst::ScalarUnary { result, op, operand }`, which never
fails, for the exact core operations of one operand (`ScalarUnaryOp`):
`float.sqrt` (IEEE squareRoot), `float.floor`, `ceil`, `trunc` and `round`
(roundToIntegral toward negative, toward positive, toward zero and ties away)
and `float.abs` of f32 or f64, and `int.leading_zeros`, `trailing_zeros`,
`count_ones`, `swap_bytes` and `reverse_bits` of a sized integer's two's
complement word (a zero counts the width). `IntArith` also admits `Sub` in every
mode and `Mul` in `Checked` mode, for `int.wrapping_sub`, `saturating_sub`,
`checked_sub`, `wrapping_mul` and `checked_mul`. C holds a float op to a float
operand, an integer op to an integer one, and the result to the operand's type;
`tests/it/revision6.rs` retains the hand-built twins.

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
LIR revision 40 adds `ConstantValue::Descriptor(LayoutId)`: a `u32`, the run's registry index of
that layout's runtime descriptor, which both backends emit as the index a construction's
allocation receives. The structure validator requires a `u32` result and a layout of the table;
`channel.open`'s body passes `Option[T]`'s to the runtime, which then moves and drops messages
it was not compiled for (CONC-6).
C admits that exact literal at Arena layout; integer bits and call results still
cannot forge tokens. Scoped Arena tokens retain their open-region obligations.
Runtime ABI 6 passes the destination token to both deep-copy entries.

LIR revision 26 (MEM-6) adds an optional u64 `budget` operand to `ArenaOpen`
and `ArenaToken`, the bounded arena's capacity in logical bytes (the table is
in the runtime README; `layout::logical_size` and `layout::block_charge` read
it from layouts, and `tests/it/logical.rs` holds them to
`hirsel_type_analysis::logical`). `Terminator::Recover { arena, caught, failed }` is
the one way from failure cleanup back to ordinary code: when the active failure
is exactly the `OutOfSpace` of a bounded arena this unit opened, with no later
failure, it is forgotten and control resumes on `caught`; otherwise it continues
on `failed`. C admits it only in a library body's failure cleanup, with a
`Body` target for `caught`, naming an arena opened with a budget in the unit;
`tests/it/revision26.rs` pairs each rule with its valid twin. Runtime ABI 7 grows the
region record to 64 bytes (`REGION_REMAINING_OFFSET`), adds `STATUS_EXHAUSTED` and the
`arena_bound`, `arena_exhausted` and `arena_recover` entries; layout descriptor version 6 adds
each payload's `logical` charge (`rt_program::Descriptor::logical`).

LIR revision 27 adds `Shared[T]` (MEM-11): `LayoutKind::Shared { payload }` is a
counted block reference (`ValueRepr::Block`) whose payload is
`LayoutKind::Frozen { value }`, the frozen value inline at offset 0. The frozen
record's drop program leads with `DropStep::Thaw`, lowered to `STEP_THAW`, and its
descriptor carries `LAYOUT_FROZEN`. `Freeze` and `Thaw` are library-only: `Freeze`
moves a value into a fresh root and `Thaw` moves the last root's value out. A
library unit may test the uniqueness of an owned root before RC, since its count
is the root count. Runtime ABI 8 adds `hirsel_rt_v2_freeze` and `hirsel_rt_v2_thaw`.

A library unit may also address field 0 of a borrowed `Shared` root's `Frozen`
payload (`shared.with`): the projection dereferences the root block as it does a
Block or Box. Such an address admits only its preparation, a further projection, an
immutable `BeginLoan` and a `Read Borrow`; any other use, such as a `Store`, `Read
Take`, `Copy`, `Drop` or `mut` argument, is `Type("access to a frozen value other than
a borrow")`, and a source unit's projection through a root is `AddressType`. The loan
on the root's slot keeps the root live while the view is used. `tests/it/shared.rs`
pairs each rule with its valid twin.

LIR revision 28 adds `AddressInfo.source_place` and `unchecked`: the canonical
TIR place and the exact projection ordinals whose bounds/tag guards PS-13
removes. Paths, reaching versions and ownership replay are unchanged. C requires
an independently supplied `Inputs.discharge` plan for each unchecked ordinal's
source site, canonical place and rule, and refuses duplicates, non-check steps
and absent evidence as `Type("unchecked projection evidence")`.
`tests/it/projection_proofs.rs` holds hand-built controls and forged-site twins.
The discharge inventory counts projection guards individually, independently of
the address's shared failure-rule label. Origin/loan naming events have no guard.
Wrapping `IntArith` also admits Sub/Mul/Div/Rem/Shl/Shr; Neg is Sub from zero.
Div/Rem require a nonzero divisor, signed Div excludes MIN / -1 and shifts require
a count below the width. Signed MIN % -1 is zero. Checked/saturating modes stay Add.

LIR revision 29 (SER-2) adds `Inst::Encode { result, value, codec }` and
`Inst::Decode` over `SideTables::codecs`. A `codec::CodecPlan` holds the
canonical descriptor bytes (`hirsel_types::wire::Codec::descriptor`) and one
`CodecNode` per schema node at its native layout, plus the root, the Bytes node
and, decoding, the `Result[T, DecodeError]` node. `CodecPlan::check` holds each
node to its layout record: value scalars, byte slices over their storage
sequence, record, tuple and flat fields, ordinal or niche tags (Packed
included), boxes, ordinary/flat/sequential/SoA sequences, Inline/Simd repeats, and Map/Set containers of an
entries list and an optional u32 index. `CodecPlan::program` lowers a plan to
the u64 words the runtime walks, with descriptor indices from the linked
registry rather than layout ids. Neither instruction fails: an operational
budget is environmental and a decode error is a value. C rejects a plan that
disagrees with the table or with the instruction's direction (`codec plan`)
and an operand or result at another layout than the plan's (`encode`,
`decode`); `tests/it/revision29.rs` pairs each rule with its valid twin.
Runtime ABI 9 adds the `codec_encode` and `codec_decode` entries.

LIR revision 30 extends codec plans with FlatText and Framed nodes. C verifies
exact length framing, byte element layouts and each framed child's content
layout. Variable flat blocks retain their payload nodes; native codec program
version 2 distinguishes flat fields and Packed arms, sequential active extents,
and declaration-order SoA columns addressed by stored capacity. Decoding checks
wire data before rebuilding native framing or allocating counted storage.
A Path node (a core `Path`, program node 16) is held to a `LayoutKind::Path`
record and a scalar node never is: the runtime encodes the run-context
identity's text and decodes by interning it as `path.of` does.

LIR revision 31 (FAIL-2) adds `Terminator::Isolate { start, caught }`, the
catch of `task.isolate`: every failure recorded since `start`, the u64 record
count the unit read before its callback ran, is caught, and control resumes on
`caught`. Like `Recover`, it leaves failure cleanup for ordinary code; C admits
it only in a library body's failure cleanup, with a u64 `start` and a `Body`
target for `caught`, and `tests/it/revision31.rs` pairs each rule with its valid
twin. Runtime ABI 10 adds the `isolate_catch` entry.

Sequence length, capacity, reserve, length commits and library Slot projections
also admit SoA payloads. Their element layout remains the ordinary row layout;
physical consumers gather and scatter its fields using stored column capacity.

LIR revision 32 (MEM-6) keeps a List's budget in a bounded arena. A List or Array
construction whose `ConstructSite::tracked` is set, placed in an arena, builds a
tracked arena block: its count is `COUNT_ARENA_UNIQUE` until a dup or `Share`
makes it `COUNT_ARENA_SHARED`, stickily, and a release never changes it.
`IsWritable { result, value }` (RC onward) is copy-on-write's test, true for one
holder or an unshared tracked block; P4 emits it where it emitted `IsUnique`.
`Share { value }` follows a semantic copy and marks the tracked blocks the value
holds at inline depth shared. `ArenaPush { address, version, failure }` is
library-only and begins `list.push`: a tracked block is charged one element (and
its length when shared), then copied or relocated within its arena, or the
arena's own `OutOfSpace` is raised on `failure`. `tests/it/revision32.rs` pairs
each rule with its valid twin. Runtime ABI 11 adds `hirsel_rt_v2_arena_push`, a
`flags` argument to `arena_alloc` and `unique_copy` (`ALLOC_TRACKED`), and the
tracked block's 32-byte prefix, which names its arena; layout descriptor version 7
adds `LAYOUT_SEQUENCE` and `LAYOUT_UNTRACKED_BELOW`.

LIR revision 33 (MEM-6) keeps a Map's or Set's budget in a bounded arena.
`ConstructSite::tracked` also marks a Map's or Set's block and its entries List, which a
deep copy into an arena tracks too (`layout::keyed`, descriptor version 8 `LAYOUT_KEYED`):
the block charges nothing itself (`block_charge`) and its index is neither charged nor
tracked (`layout::keyed_index`). `map.insert` and `set.insert` append through `list.push`,
whose `ArenaPush` charges the entry. RC's copy-on-write treats a Map's own entries and index
as its storage: crossing its block does not make them nested, while a Map or Set lent or
written through another block is shared like a List. Runtime ABI 12 lets a tracked arena
block have a fixed layout.

LIR revision 34 (MEM-6) charges an Io read into an arena List before its effect.
`ArenaPush::count`, a u64, prepares that many appended elements at once: import places one,
with the read's maximum, before each `stdin.read`, `file.read` and `net.read` call, in any
unit, through an address of its own. The read then appends without charging or relocating.
`tests/it/revision34.rs` pairs its rule with a valid twin. Runtime ABI 13 adds
`hirsel_rt_v2_arena_reserve`.

LIR revision 35 (MEM-6) tracks an `@soa` List in a bounded arena. `ConstructSite::tracked`
also marks a List of `SoaColumns`, a deep copy tracks one too (descriptor version 9
`LAYOUT_COLUMNS`), and `ArenaPush` accepts its place: it is charged and copied at its
capacity when shared, never relocated, since only the compiled column copy lays its
columns out. That copy, `SeqReserve`'s, writes an unshared tracked block in place and
regrows a tracked one in its arena (runtime ABI 14 `hirsel_rt_v2_arena_regrow`), emptying
the abandoned block.

LIR revision 36 (MEM-6) tracks a sequential `Packed` List in a bounded arena the same way.
`ConstructSite::tracked` also marks a List of `Sequential` layout, a deep copy tracks one
(descriptor version 10 `LAYOUT_SEQUENTIAL`), and `ArenaPush` accepts its place with a zero
payload size: the element's logical size is in its charge, and its block is copied when
shared, never relocated. `SeqAppend`'s growth, `hirsel_rt_v2_block_grow` (runtime ABI 15),
regrows an unshared tracked block in its arena and empties the abandoned one.

Descriptor version 11 marks the release that ends a layout's program, through the arms of
a last variant, as `STEP_RELEASE_LAST` (`rt_program::mark_last`): the runtime frees the
dying block before the child that release kills drains, and compiled drop glue follows the
same rule.

SortedMap reuses `LAYOUT_KEYED`: its sole payload field is the entries List at
offset zero. It has no index. Its placed constructor tracks both blocks, and
`sorted_map.insert` charges a vacant entry with `ArenaPush` before taking the
key/value. Existing detach and deep-copy machinery charges and tracks its entries.

LIR revision 37 / ABI revision 2 retain caller attribution independently of the
callable language type. `FnAbi.caller_site` describes two physical hidden u64 slots;
`UnitHeader.track_caller` selects attribution policy. They differ because ordinary
closures need the uniform physical carrier while reporting their own failures.
`CallSite.forward_caller` preserves source invocation boundaries through callee
specialization. The independent validator requires carriers on callable ABIs and
tracked bodies and excludes them from foreign ABIs. Hand-built controls in
`abi.rs` and `invalid_ir.rs` each have guard-removal witnesses.

Validator version 9 re-proves OWN-3 at call sites (`validate/aliasing.rs`,
`CallAlias`): a callee reaches a `mut` argument's place only through that
argument. No two `mut` arguments of one call overlap, and no other argument and
no callee value holds a loan on an overlapping place. A value's loans are its
own, the views it carries through borrowed record fields, and the loans of the
borrowed captures of every closure it may be. The closure is followed through
slot stores, reads, copies, recasts, weakenings, constructions and block
parameters, and a loan through a value root views what that value views. The
rule concerns what the call hands over, not loan activity: a loan the caller
still holds but passes nowhere is not the callee's. The native backends'
`noalias` on `mut` place addresses rests on it. `tests/it/ownership.rs` pairs a
second `mut` argument, a borrowed view and a capturing closure, each against
the same route to another place.

Native Map and Set storage retain optional private entries, tagged u64 buckets, cached
hashes and a live count. Set uses the same table with unit values. Library
revision 23 and descriptor version 13 identify the shared representation; no
TIR/LIR instruction changes. The descriptor carries a live-count program
for private Map entry Lists, derived once by `logical_storage::MapStorage` from
the frozen layout table and reused by the descriptor registry and H. Charges
count live key/value pairs and exclude tags, buckets and hash-cache storage.
Codec program version 5 extends the private entry offsets to Set, after version 4
added checked opaque nodes, without changing
`CodecNode::Map` or the wire format. Runtime ABI 21 adds the streaming hash entries.

Runtime ABI 22 enables the existing hidden caller-site pair on blocking
task/channel exports, so native Blocked outcomes can name their parked waits.
The LIR vocabulary and physical ABI encoding are unchanged; LLVM and Cranelift
consume the same existing caller-site mechanism.

Runtime ABI 24 adds the `$intrinsic.SharedThawTurn` operation export
`hirsel_rt_v2_thaw_turn`, which `shared.thaw`'s compiled body calls instead of an inline
`IsUnique` on its root, so free mode can record and replay the answer.

Runtime ABI 25 makes `Shared` root counts exact and atomic (lane R3). Layout
version 6 marks root slots with `ScanStep::Root` and `DropStep::ReleaseRoot`,
which `hirsel-layout` derives for every `Shared` reference and which descriptor
version 13 lowers to `STEP_RELEASE_ROOT`. Both emitters add a root with
`hirsel_rt_v2_root_dup` (LLVM inline by a CAS below the dup bound) and give one
up only with `hirsel_rt_v2_root_release`; glue never releases a root inline.

Runtime ABI 29 dispatches extern calls by S-13 (lane R8): `hirsel_rt_v2_foreign_enter`
takes the extern's dispatch word (`FOREIGN_THREADS_*`, `FOREIGN_BLOCKING` and the package
ordinal from `FOREIGN_PACKAGE_SHIFT`), which emission reads from `EmitInputs::foreign`; the
caller-owned interval record grows to `FOREIGN_SCOPE_BYTES` 40; and callback thunks call
`hirsel_rt_v2_callback_leave` before returning to C. The LIR vocabulary is unchanged.

`objects::DirObjects` adapts the common artifact store: typed native action
associations refer to checked immutable outputs. Only the configured local
writable provider supplies `ProducedBytes`; imported content supplies integrity
without production authority. The v3 directory reset has no old-format reader.

LIR revision 61 allows `FnAbi.nonretaining` to select immutable data roots as
well as callable content. Strict C seeds the selected content, follows its
physical flow through slots, projections and SSA joins, and rejects retained
results, owning captures and calls without a matching body-proved contract.
Ordinary C also enforces these data claims and every sealed-root read.
Numeric field reads and `SeqLen` produce independent bits. A `mut` data entry
cannot claim this read-only contract. `ScopedEntry` binds the contract of the
`region.with` callback; complete qualification checks its actual body and entry
identity. These contracts grant no receipt provenance or storage ownership.
