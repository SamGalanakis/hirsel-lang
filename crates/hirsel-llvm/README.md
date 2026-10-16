# hirsel-llvm

The LLVM backend: emission of validated Ready LIR units through a
small RAII layer over the LLVM C API, returning an owned `hirsel_lir::ObjectArtifact`.

Allowed dependencies: the shared data crates `hirsel-lir`, `hirsel-core-contract` and
`hirsel-syntax`, `hirsel-span` (the existing Unicode-aware source-line index), and `llvm-sys = "=211.0.1"` with `strict-versioning` and `force-dynamic`,
bound to the shared libLLVM 21.1.8 pinned in `tools/native-toolchain/llvm-21.lock`. No
Inkwell, no other implementation crate. Test-only: `hirsel-types` and `hirsel-tir` (fixture
type stores and representation records). `.cargo/config.toml` points `LLVM_SYS_211_PREFIX` at
the pinned install; an environment value wins, for example a local
`tools/native-toolchain/install --unpack` prefix.

Interface:

- `emit_object(&[&ValidatedUnit<Ready>], &EmitInputs) -> Result<ObjectArtifact, CodegenError>`.
  `EmitInputs` is immutable: the front-end `InputIdentity`, `TargetSpec`, `LayoutTable`,
  operation `Manifest`, `FailureCodes` and `OptLevel`. No provider, query engine or linker.
- `check_target(&TargetSpec)`: the loaded libLLVM is the pin and LLVM's DataLayout agrees
  with the target spec.
- `compile_ir(&str, &TargetSpec, OptLevel) -> ObjectFile`: the toolchain smoke path for
  textual IR, through the same verifier, optimization and object stages.
- `symbol_name(DefPath, Option<instance>)`: the injective, C-identifier-safe link name of a
  definition instance. `site_code(DefPath, revision, SiteRange)`: the `u64` source site a
  failure passes to `hirsel_rt_v2_failure_begin`.

Invariants:

- No raw LLVM handle is public. `context.rs` owns every handle (`Context`, `Module`,
  `Builder`, `TargetMachine`, and the buffers and messages LLVM returns) and disposes of each
  on drop; types, values and blocks borrow their module, so nothing outlives its owner.
- Emission refuses a libLLVM other than `PINNED_LLVM`. LLVM's DataLayout (string, byte
  order, pointer width, each scalar's size and ABI alignment) is cross-checked against the
  `TargetSpec`, never adopted.
- A unit is emitted only if its receipt binds this unit, the Ready stage, this LIR and
  validator version, and exactly the target, layout table, manifest and input identity
  emission was given.
- Checked arithmetic: integer `+ - *` and signed negation use the overflow intrinsics;
  division, remainder and shifts are guarded before the instruction, so no path reaches
  poison or undefined behaviour (signed `MIN % -1` is 0, SEM-1). Floats are single IEEE
  instructions with no fast-math flags; `MulAdd` (`llvm.fma`) is the only fused
  operation.
- The scalar library instructions (LIR schema revision 6) are native: `Convert` is
  `trunc`/`sext`/`zext` with a range compare (`llvm.fptosi.sat` and ordered compares for
  `float.to_i32`); `float.from_int` is `sitofp` or `uitofp` into its destination width;
  `Reinterpret` (`f32.bits` and its siblings) is a `bitcast` whose NaN self-compare selects
  the canonical NaN's bits.
  `IntArith` is `add`, `llvm.{s,u}add.sat` or `llvm.{s,u}add.with.overflow`,
  the named divisions guard a divisor made safe, `min`/`max` are `llvm.minimum`/`llvm.maximum`
  (the `_number` forms select a NaN operand's other operand around them) and `total_order`
  compares sign-magnitude keys of NaN-canonical bits. LLVM may lower `llvm.fma` and
  128-bit integer-to-float casts to target support routines. `tests/it/scalar_library.rs` holds each to the
  interpreter's kernels and the heap checker at every width and edge, on both targets.
- The native library's shapes (LIR schema revision 7): `SeqLen` loads a sequence
  block's count (or a slice's length word), `SeqCapacity` reads slab/prefix/immortal extent metadata inline and subtracts the elements' offset, by the stride, never below the count (zero-sized elements
  report `u64::MAX`), `SeqReserve` calls `hirsel_rt_v2_seq_grow` by root value (which detaches a block that is not unique at the requested capacity) and writes its returned, unique root into the place,
  `SeqSetLen` stores the count, `SeqAppend` (revision 19) copies its element's active extent
  past the used bytes after `hirsel_rt_v2_block_grow` doubles full storage, `FlatAppend`
  (revision 22) sizes its construction's parts, grows the same way, then writes the content
  length and the parts in place, a flat list builder source is copied whole (its count, then
  one memcpy of its used bytes behind `8 + used`), except that a heap record whose one field is
  a builder it borrows, then takes and releases with nothing between, takes over a unique
  builder's block (its two header words become `8 + used` and the count, its descriptor index the
  record's) and skips that release, a builder's `SeqReserve` grows it once for `capacity` of its
  least element, a `Slot` step is an unchecked element address, `FlatAt`
  checks its index against the flat or sequential count (MEM-14) and offsets from the elements' start
  (a sequential Packed sequence's first element, MEM-13),
  `FlatNext` is the element's end less that start, and `Recast` emits nothing. `Hash`
  spills its operand, takes the run's key state from `hirsel_rt_v2_hash_begin` and calls
  a private per-layout `hirsel.hash.<L>` (`compare.rs`, beside `hirsel.compare.<L>`) that
  folds each scalar leaf through `hirsel_rt_v2_hash_bytes` (floats with -0 as +0,
  variants by tag, sequences by count then elements) and sets a flag past
  `COMPARE_MAX_DEPTH`, which takes the failure edge (MEM-3 Depth). Maps hash their entries in order; sets combine element hashes commutatively. Their comparators read entries, with sets sorted by structural order without allocation. A `UnitOrigin::Library` unit is `linkonce_odr`
  and `inlinehint` in every object that defines it and is no export of the object, so
  its copy may vanish once every call is inlined. Definition-shaped library
  bodies also have a callable thunk with that linkage, so FunctionRef addresses
  resolve in every object (the integer Order dictionaries exercise this). `tests/it/ready.rs` runs a library push
  (`seq_push`) and a flat walk (`flat_walk`) on both targets at O0 and O3.
- A call to a `RuntimePrimitive` operation imports the symbol its template resolves to for
  the call's scalar kinds (`hirsel_lir::abi::resolve_call_symbol`); a `Instruction`
  operation is never a call at Ready. GEPs are not `inbounds`; no `noalias`, `nonnull` or dereferenceability
  attribute is granted.
- Runtime declarations carry `nounwind` and explicit memory effects. Audited scalar
  queries/hash leaves additionally carry `captures(none)` and `willreturn` where
  their Rust bodies prove those promises (`runtime.rs`). Allocators retain pointers
  and cleanup can invoke diverging user callbacks, so neither promise is blanket.
  `hirsel_rt_v2_alloc` (a `noalias` return, `allockind("alloc")`) and `hirsel_rt_v2_free`
  (`allockind("free")`, its block `allocptr`) are one `alloc-family`, as C's malloc and free:
  allocation cannot fail and no program observes it. After each alloc the emitted code
  stores the block's count, the alloc postcondition of one holder, so a block that never
  escapes has its uniqueness tests and release folded and is deleted with its free
  (`runtime::allocator`; `hirsel-driver`'s `tests/it/alloc_elision_ir.rs`).
  TBAA separates live holder counts, sequence header lengths and scalar
  accesses within proved block payloads. Unknown pointer accesses stay unclassified. Element addresses start after their length; scalar source projections
  cannot reach either header. `SeqSetLen` and glue's count stores keep the matching
  tag, while aggregates, copies and arbitrary runtime calls remain conservative.
- Status edges: every Hirsel entry returns a `u8` status. A failure edge that starts a rule
  calls `hirsel_rt_v2_failure_begin(ctx, kind, rule, site)` with the driver-assigned codes
  and the anchor's site code, disposes of its listed tokens and enters its cleanup block;
  `Propagate` returns 1. A rule without a code is an error, never an invented code.
  A diverging call (LIR revision 19) is not `noreturn`: its failure still returns
  status 1. A zero status branches to `unreachable`; nonzero takes the failure edge.
- ABI v1 (`FnAbi`): context, [environment], [caller destination], [result pointer], then
  each declared parameter's physical slots. Emitted code provides the declared scalar
  extension (call-site attributes) but never assumes it from a caller or callee, because
  AAPCS64 C code does not extend narrow integers.
- Register results: a Hirsel entry whose result is a plain scalar of at most 64 bits (an
  integer, Bool, float, Path, arena token, or block, box or header reference) has no result pointer and
  returns `{ i8 status, word }`, the word `ptr` for a reference and `i64` otherwise. On
  success the word's low bits are the value's (Bool as a byte, floats by bit pattern),
  zero-extended by emitted code and never assumed; on failure it is zero. x86-64 returns
  the pair in AL and RDX, AArch64 in W0 and X1: the C struct `{ uint8_t status;
  uint64_t value; }` (or `void *value`). Definitions, function-reference thunks, closure
  and iterator bodies all follow it, so direct, private (`.roots5`, as one more word after
  the roots) and indirect calls agree and an erased call keeps no result cell. 128-bit and
  aggregate results, and runtime primitives, keep their result pointer. Every return of
  such an entry branches to one return block where status and word meet in scalar phis
  and the pair is built once: neither return merging nor the inliner then forms a phi of
  pairs, which LLVM does not split, so an inlined callee's constant statuses reach the
  caller's test and thread as a bare status does. Failure sites,
  cleanup and SEM-1 are unchanged. `hirsel-driver`'s `tests/it/scalar_return_ir.rs` checks
  the erased witness in optimized IR on both targets. `register_result` is the rule itself, and
  `REGISTER_PAIR` with `PairField::word` the pair's fields, from which emission builds the returned struct
  and `c_entry` writes the C typedefs (`c_pairs_are_the_emitted_pair_field_by_field` pins both). No C harness
  restates it: `c_call(layouts, abi, symbol, name)` declares an entry as emission lowers it
  and defines `static uint8_t name(...)` of its ABI v1 signature (result pointer included)
  calling it, and `c_definition(layouts, abi, symbol, body)` defines a C-implemented entry
  over a body of that signature. Every harness calling or defining an entry (`tests/native/*.c`,
  the generated harnesses in `tests/it/`, `hirsel-pipeline-tests`' `rc/maps/seed.c`, and
  `hirsel-rt`'s `harness/binarytrees.c` through the binding header `glue.rs` writes) goes
  through them, so a rule change rewrites the harnesses with it.
- A `Packed[T]` value is stored at its largest variant (`SizeSpec::storage`), as an
  ordinary variant is; a value whose size reads its own lengths (variable flat) is not a
  stored value and is refused. A whole Packed value is loaded through a stack copy of its
  active extent, and a sequential sequence's elements (MEM-13) are stored back to back at
  theirs; `Index` into one walks past each earlier element's extent.
- `OptLevel::Speed` runs LLVM `default<O3>` with Aggressive code generation;
  `None` remains O0. The LLVM verifier runs before and after the optimization pipeline; each failure reports
  its stage. `Unreachable` traps rather than letting the optimizer assume.
- The object's ELF symbol and relocation tables are read by `object.rs`, independently of
  LLVM, after every LLVM handle is dropped. Exports must be exactly the declared source entries,
  excluding replicated library and known callable bodies;
  imports must be declared callees, runtime entries, or `TOOLCHAIN_IMPORTS` (C library and
  128-bit division and integer-to-float support routines). `ObjectArtifact` owns its bytes and borrows nothing;
  emission is deterministic.

Status (phase 4): scalars, control flow, block parameters, slots and addresses
(field, variant-tag-checked and bounds-checked index projections, block payloads, `@packed`
unaligned access), inline, heap and local-region block constructions (records, variants,
boxes, sequences, Simd lanes), `Release`, `Reset` and `Reuse`, direct, indirect and
runtime-family calls, `mut` arguments by exclusive address, failures and returns.
A `Reuse` with a non-null token neither zeroes nor stores its `kept` fields, whose bytes the
donor's moved-out children left in place; a null token stores every field. A field is
moved out of an owned block by `Read Take` through the block's value, and the block is then
released or reset under a static live mask (a private `hirsel.mask` constant). `Reset` of a
callable resets its environment block. Drop glue (`glue.rs`) lowers a LayoutRecord's drop program to straight code. A `Release`
of a whole block whose payload layout has no finalizer, inline resource or columns calls
the private `hirsel.release.<layout>(ctx, block, anchor, budget)`, one per layout and
object: it consumes one credit on the count word (the contract in `hirsel_rt_v2.h`; T04,
T16), and when that was the last it walks the payload, tags switched on, static repeats
unrolled or looped, sequences looped from the highest element, then frees the block
through `hirsel_rt_v2_free`'s fast path (`runtime::free_fast`), onto its page's deferred list
when the context allows: post-order, as the runtime's worklist frees. A layout with an
empty program has no function, only the inline count test and the free. A child whose
layout the record fixes (a `Block` field) is released by its own function, by native
recursion under a budget of 64 nested frames, with the next sibling's header prefetched; a
block that dies with the budget spent, a slice's owner, a callable's environment and a
block of a layout without glue go to `hirsel_rt_v2_release` with the descriptor their own
header names, so the stack is bounded and the order is the walk's. A layout whose release
can never fail carries no status. A masked `Release`, `Reset` and a block of any other
layout call the runtime with the descriptor index stored in the block header. An inline `Release` is the same walk
over the value in its slot: handles
through `hirsel_rt_v2_close_handle`, statuses accumulated and the failure edge taken after
the walk; a static mask gates a record's top-level fields. A Ready `Copy` (Dup or Fields
recipe) loads the place and adds one count per counted holder in scan order.
FlatFormatV0 (LIR schema revision 4): a flat construction writes each field at its cursor,
a String or Bytes slice or a variable flat record's block payload behind its u64 length
prefix, into a block sized to the parts or inline when the record is fixed; `FlatView` is
a slice whose owner is the block the address crosses. A region-held flat value
reserves a real immortal region-kind header with its rounded payload capacity,
once per value; its views use that header as the owner. `FlatLength` and `Index` read a flat
sequence's count and walk to an element; a flat field lies at its cursor; `FlatCopy` is the
inline byte copy of a fixed flat record, or, for a variable flat value (a block of its
payload), a new heap block of the payload's extent holding the same bytes. Dynamic
extents come from the layout's size program, evaluated in emitted code.
Ordinary fields and Inline element strides use the bounded storage maximum,
including Packed payloads. Aggregate loads, stores, construction and capture
placement use `SizeSpec::storage()`; variable flat encodings retain their byte
extents. The Packed tuple and closure regressions exercise these consumers.
Closures build counted heap
environments at B's `environment()` layout (an empty one is the static `hirsel.env.empty`
block), and capture slots are bound to their environment fields. A generic nested
function's closure has no code (LIR schema revision 9): its code word is null, and a
`Callee::Bound` call imports and calls the named instance directly with the closure's
environment. Every definition also
gets a function-reference thunk `_H1T…` that takes and ignores an environment.

Ordinary callable SSA is a pair of LLVM pointers (code, environment), preserving
the same two-word storage and cross-object ABI without byte packing. Erased
calls retain indirect dispatch and receive `nounwind`, as Hirsel failure uses
explicit status. The driver's callable-target specializations and known source
bodies are `linkonce_odr`/`inlinehint` in their calling partitions. The units in
`EmitInputs::available_externally` are small source units another object exports:
they are `available_externally` bodies, never exports, and a call the optimizer
keeps stays an import. Source failure
reanchoring and required external/function-reference entries are preserved.
`ReadStatic` emits its constant's static blocks (`hirsel_lir::statics`), each a private
constant whose reference cells are relocations to the blocks they name; a reference result
is block 0, any other result is loaded from its payload. String/Bytes literals are private static immortal blocks
(`COUNT_IMMORTAL`, `DOMAIN_STATIC`), and a literal slice's owner is its sequence block.
`Compare` calls a private generated comparator per layout (`compare.rs`), which follows the
interpreter's order and fails MEM-3 past `COMPARE_MAX_DEPTH`. Path equality and hashing
use its inert intern ID; ordered comparisons resolve UTF-8 text with
`hirsel_rt_v2_path_compare(ctx, left, right)`. Comparator helpers carry the context
through nested layouts, including Set canonical ordering. Capability handles remain
incomparable. `IsUnique` loads the task-local u32 header count and compares it
with `COUNT_UNIQUE`, without a runtime call or diagnostic-flag branch. `Weaken`
is a move, and `UniqueCopy` calls
`hirsel_rt_v2_unique_copy` when deep and `hirsel_rt_v2_detach` when shallow; a deep copy of
an inline value spills it to a stack slot of its layout and calls
`hirsel_rt_v2_unique_copy_value` with that layout's descriptor. Regions use `region_enter`/`reserve`/`schedule`/`exit` on a
48-byte record per region instance, and a region reference addresses its payload directly.
`FinalizeBegin`/`FinalizeEnd` call `hirsel_rt_v2_finalize_begin`/`end`. A layout with a
user finalizer gets a private drop wrapper `hirsel.drop.<id>` that calls its drop body
(`EmitInputs::finalizers`) with its exact type. Each heap layout's runtime descriptor
(`hirsel_rt_v2_layout` and its steps, lowered by `hirsel_lir::rt_program`) is a
digest-named `linkonce_odr` constant with one address across all linked objects.
Descriptor addresses are significant; LLVM cannot merge them as unnamed data.
Step arrays use the same digest scheme. The fixed empty-environment descriptor
also has one shared identity. Private drop, hash and compare helpers keep their object-local
per-layout names: their addresses are not descriptor identity. A construction in an arena or at a caller
destination reads its arena token (the arena instance's slot, or the hidden destination; null is the heap)
and bump-allocates an immortal block inline by advancing the record's cursor within its
inline limit. A construction in an `Arena` slot has no heap test: C admits no null `Arena`
value. Segment overflow calls `hirsel_rt_v2_arena_alloc`, which, like `hirsel_rt_v2_alloc`,
returns the block and cannot fail (runtime ABI 3), so no allocation tests a status; a null inline limit
keeps diagnostic allocations in the runtime for owner tracking and poisoning.
A heap construction whose extent is a constant small request calls its pool's fast path
(runtime ABI 4, `runtime::alloc_fast`), a private function with `hirsel_rt_v2_alloc`'s
signature and allocator attributes that pops the pool's current page: the page pointer at the
context's `CTX_PAGES_OFFSET`, the page's `free` list and live count, and the header written
as one holder and the descriptor index. A null page or an empty list calls
`hirsel_rt_v2_alloc`, as every allocation does where the runtime keeps the pages null
(diagnostic mode, measurement archives, the system allocator). Glue's free calls the matching
`runtime::free_fast`. Both stay `noinline` calls through the optimization pipeline, so a block
that never escapes is still deleted with its free; `runtime::expand_fast_paths` then marks
them `alwaysinline` and the always-inliner expands every remaining call (`optimize`). A dup
is the same contract's inline increment below the context's dup bound (`runtime::dup`),
emitted in place.
A fixed-size, non-flat layout's block is its eight-byte header and payload, one bump and one
limit test from the word-aligned cursor; other layouts keep the 16-byte extent before the header.
Construction schedules the payload's drop with the
arena only when a counted child is not immortal. C-proved `arena_no_drop` constructions omit both the child-credit walk and scheduling. A dynamic live
mask or a mask on a variant in an inline release (no producer yet), return `CodegenError::Unsupported`, never a placeholder.

Tests:

- `cargo test -p hirsel-llvm` runs the in-process tests: the loaded libLLVM is the pin, every
  `TargetSpec` agrees with LLVM, receipt and failure-code rejections, the object manifest,
  reproducible owned artifacts, parse and verifier errors, triple and DataLayout mismatches,
  and malformed objects. Fixtures (`tests/support/`) derive layouts and `FnAbi`s with
  `hirsel_lir::layout::LayoutBuilder` and seal every unit with `hirsel_lir::validate`.
- `tools/check-native.sh` adds `--include-ignored`: `tests/it/native.rs` links emitted objects with
  C harnesses and the `hirsel_rt_v2` archive and runs them on x86-64 and under
  `qemu-aarch64-static`: the smoke fixture, the scalar boundary matrix against an
  independent oracle, and the ABI probes (`tests/native/abi_harness.c`): C calls emitted
  entries with by-value, read-only, taken and aggregate-result signatures, an unaligned
  `@packed` record and a caller destination; emitted code calls a C-defined Hirsel entry
  and a C closure; direct calls, loops, i128, LANG-9 checks, MEM-13 tags, MEM-14 bounds and
  propagated failures with their sites; and the heap probes (`tests/native/heap_harness.c`),
  which allocate, release (through drop glue and on failure paths), reset
  and reuse blocks under the runtime's diagnostic mode and require its live block count to
  return to zero. `tests/it/ready.rs` runs the Ready-construct probes (`tests/support/ready.rs`):
  closures, function references, `IsUnique`, `Weaken`, `Compare` (records, variants and
  null-pointer niches, NaN, strings, a chain at the MEM-3 bound), statics, literals, unique
  copies, finalizers by drop thunk and explicitly (the slot closed by emptiness, a
  resource with counted fields), regions, `mut` parameters, Ready `Copy`, inline `Release`
  of a record, a callable and a variant with a failing leaf, a field moved out of a block
  with masked release and reset, a reuse keeping a field, a closure environment reset,
  flat construction (a nested variable record included), views, lengths, indexing and
  copies, and Simd lanes. The heap checker
  runs every case for the expected line; only the handle close, whose handle C passes and
  whose adapter is the harness's, is written out. The native program must print exactly
  those lines on both targets, under the runtime's diagnostic mode and again in production
  mode, with no block left live after 1000 rounds.
  `tests/it/glue.rs` is the drop-glue differential: typed graphs of the J2 corpus and
  rc-trace shapes (records, ordinal and niche variants, lists, repeats, a handle, a
  callable, and as children a resource, an inline resource and a structure-of-arrays list)
  are built twice from generated C data and released by the runtime walk and by an emitted
  entry; the finalizers, closes, failure chain and each slab's free-list links (its free
  order) must be equal, in both modes, with no failure, every finalizer and close failing,
  and every third; a full tree's order is checked against the drop program's post-order,
  a 300-link chain outruns the budget, and a million-link chain is released on a 128 KiB
  stack. The same test file writes the object `crates/hirsel-rt/harness/bench.sh` times.

A `Switch` on an aggregate value with a layout tag spills the value and loads
its tag. A projection from a slot of an uncounted type (region storage) starts
at the payload the slot points to, as a projection through an uncounted value
does; a counted block's payload follows its header.

Iterator `Closure` and indirect-call emission use the layout's environment-first
three words (environment, step code, zero). Function callables retain their
code-first pair. Steps stay indirect in LIR and use the ordinary status ABI.

A called runtime manifest row may name a deterministic `failure` rule. Its
status-1 branch records that rule with driver-assigned codes at the CallSite
anchor before entering the ordinary cleanup edge. The export writes no output
on that path; the success path adds no call. Propagating rows keep their existing
already-recorded failure protocol.

Revision-11 consuming closures retain caller-owned environments. Construction
initializes their trailing live word; `UpdateMask` clears it in the body, and
glue and descriptor release both follow its guarded drop spans. The source-free
`tests/it/once_capture.rs` controls run C and H and compare both native targets at
O0/O3, including an uncalled closure and many-to-once weakening.

LIR revision 13 field drop flags lower through ordinary bool block parameters
and Branch instructions. Cleanup emits a guarded preparation and release for
each conditional inline part; LLVM needs no new runtime flag representation.
Explicit finalization markers enter the backend only in validated Ready units.

Read-only Bytes Index resolves a Slice through its owner payload, the sequence header, and accumulated start; it checks against the selected length before loading the u8. The Array layout remains one pointer.

Payload addresses dereference exactly one Box using its existing payload
offset. Region-backed slot preparation consumes the corresponding explicit
step when it starts at the headerless payload. The Ready controls include
whole-payload writes through scalar and nested boxes.

`EmitInputs.measurement` defaults to false at ordinary callers. True adds
`hirsel_rt_v2_measure_release(ctx, block)` to every inline compiled count edge;
runtime fallbacks account for their own edges, including a dying block at the
spent glue budget. Dup, allocation and the glue's free run inline only below the
context's dup bound, on a non-null page and under its inline-free flag, which
measurement archives keep zero and null, so each reaches its runtime entry there and is
accounted; detach calls its entry. Uniqueness reads are not count events. These hooks exist only in
measurement archives. The false option preserves ordinary object bytes.

Shallow detach postconditions come from
`hirsel-rt/src/copy.rs`: allocation starts with one holder, the payload bytes are
copied unchanged, and only child counts are incremented. The emitter saves a
sequence's old length in SSA before detach. After successful detach it exposes
count one and equality with that saved length through `llvm.assume`. This is the
opaque runtime out-parameter boundary: subsequent local storage-version joins
have no LLVM pointer/length companion phis, so this emitter cannot directly
forward those properties across the joined pointer. The assumptions are neither
invariant loads nor promises about later writes or sharing. The actual count
emitter's Dup control keeps the subsequent count load; diagnostic runtime controls
check empty/nonempty, unique/shared, immortal and static sources.

Borrowed flat parameters pass the logical payload address and its byte extent.
Fixed flat records bind directly to that address; variable flat records retain
the existing owner immediately before the payload, so FlatView and explicit
copies keep the ordinary owner and lifetime rules.

Flat sequence fields encode ordinary List/Array elements in two borrowed walks:
one computes the byte extent, and one writes into the final flat payload.
Fixed elements retain their bytes; slices and variable flat elements carry the
existing length prefixes. Nested lists use the same layout-driven walks. Extent
addition is checked before allocation and faults on overflow.

Direct library calls pass mutable Block/Box roots by value and return the roots
alongside status, with explicit write-back before either continuation. Required
external and function-reference entries retain the declared address ABI. Private
scalar-element sequence roots carry length and a must-unique bit in promotable
companion storage through detach/growth joins and internal call returns. Copies,
unknown calls, regional cleanup and potentially effectful releases discard the uniqueness fact;
escaped and captured roots retain ordinary memory reads. No producer IR claim or
validator rule is introduced. `tests/it/root_ssa.rs` checks growth followed by failure
at both the address entry and a direct internal call, on both targets at O0/O3.

`list.push` and `$native.list_push_spare` (a push `hirsel_rc::spare_push` proved
to have spare capacity) are always inlined into their callers: spare capacity
stores the element and length directly, while `$native.list_grow` is cold and noinline. Capacity
loads use the written allocator metadata offsets in runtime_abi, dispatching on
the header storage kind before reading any slab/prefix/extent. Zero-sized
elements report u64::MAX and capacity is never below the full u64 length.
The private single-sequence-root return is five words: the root pointer, typed
`ptr` and never tagged, the untruncated length, the exact capacity slots, the
must-unique fact (an `i1`) and the status. A loop over a root call or an inlined
body therefore carries the pointer itself: no `ptrtoint`, mask or re-tag per
iteration, so LLVM sees it loop-invariant between growths and can sink the
length store and vectorize a spare-capacity copy. Status has its own word so that every return site supplies
a constant: once a caller inlines the body, its status branch threads to the
right continuation and the root facts that continuation receives fold. AArch64
returns the words in registers, x86-64 in memory (only uninlined calls, such as
cold growth, pay for it); required address/function-reference entries decode
and write back before testing status.
The emitted count edges retain their existing measurement hooks.
`tests/it/sequence_capacity.rs` checks each metadata kind, small/large classes,
over-alignment, zero-sized and 16-byte elements, and the full length range.
Other single Block/Box roots use an intact pointer word and a status word,
so headerless regional payloads need no extra alignment for return transport.
Private entries use `.roots5`: sequence roots also receive their current full
length, must-unique bit and the caller's capacity cache by value (slots and a
validity bit). A capacity cache holds metadata-derived slots separately from
length; every query still takes the maximum with the current length. Stores,
detach and growth invalidate it. Every successful return hands back the exact
slots for the returned root (from a valid cache, else from metadata), so a
caller's cache is valid after any successful private call; a failed return
carries none and the caller invalidates. No cache cell's address reaches a call,
so the caller's cache and must-unique fact are SSA values across loop feedback:
a push loop over a list its function owns, after `reserve` or a growing push,
tests only capacity against a register, and the cold growth call refreshes both
facts. `hirsel-driver`'s `tests/it/growth_ir.rs` checks this in optimized IR on
both targets, with shared, escaping and over-reserved negative controls. Private calls preserve unrelated roots'
uniqueness only for resultless entries with pointer-free, silently destroyed
nonroot parameters. Any call handed no reference (no environment, no caller
destination, and pointer-free, silently destroyed parameters and result, such as
`fn step(h: u64, value: u64) -> u64`) preserves every must-unique fact: the
fact names a block whose one holder is this frame's unescaped slot, and counts
are task-local (OWN-6), so the callee cannot add a holder. `growth_ir.rs` checks
that a list written around such a call tests no count, against a borrowing twin.
Other calls retain conservative invalidation. The public
adapter derives length at the aligned payload and seeds unknown facts, preserving
its address ABI. The suffix prevents old signatures from linkonce merging.
Arithmetic guards use cold branch weights and shared per-FailureId blocks,
retaining the exact source site, token disposal and language cleanup.

A `FlatCopy` whose result has an ordinary source layout materializes the
address's prefixed encoding directly: a new flat-record block holds the payload
past its prefix, and a String/Bytes slice owns a new ordinary byte sequence,
with its copied length word, start zero and copied bytes. No source credit moves.

Optional `emit_object_profile(units, inputs, Some(&ProfileSources))` emits DWARF 5
line tables through a DIBuilder, with file, compile-unit, subprogram and instruction
locations. Ordinary emission stays unchanged. The driver retains immutable file text
and paths by definition; backend inputs contain no provider. Operation anchors take
precedence over the enclosing block anchor. LIR currently carries no individual span
for every scalar/read/Dup instruction, so those instructions use their block anchor.
The verifier runs on both sides of optimization as usual.

`emit_optimized_ir(units, inputs)` lowers and optimizes exactly as `emit_object`
does and returns the verified module text instead of an object, for codegen
controls that inspect what LLVM kept.

With `EmitInputs.measurement`, profiled calls enter and restore an object-owned site
constant (64-bit content id, package, file, function, LIR operation, line and column).
Generated glue and runtime child walks inherit the initiating call's site; nested source
calls and finalizer callbacks restore the caller's site. These hooks exist only in the
measurement archive. Sites group monomorphic instances of the same source operation.

Static block extents contain exactly their initialized payload bytes, with no
heap-class rounding: [] has zero element capacity. The private list_grow body
returns unique storage on success through SeqReserve; callers retain that fact
across its out-of-line root return so an append needs no second count test.
The driver's growth_ir controls pin the single first-push allocation on both
targets and the steady-loop and shared spare-capacity paths.
