# hirsel-rc

Native program transformations: instantiation and
mandatory self-tail normalization with SSA import, placement and Perceus
insertion, drop specialization and reuse, cleanup lowering,
scalar legalization and the Ready stage. Each lives in its own module;
sharing this crate does not imply sharing implementation state.

Allowed dependencies: the dependency-free `hirsel-graph` utility and shared data crates (`hirsel-lir`, `hirsel-core-contract`,
`hirsel-tir`, `hirsel-types`, `hirsel-syntax`, ...). Never another implementation
crate (`hirsel-llvm`, `hirsel-rt`, `hirsel-heapcheck`, `hirsel-kernels`, the M1a
implementation crates) or `hirsel-driver`; only the driver wires them. The
`hirsel-lir` dependency test enforces this.

Invariants:

- Every pass consumes and produces `hirsel-lir` artifacts; a stage's output is
  usable only after that stage's validator in `hirsel-lir` accepts it. A pass
  never constructs a receipt.
- Passes read TIR loans, cleanup chains and `Destination`, never
  `OwnershipFacts`.
- Rewrites follow discover, prove, commit: discovery is read-only.
- Validation never consults a pass's own liveness, availability or effect answer.

- Every analysis is near-linear in the size of a unit. Abstract states are
  `hirsel_lir::persistent::PVec`s, which share structure between program
  points: never clone a dense per-slot vector per instruction, edge or block,
  and never scan every slot at a site (`lacking` lists the slots that hold
  something). `hirsel-pipeline-tests/tests/rc/rc_scaling.rs` gates the whole pipeline at eight times the
  statements.

## Append fusion (`append_into`)

Before instance expansion, `append_into::fuse` rewrites the `Program`'s TIR: a
call whose `Bytes`/`String` result is only copied whole onto a list `D` (a
recognized `for k in 0..bytes.len(V) { list.push(mut D, V[k]) }` loop, or a
call to a function whose whole body is one) instead appends to `D`, and its
result slot gets the static empty value, so the copy runs zero times.
Producers: std@1 `T.to_string` appends as `T.format_into`; a *builder*
(`bytes.from_list(take L)` of a local list created empty and otherwise only
appended to by `list.push`, `list.reserve` or an appending form, not a copy
function) gets a derived `@preserves_unique(out) f$into(.., mut out:
List[u8]) -> ()`, as `T.format_into` declares it, so a destination the copy
kept unique stays unique for the IR checker (MODE-2). A site
fuses only when the instructions between call and copy are total and leave
`D` untouched and the producer's arguments do not name `D`; a builder only
when nothing can fail after its first append (FAIL-1 cleanup could otherwise
observe a partial `D`). The output is ordinary TIR: every later pass and C
see a call and a function. `hirsel-pipeline-tests/tests/rc/rc_append_into.rs`
validates every rewritten and derived definition with the IR checker, runs C
at each stage and H against the interpreter, and keeps the declined twins.

## Task E: instances, self-tail loops, SSA import

- `instance`: expands the program from the GEN-5 graph (`Program::requirements`,
  the driver's `InstanceGraph` edges). Roots are non-generic functions and
  nominals; requirements are followed under each owner's actuals, closures are
  found in their parent's body. Keys are `InstanceKey`s of binder-aware type
  and size actuals with relations, brands, uniqueness and sendability erased,
  so actuals differing only in those share one instance. Every view shares its
  generic TIR body (`DefinitionRef`); `Instances` also records each instance's
  actuals, callable type, `FnAbi` and closure environment layout. A generic
  nested function (spec #42: a value only as a direct callee) resolves through
  whole assignments and lexical capture edges to its producer. Generic names
  whose environments contain only other static generic names need no runtime
  storage or captures; their calls use a Definition ABI and direct code.
  Expansion keeps the caller's argument actuals separate from the producer's
  enclosing actuals. The lexical edge index and shared producer paths are
  computed once per definition. A nonempty generic declaration stores an
  ordinary environment record containing only its data snapshot. Wrapping
  closures capture that record; concrete nested instances receive its fields
  as explicit Borrow parameters and use a Definition ABI. Ordinary once closure
  capture takes and their persistent live masks stay in the owning wrapper.
  Borrowed results name their declared and captured Borrow parameter roots.
  C still rejects a code-less callable captured as a value. A
  requirement naming generics its owner instance does not bind belongs to a
  nested function's body and is followed by that function's instances.
- `normalize`: plans mandatory self-tail loops from `Body.tail_sites` only,
  re-verifying each site for its instance with that instance's layouts; a site
  that fails stays a call and is reported as `Declined`. A diverging self call
  (`fn serve() -> Never { ..; serve() }`) has no continuation and loops like any
  other site. `ConstructorHole` is
  rejected until spec #46 rules. `permute_parameters` reorders a helper's
  parameters, updating its view, a new `FnAbi`, its unit's parameters and slot
  ordinals, caller-placed destinations and every call site in one transform.
- Discarded temporary callable reads import as borrowed SSA observations. They
  preserve bounds/tag failure order without creating owning storage or credit;
  discovery requires one Borrow assignment and no value, loan or cleanup use.
- `import`: one `LirUnit<Value>` per instance (`lower` runs expansion,
  planning and import and seals units against the finished layout table).
  `lower_discharging` imports under a PS-13 check-removal plan
  (`hirsel_lir::discharge`): a licensed TIR `Check` emits no `Check` and no
  failure edge; licensed integer operators use wrapping `IntArith` (Neg is
  subtraction from zero), and projected places mark only licensed bounds/tag
  guards unchecked.
  Each planned definition's units are imported twice, first with every check
  as the reference, and both are kept in `Lowered::discharged` for the
  driver's `validate_discharge`. Append fusion carries original sites and
  canonical places through retained instructions and derived appending bodies;
  inserted instructions have no obligation. `Lowered::native_discharge` keys
  the accepted source sites by native definition.
  Slots hold mutable storage; every write names a fresh version, merges join
  versions through block parameters. Operands are read in TIR order with their
  declaration ordinals. Failure successors are TIR cleanup chains expanded and
  specialized by an abstract state that mirrors the validator's; the reserved
  cleanup anchors and the synthetic disposal scope are fixed. Chains share tails: a chain block is keyed by what it
  runs (its loan ends and its drop), the state of the slot it drops and the
  block it continues to, so failure sites that differ only in slots the rest
  of the chain does not touch meet as early as they agree. A TIR cleanup
  action anchors at its position in the first cleanup (by id) that runs it,
  the same name in every chain through it. `Match` becomes ordered tag switches (exhaustive, the
  `otherwise` edge unreachable) and literal tests, with guard continuations.
  A comparison of non-scalar operands is `Inst::Compare` (MEM-3 Depth), its
  owned operands read through views under synthetic loans and the taken ones
  released after. Taken comparison and flat-construction operands move into
  fresh operation-owned temporaries, so their cleanup precedes kept sources
  and intermediate copies on failure; a many callable received by once storage or an owned once parameter is
  `Inst::Weaken`ed; mutable aliases require exact multiplicity. An origin event (`BindOrigin`, `BindFields`,
  `EndOrigin`) names its places without preparing them, so a bounds or tag
  check runs only where the place is accessed (after an assignment's
  right-hand value is built), at the site of the origin binding that named
  the place, as in the interpreter. The exception is a `BindFields` of the
  variant a match's tag tests proved (an arm's own test, or elimination by the
  earlier arms' tag tests): its field addresses are prepared there, where the
  check cannot fail, and later operand reads of those places reuse
  them while their slot keeps that version (FACT-3). Such facts flow only along
  single-predecessor TIR edges, so the preparing point dominates every reuse,
  and each emission pass starts with none: value ids restart every pass.
  A pattern tag already proved at the current version is not tested again,
  avoiding infeasible edges that would restore absent payload fields.
  A temporary assigned views of places under loans on
  several paths is a joined view: an SSA value carried on edges and joined
  into a borrowed block parameter declaring the least of its loans, never
  stored or copied. A field of a flat value, or an element of a flat sequence, is a place at
  its cursor, its address prepared where it is used; a variable-size part is
  its length-prefixed encoding, read by flat-aware operations (LIR schema
  revision 4): a flat record is constructed from its fields' source values,
  which it only reads (taken ones are released after it); a borrowed String
  or Bytes part is a `FlatView`; `list.len` of a flat list part is a
  `FlatLength`; and a temporary holding a read-only copy of a part of a
  borrowed parameter (`for x in xs.items`) is a view of that part. The same
  holds for a stable owned local when it has one whole initialization outside
  every CFG cycle, is never written, moved or mutably lent, and its scope
  encloses the receiver. Computed source indices and whole ordinary List
  consumers retain materialization. Copying or
  taking a variable flat record or String/Bytes part materializes distinct
  owned storage through `FlatCopy`, with the address encoding and ordinary
  result layouts kept separate. Ordinary List parts materialize through an empty
  sequence, list.reserve and a checked element decode/list.push loop; flat_walk
  carries the variable-element cursor. Nested Lists decode recursively; the
  existing proof leaves nested cycles indexed. Partial results use normal
  failure cleanup, and flat Index/FlatAt paths retain their logical owner for
  last-use release. Array materialization remains unsupported.
  A taken indirect callee moves into a fresh invocation temporary in the call's
  scope. The call borrows it, then releases it before caller cleanup on either
  outcome; its later slot ordinal preserves reverse temporary destruction order.
  Native call operand preparation cleans current caller storage on failure;
  only an entered call uses TIR cleanup that excludes its taken inputs.
- `statics`: `lower` encodes every constant a unit reads (`ReadStatic`) from
  its materialized value as `hirsel_lir::statics` blocks at its layouts:
  scalars, aggregates, variants (tag form included), inline repeats and Simd
  lanes in place, a block reference or a String or Bytes slice as a
  reference cell to a block of its payload. Maps and sets store ordered entries and an empty index as static sequence blocks. A flat constant is one pointer-free block sized from its materialized fields and elements; dynamic prefixes follow the existing layout. A callable, a non-flat Packed extent and a handle are `StaticError`s, never a guessed image.
- `liveness`: backward value liveness over normal and failure edges, for RC
  insertion; validators never read it.

`cargo test -p hirsel-pipeline-tests --test rc rc_import::corpus -- --ignored --nocapture` lowers
every accepted conformance program and groups what does not validate (`ALL=1`
names every program of a group);
`rc_import::inspect` with `SRC=<file>` prints one program's units.

## Task F: placement and Perceus insertion

- `placement::place`: Value to Placed. TIR `Destination` is an obligation:
  `Heap` stays on the task heap, `Caller` becomes the hidden ABI destination,
  `Inferred` is placement's choice, `Arena(origin)` is
  `Fixed(Arena(i))` with `tables.arenas[i]` the slot holding the token, and
  `arena.with`'s `ArenaOpen` becomes a region entry at the unit's start, its
  `ArenaToken` (with the opening's budget) in the opening's place and an
  exit on every return and propagation. Call sites into a callee
  with a hidden destination name `Heap`. With `Options::regions` (and always in
  a `noalloc` function, MEM-5) an inferred block whose flow class never leaves
  the frame goes to the unit's internal region: the class is retyped
  uncounted, its drops go (the region exit discharges them), its copies become
  plain reads, each block is published as soon as it is built, the region is
  entered at entry and exited before every propagation and after return preparation.
  An owned result remains in storage until exit succeeds; exit failure cleans
  it after frame cleanup. A
  heap block a `noalloc` unit cannot avoid is `PlaceError::HeapInNoalloc`.
- Insertion splits normal predecessor edges when silent content dies at a
  join, placing its drop before moved masks merge. Flow tracks general absent
  parts and actual transfers; it has no pattern-specific owned-empty cache.
  Early releases skip sealed Unreachable blocks, preserving the adjacent
  exhaustive-switch proof of these impossible defaults through cleanup.
- `insert::insert`: Placed to Rc. Every slot whose content has no
  user-visible destruction is released at the last use of that content on
  every path, normal and failure (uses include `EndLoan` of a loan on it and
  views of it); a semantic copy is the Dup of a non-final consumer. Releases
  supersede the TIR drops they make unreachable, version joins of released
  content are removed, and each early release gets a synthesized failure
  cleanup of what is still owned. A shared failure-cleanup block whose
  incoming paths disagree, after an early release, on whether a slot it or a
  later block drops is still full is first copied once per state they agree
  on, so each copy keeps or deletes the drop as one site's own chain would. Borrowed, `mut`, capture and return slots and
  destructible content keep their TIR drops.
- `insert` first makes written storage unique (MEM-1, OWN-7): before an
  address that a store, replacement, mutable loan or `mut` argument writes
  through a counted block is prepared, each block on its path is tested with
  `IsUnique` and, when shared, replaced by a shallow `UniqueCopy` (outermost
  first; the slot's version joins after the test): the block alone, its
  counted children shared with one more holder each (OWN-4), so a child is
  detached in turn only when a write reaches it. The test also goes above the
  preparations just before it of other places located through those blocks,
  since a detach moves them (C's `StaleAddress`). A block an active mutable loan
  lends was detached where the loan began. `Proofs` (from the instance's
  callable type) skips `unique` parameters and `noalloc` bodies. A block
  whose only write is a `SeqReserve` is not detached: the reserve copies a
  shared or static block once at its requested capacity.
- `import` lowers an empty sequence construction (`[]`, and the sequences of
  `map.empty`/`set.empty`) on the heap to a `ReadStatic` of the static empty
  sequence of its payload layout (`import::empty_sequence`, instance: the
  layout record's digest), whose zero count `Lowered.statics` carries. It
  allocates nothing; its first reserve or growth allocates.
- `elide::elide`: Rc to Rc. A copy immediately followed by insertion's release
  of the same slot cancels into a move (a partial copy into a partial move
  plus the rest's drop). Flat copies stay; nothing allocates.
- `insert` also holds the shared machinery: layout queries, place
  flattening, the unit `Editor` (finish drops unreachable blocks, unnamed edges
  and failure edges, undefined values and undischarged anchors, and rebuilds
  the switch-arm pool from the switches it keeps, so no arm a rewrite replaced
  keeps a pre-compaction edge id), forward
  ownership state and slot-content liveness. They are this crate's own
  analyses; the validators replay ownership independently.


## Task G: drop specialization and reuse

`reuse::reuse(unit: LirUnit<Rc>, table, &Options) -> LirUnit<Reuse>`.
`Options { reuse: false }` is the reuse-off switch: the unit only changes
stage. With it on, three rewrites run in order:

- `specialize_drop::specialize`: a run `address d.f..; copy d.f..; release d`
  of a counted owned block whose copied children are counted blocks becomes
  `is_unique d` and two paths joining with the children as parameters. The
  unique path takes each child out (`read take` through `Value(d)`) and
  releases `d` under a static mask without them: no Dup, and the layout's own
  drop program for what stays. The shared path copies and decrements. A
  failure cleanup naming a child is cloned per path with the child renamed;
  any cleanup the clone cannot express leaves the run alone.
- `reuse::pair_tokens`: buckets are exact `(usable bytes, alignment, domain)`
  of fixed heap payloads. A release (or a whole closure-slot drop, by the
  environment layout of the `Closure` that built it) of a counted, owned,
  non-static heap block becomes `Reset` when a construction of its bucket is
  anticipated; a construction whose bucket has a live token becomes `Reuse`,
  every source operand having been evaluated before it. A token is disposed
  as soon as no construction of its bucket is anticipated, on every return,
  propagate and unreachable exit, and in the dispose list of every failure
  edge it is live across. No token crosses a loop-forest feedback edge
  (irreducible cycles included). A split gives each edge its own continuation
  (`split` blocks); a join takes one token parameter per bucket, an edge
  without a token passing `TokenNone`. The release on a specialized shared
  path never donates.

An empty closure environment is static even at a heap placement site and never
donates storage. Its ordinary callable drop remains in place.
- `reuse_stores::keep`: a `Reuse` keeps (`kept`, LIR schema revision 5) each
  field whose operand is the take of that same field of the reset donor, of
  the same layout and active variant, through token joins edge by edge, the
  evidence C requires.

`hirsel-pipeline-tests/tests/rc/rc_drop_specialization.rs` builds Rc units the source cannot state
(children of boxes, variants and records in boxes, irreducible loops);
`hirsel-pipeline-tests/tests/rc/rc_reuse.rs` runs driver-lowered programs. Both check every unit with C
before and after, run every mode with every failure-capable site injected on
the heap executor, compare reuse-on and reuse-off observations, and witness
reuse, elided dups and kept fields through counters and the units.

## Task K: scalar legalization

- `legalize`: a call of a scalar library operation (native disposition
  `Instruction`) becomes its instruction (LIR schema revision 6), chosen
  from the call's scalar argument and result kinds; the Ready validator
  rejects any such call left. `int.narrow` is a `TryNarrow` `Convert` and a
  SEM-1 `Check` of its flag at the call's site. `int.try_narrow` and
  `int.checked_add` branch on the flag into `Some(result)` and `None`, and
  `float.from_int` is an unflagged `IntToFloat` conversion,
  `f32.bits`/`f64.bits` and `f32.from_bits`/`f64.from_bits` are unflagged
  `Reinterpret` conversions, and
  `float.to_i32` switches on its status into `Ok(result)`,
  `Error(NotFinite)` and `Error(OutOfRange)`: inline constructions joined in
  a block whose parameter is the call's result. Ordinary std reductions stay source calls.
- `hirsel_lir::ir::Inst::defs` lists the result and the flag of `Convert` and
  `IntArith`; renumbering and liveness read definitions only from it.

`hirsel-pipeline-tests/tests/rc/rc_legalize.rs` runs source programs for every operation at every width
through the pipeline: C accepts each stage, and the heap executor and the
interpreter agree on results and SEM-1 failures.

`arena::COPY_TO` builds a deep `UniqueCopy` with the allocator parameter as its
construction destination. `arena::HEAP` imports the inert counted-heap token as
`ConstantValue::Heap` (LIR revision 25); scoped tokens still come from `ArenaOpen`.
`arena::BOUNDED` (MEM-6) reads its capacity in the entry block, opens the arena
with it as the budget and calls the callback; the callback's result becomes
`Ok`, and its failure path ends the loan and then `Recover`s the arena: the
arena's own `OutOfSpace` returns `Error(OutOfSpace())`, every other failure
propagates.

`isolate::ISOLATE` (FAIL-2) reads the task's failure record count, moves its
`take` callback into a temporary and calls it: its result becomes `Ok`, and its
failure path ends the loan and then catches (`Isolate`, LIR revision 31) every
failure since that count. The caught chain becomes `Error(Failure)`, built in a
loop from the last record to the primary, each record but the primary boxed as
the cause of the one before it.

`shared::FREEZE` moves its parameter into a root (`Freeze`). `shared::THAW` tests its
owned root's count (`IsUnique`): the last root thaws (`Thaw`) into `Ok`, any other
root is returned as `Error`. A `Shared` payload is never a reuse donor.

## Native library (LIR schema revision 7)

`library` holds the core library's generic operations as LIR.
`INLINE` rows replace a call during import
by instructions (`list.len`, `array.len` and `string.byte_len` are a borrow
and `SeqLen`); `BODIES` build one `LirUnit<Value>` of `UnitOrigin::Library`
per (operation, instance), and the call becomes `Callee::Direct` to it.
`Instances` routes a `BODIES` callee to a `InstanceRole::Library` instance and
`import::lower` builds each body's unit through its worklist, so a body that
calls another body or a `$native` finalizer gets it too. `library::owners()`
supplies the `$native` cursor owners (`OwnerRepresentation::Cursor`).

`library::codec` lowers `codec.encode` and `codec.decode` inline (LIR schema
revision 29): it takes the call's concrete schema from the checker
(`serializable_schema`), pairs each schema node with the native layout of the
value it describes into a `CodecPlan` (memoized per schema node and layout,
nominals over records, variants and nonempty tuples sharing their layout, Packed
over its inner variant's arms), checks it against the table and emits
`Inst::Encode` or `Inst::Decode` over a read-only operand. Variable flat values
use explicit block, length framing and text nodes;
flat lists distinguish byte extents from element counts. Sequences retain their
ordinary, sequential Packed or SoA storage kind in the checked plan. The runtime
and H walk fields and active extents rather than copying native storage to wire.

A variant pattern reads its tag through a synthetic loan only when the
variant's type owns counted storage; an uncounted variant that is not plain
bits (one holding a fixed flat record) is a plain borrowed read.

A body is written against `builder::Body`, which prepares each parameter in
an entry prologue (a `take` parameter is held in a slot with a cached failure
cleanup that drops it). Rules every body keeps:

- Mutation bodies never test uniqueness. Sequence consume open queries its
  owned source once and stores the result; next moves on the unique path and
  copies on the shared/static path, with no detach or allocation. Insertion's
  write collection (P4) treats
  `SeqReserve`, `SeqSetLen`, `SeqAppend`, `FlatAppend` and a take through a `Slot` step as writes of the
  block at their place. A shared or static block detaches where its write
  address is prepared: after the prologue for parameter writes, or later for
  an append prepared after growth. A place whose only
  write is a `SeqReserve` (`list.reserve`, `$native.list_grow`) is not
  detached: the reserve makes its block unique at the requested capacity.
  `list.reserve` of a capacity whose storage would exceed `runtime_abi::EXTENT_MAX`
  (beyond any host's address space) reserves nothing, as the interpreter's does;
  capacity is unobservable, and `SeqReserve` itself keeps an overflow an invariant fault.
- A `Slot` step names an element slot below the capacity without a count
  check; the body proves the bound (C cannot), and the heap checker fails an
  unwritten read, a length over an unwritten slot or past the capacity, and a
  length that abandons a written slot.
- `library::list` implements `list.push`: read count and capacity, call the
  private `list_grow` body when full (reserve `max(4, 2 * capacity)`), prepare
  its append write address, move through `Slot(len)`, then `SeqSetLen(len + 1)`.
  Preparing the write after growth lets a static empty list grow directly;
  P4 still detaches a shared list on the spare-capacity path. `forget` imports as
  `Inst::Forget`, consuming its operand without finalizer or field cleanup.
  `hirsel-pipeline-tests/tests/rc/rc_library_list.rs` executes these operations against the interpreter,
  audits every injected heap failure, and checks both native targets. Its
  million-append probe asserts allocation and relocated-byte bounds.

Library finalizers receive `mut self`, with their new storage version reaching
both the normal and failure continuations before field cleanup. Source drop
bodies retain borrowed self. The hand-built control in `hirsel-pipeline-tests/tests/rc/rc_library_list.rs`
mutates a field and verifies that its counted sibling is released once.

`ListConsume`, `ArrayConsume` and `InlineConsume` are library bodies. Inline
open moves into one allocated Array; the other opens move the existing block
with its uniqueness flag. The unique cursor finalizer drains the unreached
suffix last first and empties its source; shared cursor cleanup releases its
source credit. `hirsel-pipeline-tests/tests/rc/rc_library_list.rs` covers break, body failure and failing
element finalizers through H's complete fault campaign and both native targets.

Cleanup decomposes record and variant fields by declaration ordinal, in reverse
order, preserving distinct zero-sized resource obligations even at equal offsets.
Its intact Variant and Inline actions move the owner into temporary
storage, prepare all payload addresses before finalization, and share the
same suffix cleanup chains. Variant selection reads TagForm (including a
null-pointer niche); Inline elements finalize in reverse constant-index order.
A failed preparation retries while the owner is whole; a failed finalizer
continues with the remaining fields and elements. The `rc_cleanup_variant` and
`rc_cleanup_inline` pipeline tests validate C and H on both call outcomes.

Copy-on-write also detaches a block where a shared view of it is prepared
when a write while that view is active would detach the same block (OWN-3: a
call argument views `xs[i].name` and a later argument writes `xs[i].n`). The
write then finds the block unique and the view keeps reading live storage.

A closure body's captures are its caller's environment, lent for the call:
a take of a copyable capture (a `once` closure's `take x`) copies it, TIR's
drops of capture slots are skipped (the environment releases them), and a take
of a noncopyable capture moves it and clears its persistent environment live bit. A many-call
closure returned as a `once fn` is weakened like one stored into once storage.

A copyable part taken out of a temporary's block (an element of a call's
list result, or of a block expression's local it extends) is copied, and the
temporary is dropped whole; elision keeps such a copy and its release. A take
would write through a block that may be shared (`WriteShared` in the heap
checker) and leave a hole its drop cannot skip.

Drop flags (LIR revisions 8 and 13):
when content must survive a join with different moved parts, `join_flags` adds
bool SSA parameters for the whole slot or individual inline parts. Edges pass
true, false or the current flag. Cleanup branches on each flag before preparing
and dropping that part, preserving reverse field order and failure cleanup.
Field flags stop at counted storage; a Box retains its container ownership
regardless of which payload variant is active.
Variants use ordinary absent-field masks, and fixed inline elements are handled
individually. There is no moved-on-some-paths Unsupported case. C's existing address-layout
checks exclude partial transfers from scalar/block-reference frame storage, and
its finalization proof excludes taking a field from a live resource with a
finalizer. Those are malformed input invariants, rather than cleanup limits.
C independently
proves edge truth and currency; H checks field flags against its physical ledger.

`join_versions` joins reaching content versions; empty edges may name any declared
version, including one defined later, while initialized bytes keep their current
version even after their inline finalizer has run.

It builds dominance and incoming-edge indexes once, then shares one solve and one
slot-row replay across joins of distinct slots. A join changes that slot's version
transfer, so another join of the same slot uses a fresh solve. Edge snapshots retain
only persistent slot rows. `RcCleanupVersionRows` counts instruction and terminator
rows transferred by cleanup's version-state solves and replays; the driver's
`it-scaling` regression bounds independent joins without using elapsed time.

`library::iter` constructs `Iter` values as closures owning state and callback,
with compiled `iter_from_step` and `iter_map_step` bodies. `iter.next` and
iterator loops call the step indirectly through the exclusive iterator place.
Captured state writes survive callback failure; mapped items belong to the
adapter before its mapper runs and finalize before the remaining iterator state.
`hirsel-pipeline-tests/tests/rc/rc_library_iter.rs` checks all fourteen iterator conformance programs,
injected failure cleanup, finalizer order and both native targets.

An erased callable or iterator environment may own resources, so Perceus keeps
its drop at the source cleanup point. A callable with a known environment is
classified by that payload layout, including environments nested in aggregates.

`library/views.rs` lowers string.scalars and string.positions
inline to Copy at the destination type. Their StringView owner representation is
an exact layout alias of String, so no retagging instruction is needed. Unicode
protocols use a private ViewCursor (borrowed Slice plus relative position), while
collection cursors keep Cursor. hirsel-pipeline-tests/tests/rc/rc_library_views.rs checks heap failure audits,
compiled early exits and ordinary source grapheme segmentation over all official
Unicode16 vectors on both native targets. GraphemeView has an ordinary source
representation and association; it uses no native view registration.

`library/sorted` lowers empty and length inline, builds binary-search insertion,
consuming SortedMap cursors and the ten integer Order comparators. Comparison
probes match the interpreter (spec #94) and finish before mutation; equal insertion
replaces only the value and drops its displaced owner. Cursor field 2 saves the
conjunction of outer/entries uniqueness; shared sources copy, unique sources move
and drain their remaining suffix in reverse. Integer `.order` constants lower to
a FunctionRef and inline Order construction, with no callable static-data format.

`library/value.rs` gives an operation without a library body a function value
(`let f = out.print`): one forwarding body per operation, instantiated at the
operation's own path and actuals, passes each parameter on as declared and calls
the operation, so a call through the value fails and cleans up as a direct call
does. Of the inline lowerings only the lengths have a value (their `SeqLen`); the
others are unsupported as values. `native-tests/regressions/operation-as-value.hir`
runs it in the native smoke.

`flat_walk` strength-reduces zero-based, unit-stride loops over variable-element
flat sequences and sequential `Packed` sequences (MEM-13, offsets from the first
element) at Value, after import has sealed the layout table. Its public
`discover`, `prove` and `commit` phases keep unsuccessful matches read-only.
A checked element prepare must dominate every feedback path and every rewritten
read; writes or exclusive access to the sequence decline. Breaks and continues
after that prepare are safe. A nested walk gets its own cursor; the outer loop
conservatively declines. The loop-carried offset starts at zero, moves into an
immutable value at the first prepare, and receives `FlatNext` on feedback.
`import::lower_with_options(..., &flat_walk::Options { enabled: false })` keeps
the original Index projections for the driver's differential matrix.

Import builds flat lists in place (`import/flat_builder.rs`, LIR schema
revision 22). A local `List[T]` of a variable flat record `T` is a builder
web, with the temporaries it moves through, when its only other uses are
storage and drop events, `list.reserve`, `list.push` of a `T` constructed in
the push's own block and only moved on the way, and one `take` into a flat
field encoding a list of `T`. Its locals take the builder layout; the element
construction becomes `FlatAppend` into the list, the push is removed, the
reservation becomes the builder's `SeqReserve`, and the taking construction
copies the builder whole. Any other
use (a read, a copy, a named or joined element, another call) leaves the
list ordinary. `hirsel-pipeline-tests/tests/it/rc_flat_builder.rs` holds
accepted and declined shapes to the interpreter under H's fault campaign.

A byte list frozen into a builder's element is written there
(`import/flat_field.rs`, revision 24). A local `List[u8]`, with the
temporaries and the `Bytes` it moves and freezes into, is built in place
when it is initialized once to `[]`, otherwise only `list.reserve`d and
`list.push`ed, frozen whole by `bytes.from_list(take ...)` and the Bytes
taken into one field of an element construction of a builder web, that
field a byte sequence after only fixed fields, and every path from the
initialization reaches that construction without another write of the
builder, another initialization of a list for it, a return or a tail
site. The initialization becomes `FlatField Open`, the reservation and the
pushes `FlatField Reserve`/`Push` (their calls removed), the freeze a jump,
and the construction `FlatClose` lacking that field. The list has no slot,
block, copy or release. `spare_push::prove_fields` marks a push `spare` under
the same counted-loop proof, the opening standing for the empty list and
the field reservation for `list.reserve`. `hirsel-pipeline-tests/tests/rc/rc_flat_field.rs`
holds accepted and declined shapes to the interpreter under H's fault campaign.

`spare_push` removes capacity tests it proves cannot fail (FACT-3). Right after a
source unit is imported, before the layout table is sealed, it retargets a
`list.push` to `$native.list_push_spare`, the append without the capacity read
and growth branch, when the push lies in a natural loop with one feedback edge,
the list's version entering the loop is that of `list.reserve(m)` of the static
empty sequence, the loop's u64 index starts at zero and grows by one, a branch
on `index < m` (or on a constant no larger than a constant `m`, or on the
length of the slice whose length `m` is, read again from the same version, as
`list.reserve(mut xs, bytes.len(data))` then `for i in 0u64..bytes.len(data)`;
a sequence block's count can change between reads and never qualifies) guards the push,
`m` is defined outside the loop, the push lies on no inner cycle and it is the
loop's only reference to the list. At most one push runs per iteration, so the
count before it is below `m`; a detach keeps capacity. Trivial block parameters
are forwarded to a fixpoint. A push instance no call names any more is not
built. `hirsel-pipeline-tests/tests/it/rc_spare_push.rs` checks proven and
declined shapes against the interpreter under H's fault campaign, and a
reservation forged smaller fails H's slot bound; `hirsel-driver`
`tests/it/growth_ir.rs` checks the steady iteration's single exit test.

`closure_calls` follows callable code identity through whole SSA storage,
copies, weakening and equal-target joins. Mutable aliases and projected/dynamic
loads stay conservative. Known literals use Bound calls with their original
environment; known function references call their Definition entry. Source
helpers specialize by callable argument targets, retaining the declared ABI,
environment operands and all source cleanup/failure anchors. Limits are 16
target vectors per original instance and 256 added units per program; beyond
either, the original indirect entry remains. `Lowered.unit_origins` names the
original signature/placement proof owner of every added unit, while
`inline_instances` identifies bodies available in each calling partition.
`hirsel-pipeline-tests/tests/rc/rc_flat_walk.rs` includes C forgeries, early exits and injected H failures;
100,000 variable records require exactly 100,000 H element-prefix reads, while
the disabled 40/80-element walks require 3,120/12,640 reads.

`library::map` owns Map/Set construction and keyed operations. Field 0 is insertion-ordered entries; field 1 is a private u32 ordinal index, empty through eight entries, built or rebuilt by private slow-path bodies at 7/8 load. Every keyed call hashes before probing, including an empty container; reads never write or build an index. Replacement installs the new value before dropping the displaced one, then drops the taken incoming key. Consuming Map cursors save source uniqueness at open, move unique entries and copy shared/static entries without allocation; finalizers drain unique remaining entries backwards through temporary holders. `hirsel-pipeline-tests/tests/rc/rc_maps.rs` audits clean and injected failures, and `hirsel-pipeline-tests/tests/rc/rc_maps_native.rs` checks compiled seed placement, internal Set ordering and source failure attribution on both targets.

Defuse handoff: Flow records per-path `fields_pending` without
changing versions or field ownership. The planner omits the parent's Finalize
and drops only remaining fields. A retained defused shell stays within its
straight-line distribution region; normal and failure paths empty it before a
join. The new opcode owner fills the marked Defuse arm beside FinalizeEnd and
independently checks that region invariant in C; no phase guard is introduced.

Wave-1 list pop/remove/swap/reserve are generic library bodies in library/collection.rs. Bounds are checked before mutation; remove rotates the selected element to the tail before extraction, keeping every failure-capable address step over initialized storage. library/failure.rs implements the finite CheckRule switch at the source call anchor.

Import maps TIR Box `Field(0)` to an explicit native `Payload` step; native
Box storage contains the payload directly. Nested steps retain each physical
Box, and writes or takes through counted storage use the ordinary COW pass.
Cleanup releases a partially moved Box with a mask of its remaining fields.

Taken flat construction sources move through construction-owned temporary slots
before encoding, preserving failure cleanup order ahead of retained source
copies. Prepared sources and remaining sources after a failed release drain
through explicit cleanup; the completed flat result stays pending until then.

Static empty sequences contain their complete zero prefix, aligned for the first
element: a count word for ordinary storage, count plus used bytes for sequential
Packed storage. LLVM and H retain that exact extent; no heap-class padding is
required to supply header or alignment bytes.
`arena_credit` derives result-arena summaries across the whole imported call graph, after callable specialization, and placement selects `ConstructSite.arena_no_drop` only when every owned payload leaf is proven same-arena or needs no cleanup. Heap/unknown operands retain dynamic checks; C derives its own placement proof.

`bytes.from_list` transfers its owned byte sequence into a Slice through `SeqLen`
and ordinary construction, with start zero and the exact owner length. C checks
the byte payload and interval; no descriptor query, allocation, retain or runtime
call is needed.

Production native import uses `hirsel-layout` for type-to-layout and ABI derivation.

Native import, library construction, RC insertion/elision, reuse, cleanup and
legalization share `PassError`. Unsupported and malformed failures retain their
message as text; instance and layout errors retain their structured payloads.
The driver supplies stage and definition attribution through Display at its six
pass boundaries. Ready returns the same error without duplicate wrappers.

Nested constructions join the same placement class through their nontrivial
fields and projected extractions. Inline containers use `Uncounted` at their
existing inline layouts; their copies and ownership takes become plain reads
and their drops disappear. A foreign child or escaping container keeps the entire
class counted. Flat encodings read their inputs without retaining references,
so they do not join those inputs' placement classes.

Slots use library-owned `List[(u64, Option[T])]` storage and two-word
`SlotId` values (index, generation). Insertion scans reusable vacancies before
growing; removal transfers the Option and advances its generation.
Generation zero is reserved: the first id uses one, and removal at the maximum
wraps to zero and retires that vacancy. Reads check both words before observing
the payload. Ordinary placement, COW, counting and drop passes own all storage.

Computed borrowed views (index or variant projections) materialize at their
binding, keeping the parent check at its own source obligation before any child
projection. Only total field paths flatten through a view alias.
