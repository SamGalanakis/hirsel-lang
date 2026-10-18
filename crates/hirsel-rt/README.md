# hirsel-rt

The native runtime substrate behind the versioned C interface `hirsel_rt_v2`
(`include/hirsel_rt_v2.h`): the task-local size-class allocator, plain counts,
the MEM-3 release worklist, compiler-local regions, failure state and finalizers. Built as an
rlib (for tests) and a static library (for linking with generated code).

Allowed dependencies: std, the data-only core-contract and run-config, the shared pure LeafKernel crate, libc (OS syscall declarations), rustls 0.23 (ring/std/tls12), webpki-roots 1 and sha2 (PEM tape identity) in production. No other implementation crate; the driver wires them. Test-only: the interpreter for independent replay oracles (the registry inventory check and the compiled-program tests are in `hirsel-pipeline-tests`, `rt_*`), plus the data crates `hirsel-lir`,
`hirsel-types`, `hirsel-syntax` and `hirsel-tir`, so the tests release values whose layouts `hirsel-layout`
derives and `hirsel_lir::rt_program` lowers.

Invariants and boundary:

- Every export is a non-unwinding `extern "C"` function: a Rust panic aborts at the boundary.
  Statuses are `u8` (0 success, 1 language failure). A library manifest may ask the caller
  to record its rule at the call site; substrate entries already record it. Broken invariants
  are faults that abort with a message; host exhaustion is environmental and terminates. Neither
  is ever reported as a language failure.
- The run context is passed explicitly and shares nothing with other contexts (OWN-6).
- Blocks have one eight-byte `repr(C, align(8))` header: a u32 count and a u32
  descriptor word (29 index bits, two storage-kind bits, one dead-wide-cursor bit).
  Every reference points eight bytes before its payload, including over-aligned blocks.
  Capacity/alignment live in 64 KiB slab metadata, large prefixes, or immutable static/region
  extent records. Pools select `(class capacity, max(requested alignment, 8))`; small pools
  have separate 8/16 alignment classes and the existing capacity schedule. Allocation pops the current slab's page-local `free` list. Free pushes a separate
  `local_free` list. Nothing is atomic and no operation checks a thread: every page belongs
  to its task. On exhaustion sequential fresh capacity extends the allocation
  list in at most 8 KiB batches before deferred frees are collected, then another
  available slab is selected (mimalloc's page design; third-party/NOTICES.md).
  A free-list pointer overlays the entire header. Diagnostic states and returned-storage
  tombstones are cold side records; production stores no lifecycle byte. Idle slabs above
  the cap (8 MiB default) return at trim points, and task exit returns every backing block.
- One immutable descriptor registry belongs to a run context. The driver assigns it before
  partitioning the linked program and every emitted object receives the same assignment.
  Runtime-owned bytes/file-walk descriptors reserve indices 0/1, empty environments use 2,
  and canonical layout-descriptor digests order the remaining indices. ABI v2 entries accept
  indices; inline FINALIZE steps retain descriptor pointers as immutable program data.
- `alloc` and `arena_alloc` return the new block and cannot fail (runtime ABI 3): backing
  exhaustion terminates environmentally, which no program observes, so callers test no
  status.
- A reused token's payload keeps its bytes, in diagnostic mode too: a reuse may skip storing
  fields whose bits the donor's moved-out children left in place (`Reuse.kept`). Fresh and
  freed payloads are poisoned.
- Compiled fast paths (runtime ABI 4, `hirsel_rt_v2.h`): the context is `repr(C)` and begins
  with its heap's `Compiled` words, a dup bound, an inline-free flag, the idle slab count and
  each small pool's current page (`Class::current`, mirrored by `Heap::set_current`). Compiled
  code pops a constant small request off its pool's page, adds a holder below the dup bound and
  pushes a dead small block onto its page's `local_free` list, keeping the page's live count
  and the idle slab count exact; a null page, an empty list, a page's first deferred free, a
  saturated count and a large block take the runtime entry. Diagnostic mode, measurement
  archives and the system allocator keep the bound and flag zero and the pages null, so every
  operation reaches its entry there; the memcheck archive keeps the bound but zeroes the flag
  and nulls the pages, so every allocation and free does.
- Counts are non-atomic; a dup past the configured finite maximum makes the block immortal,
  which is sticky and never tests unique. Ordinary uncounted storage has no header.
  Compiler-emitted flat values in regions carry a region-kind immortal header:
  dup/release are no-ops and the emitted uniqueness count test is false, while the region still bounds
  their storage lifetime and frees it with its segment.
- Layout descriptors (`hirsel_rt_v2_layout`, version 5; its `LAYOUT_NO_EXTENT` flag marks a
  fixed-size, non-flat layout whose arena blocks carry no extent) hold a flat drop program that
  `hirsel_lir::rt_program` lowers from the LayoutRecord's nested drop program; nothing
  else produces one. Steps: `RELEASE` a counted holder, `FINALIZE` an inline resource,
  `CLOSE_HANDLE` through the Io adapter's close thunk (`config.close_handle`), `TAG` (skip an
  inactive variant's span), `NONNULL` (skip the dataful variant's span of a null-pointer niche
  when its word is null), `LIVE_BIT` (skip a moved capture whose persistent bit is clear), `LOOP` (a span per element, highest first; a static count, or a
  dynamic `u64` count only in an outermost loop) and `COLUMN` (one structure-of-arrays column of
  the enclosing loop's element). Outside loops each step names its top-level live-mask field.
- Release is iterative and post-order, with no allocation or recursion through data.
  A last release of a payload with no finalizer or drop steps frees it directly,
  including an erased String/Bytes owner: there is no cursor or worklist to drain.
  Simple dead blocks retain their index and use the count word for the next step. Loop/column
  descriptors set WIDE_CURSOR: their complete 24-bit step and 40-bit mixed-radix cursor spans
  the header plus a pre-reserved four-byte slab shadow (or a large prefix shadow). The parent
  link uses the consumed reference slot. The root's bytes stay intact for Reset/Reuse.kept.
  Resumption reconstructs frames from immutable steps, preserving DROP-2 and build-order
  recycling. Diagnostic descriptor validation rejects control words aliased with consumed
  slots, allowing the deliberately shared null-pointer niche guard and words in exclusive
  arms of one ordinal tag (a nested tag in one arm may share bytes with a sibling arm's
  reference: only one arm runs). Finalizers and closes
  run shielded; cleanup keeps the first failure and records later causes. Unsafe-ffi finalizer
  failure terminates. Compiler-reserved inline continuations are separate 32-byte records.
- Compiled drop glue (hirsel-llvm) shares the count word under one written contract
  (`hirsel_rt_v2.h`, transcribed in `hirsel_lir::runtime_abi`): a plain task-local `u32` at
  `COUNT_OFFSET`; at `COUNT_UNIQUE` the block dies, `COUNT_IMMORTAL` is never written, any
  other count is stored one lower. The glue releases a dead block's fields itself, in the
  drop program's order, and returns the storage through `hirsel_rt_v2_free(ctx, block)`
  (or its inline fast path), which is shallow and faults on a static block. Only layouts without a finalizer, an inline
  resource or columns have glue; every other block, a block whose layout its holder does not
  fix, and a block that dies with the glue's depth budget spent come back through
  `hirsel_rt_v2_release`, and the walk never calls glue. Diagnostic mode checks less on a
  glue path: a shared decrement done inline does not run `check_live_block` on the child, so
  a second release of an already freed block is caught only if it reaches
  `hirsel_rt_v2_free` (live, count 1, no finalizer) or a runtime entry.
- Regions are uncounted bump storage from a task pool; their cleanup list releases held heap
  credits and finalizes scheduled resources newest first at exit, then the pool keeps at most
  8 MiB idle (MEM-10). Unpublished builders may grow (in place or by relocation); published
  ones never move.
- `unique_copy` deep-copies headered heap, static and arena blocks reachable from a root through each block's
  `RELEASE` steps (tags and loops respected), each block once so sharing is kept; immortal and
  static blocks are copied too, since a `unique` value is written in place without a test,
  and a resource or handle is an invariant fault. Runtime ABI 6 takes an explicit destination:
  null selects the counted heap; an active region selects immortal arena blocks for the
  entire copied graph, with no outgoing heap credits. Fixed arena copies use descriptor
  size/alignment; other copies retain source capacity. Its visited map and
  work stacks are runtime-owned, so the native stack stays constant over any depth.
- `unique_copy_value` is `unique_copy` from an inline value (an aggregate, variant or inline
  array holding blocks): the caller's bit copy, whose holder slots (read through the value's
  layout descriptor) it rewrites in place to deep copies, one visited map for the whole value.
- `detach` is copy on write's copy (OWN-4): the block alone, into a heap block of its layout
  and capacity, with one more holder for each counted child it holds (static and immortal
  blocks included, since the caller writes the copy).
- Sequences (LIR schema revision 7): a sequence payload is
  a `u64` count at offset 0, a `u64` capacity in elements at offset 8 (runtime ABI 5), then
  elements at a fixed stride from `align_up(16, align)`. The capacity word is never more than
  the slots the storage really holds (the metadata's usable bytes after that offset by the
  stride), so compiled code reads it with one load; constructions and literals write the count.
  A slice's data stays at `payload + align_up(8, align)` plus its start, so a slice over a whole
  sequence starts at `sequence_slice_start` (8 for bytes).
  `hirsel_rt_v2_seq_grow(ctx, block, element_size, element_align, capacity)` leaves a unique
  heap block that already holds `capacity` elements alone, its capacity word made exact; otherwise
  the count and the counted elements move bitwise into a block of the class that fits (the new
  capacity is the class's, written exact),
  the old storage is freed shallowly, never scanned, and the returned reference names the new
  block. A block that is not unique (shared, immortal, or static: an empty list literal is the
  static empty sequence of its layout) is detached once at the requested capacity: count and
  elements copied, each counted element one more holder, the caller's credit of the original
  given back. Compiled code therefore does not detach a place whose only write is a reserve,
  so `[]` then `reserve(n)` makes one allocation. Empty static sequences size
  that fresh allocation directly, without querying old capacity or preparing a copy walk. A dead block and an overflowing size are
  invariant faults. Fresh allocations derive exact slots from `class_capacity`, the same
  mapping the allocator and `request_capacity` use. Reserves carry the allocator's chosen
  byte capacity with the new block, avoiding both a metadata read and a second class
  calculation; only an existing block needs the metadata lookup, since its original
  request is unknown.
  `hirsel_rt_v2_block_grow(ctx, block, keep, extent, align)` is the same growth by bytes:
  a block that holds `extent` payload bytes is left alone, otherwise its first `keep` bytes
  move. A sequential Packed sequence (MEM-13, `SeqAppend`) grows through it; a tracked arena
  one regrows in its arena instead, its old block emptied rather than freed.
- `Shared[T]` roots (MEM-11, runtime ABI 8, `shared.rs`): a root is a heap block whose payload
  descriptor carries `LAYOUT_FROZEN` and holds the frozen value inline at offset 0; its header
  count is the root count, so compiled `Dup`, `Release` and the uniqueness test add, give up
  and count roots with the ordinary count word contract. That is sound only because one
  cooperative scheduler runs every task of a run on one heap: roots cross task boundaries as
  pointer moves, and no two threads touch a count. `hirsel_rt_v2_freeze(ctx, root)` walks the
  value through its scan steps and makes every counted heap block it reaches `COUNT_IMMORTAL`
  (static and region blocks, never written, and nested roots, which keep their own counts, are
  skipped), so a read that copies a part writes no count and copy on write detaches. A
  `unique` input is a tree: a block whose count is not unique faults. The frozen descriptor's
  drop program leads with `THAW` (step 9), which restores those blocks to `COUNT_UNIQUE` before
  the following steps release them, so the last root's release frees the whole value; compiled
  glue never covers a `THAW` layout. `hirsel_rt_v2_thaw(ctx, root)` faults unless the root is
  the last, restores the counts the same way, and the caller moves the value out and frees the
  root shallowly. Both walks keep their stack in the run context and never recurse.
  `unique_copy` dups a root it reaches instead of copying it: copying never traverses a
  frozen graph. A threaded scheduler would need atomic root counts (the compiled fast paths
  and the runtime entries alike) and a frozen graph whose blocks no thread's heap reclaims
  before the last root's thaw or release.
- Keyed hashing (SEM-5): `hirsel_rt_v2_hash_begin(ctx)` answers the run's SipHash-1-3 key
  state from `Config.hash_seed` (Config's hash-seed word, revision 7), and
  `hirsel_rt_v2_hash_bytes(state, bytes, length)` folds bytes into a state. Compiled
  `Inst::Hash` walks a layout's scalar leaves itself (hirsel-llvm `compare.rs`) and calls
  only these two; the seed moves placement and never an observation.
- The canonical codec (SER-2, runtime ABI 9, `codec.rs`):
  `hirsel_rt_v2_codec_encode(ctx, codec, value, out)` and
  `hirsel_rt_v2_codec_decode(ctx, codec, bytes, out)` walk a compiler-emitted
  `hirsel_rt_v2_codec` (its descriptor bytes and the u64 program words
  `hirsel_lir::codec::CodecPlan::program` writes) over native layouts. Encode
  writes the `HSC1` envelope with Set elements sorted by their encodings, NaNs
  canonical and bools 0/1; Map entries keep insertion order. Decode checks the
  envelope, validates the whole payload in `hirsel_types::wire`'s detection order
  (each failure one `DecodeError` class) and only then materializes zeroed
  counted blocks and sequences, so a rejected input allocates nothing; a decoded
  Map or Set carries an empty (unindexed) index. Program version 2 walks
  private flat framing, active Packed extents and SoA columns in row order.
  Flat reconstruction patches native byte extents separately from wire element
  counts; sequential blocks retain exact used bytes and SoA blocks retain rounded
  column capacity. Both walks are iterative. Past
  `hirsel_types::wire::Limits::default`'s byte and value budgets the run ends
  environmentally, as the interpreter's operational limit does.
- No numeric Depth or frame charge exists until spec #16 rules.

Transitions of D's Lean model (`proofs/rc/README.md`) cited in code comments as `T<ID> <name>`;
`tests/it/model.rs` runs the model's `rc-trace v1` vectors (`native-tests/model/`) on the runtime and
requires its checked final states:

| ID | names | entry |
|---|---|---|
| T01 | Alloc | `alloc` |
| T03 | Dup, DupSaturate, DupImmortal | `dup` |
| T04 | Drop, DropLast, DropImmortal | `release` |
| T12, T13, T14 | Reset, Reuse, Dispose | `reset`, `reuse`, `dispose` |
| T15 | FinalizeBegin, FinalizeEnd | a finalizer call and its outcome; `finalize_begin`, `finalize_end` around a drop body compiled code calls |
| T16 | DropEdge, Free | a `RELEASE` step consuming a child's credit; storage returned after draining; `free` after compiled glue's inline edge |
| T18, T19, T20 | RegionOpen, RegionAlloc, RegionEdgeAdd | `region_enter`, `region_reserve`, `region_hold` |
| T23 | RegionClose | `region_exit` |

An arena (`arena.with`) is a region whose record its
token names. `arena_alloc` bump-allocates an immortal regional block (header and
payload, after an extent unless its layout is `LAYOUT_NO_EXTENT`, whose capacity
is its descriptor size) whose segments grow one and a half times the previous one plus
the request, as Zig's `std.heap.ArenaAllocator` does (third-party/NOTICES.md).
`arena_enter` enters an arena's region warm: it seeds the cursor and limit from an
idle standard segment of the pool, when one exists, so the first block is an inline bump.
Compiled arena allocations bump `Region.cursor` inline up to `Region.end` at the
C header's asserted offsets; every reservation ends on a word, so the cursor needs no
alignment step. Diagnostic regions keep that limit null, so every
allocation registers its owner and poisons its payload through the runtime (the memcheck
archive keeps it null too, so every reservation is described to Valgrind);
runtime reservations and builders derive the actual bound from the segment.

Checks: the `hirsel-rt` Buck2 tests and `harness/run.sh` (ASan/UBSan C harness, AArch64 under
qemu, on the diagnostic `//:hirsel-rt-archives` archive), which `tools/check-native.sh` runs. `harness/bench.sh` times the `binarytrees` C port
against glibc malloc, mimalloc, jemalloc and 48- and 32-byte-node mimalloc controls (the prior header controls) in paired rounds, with a tree freed by the walk
(`hirsel-walk`) and by the compiled glue of the node layout (`hirsel`).

The compiler's native entry wrapper registers the returned Error and its immutable `exit_report` diagnostic view. Launch flushes program stdout, formats and writes the report, then finalizes the returned ownership before task exit. Replay recreates the report from the returned value; no stderr capability or Io tape event is needed.

## Processes

`process.rs` holds the Process capability (docs/spec/core-library.md#processes).
`State::execute_process` checks every `proc.run` before any effect: the Proc and Dir
handles, a canonical absolute program and NUL-free arguments (InvalidInput), then the
exact rules (NotGranted); a scripted world (`process.script@1`, `test.process`) is
served from its entries, and only a checked host run reaches the adapter's provider.
`RealIo` runs it with `run_host`: std `Command`, an empty environment plus the rule's
pass-through names, `/` or the pinned Dir as working directory, and one `poll` loop
that feeds stdin and drains both pipes, killing a child past `max_output` on either
stream. A `Deadline` is the Instant's CLOCK_MONOTONIC ticks: a reached one is
`TimedOut` before spawning, and at expiry the child gets SIGTERM, then SIGKILL after
`TERMINATION_GRACE` (1 s), and is reaped before `TimedOut` returns. Under cooperative
admission the run is a `HostJob`: the same loop on a helper thread, whose finished
outcome makes a socket readable that is the parked token's interest, so the task parks
(`host_waits::may_park`); a cancelled poll finishes the token, whose dropped job makes
the loop's cancellation socket readable, SIGKILLs the child and joins the thread, and
the run completes `Canceled` with no tape event beyond its `blocked` park. SIGPIPE is
blocked on the calling thread for the loop and a raised one is
discarded, so a child that ignores its stdin cannot end the program. `SimIo` replays
the recorded completion in place of the host and rejects one that contradicts the
re-derived checks. A launch without a run record reads the root Proc's `io.process`
grant JSON from `HIRSEL_RT_PROCESS`, for live runs and replay alike. Tape forms are `process`, `test_process` (runs and
children) and `process_run` (with its deadline as `clock:ticks` or `none`) requests and a
`process` completion (`host_io_tape.rs`). A recorded `TimedOut` needs a deadline, and
raises the latest monotonic reading to it as a completed timer wait does; a scripted
world times out only a deadline that reading already reached.

`proc.spawn` makes the same checks and mints a `Grant::Child` of its Proc, which every
child request names (CAP-2); the policy tracks whether its input is open. A scripted
world plays the entry with `hirsel_run_config::ScriptChild`: a read at an `Expect` is
`AdapterError::Blocked`, and a contradicted or unmet `Expect` writes its Test report and
completes `Checked(false)` (LANG-9). A host child goes through `HostCall` to
`host_child::HostChildren`, three nonblocking pipes per child: a write or read that would
block records its pipe as the token's interest and parks (a write that accepted bytes is
committed, so it is no longer a cancellation point), and without cooperative admission it
polls that pipe. `child.wait` closes input, discards the remaining output and reaps, as a
`HostJob` under cooperative admission. Dropping a Child (`Request::Close`) closes its
input, SIGKILLs and reaps it, then closes its outputs; a cancelled wait drops the Child
it took (`host_waits.rs`). Child tape forms are `process_spawn`, `child_write`,
`child_close_stdin`, `child_read` (with `stdout` or `stderr`), `child_wait` and
`child_kill` requests, the dropped Child's `close`, and `spawned`, `child_unit`,
`child_bytes` and `child_exit` completions; `SimIo` replays them without spawning.

## Random draws and timer waits

`RandomSource` is the admitted Random provider: `random.splitmix64@1` keeps the SplitMix64
state of the run's Random seed and `random.system@1` fills the draw with `getrandom(2)`,
retrying EINTR and short reads; a draw is all-or-`Unavailable`. `hirsel_rt_v2_random_bytes`
issues no request for a zero count. `clock.sleep_until` is `Request::SleepUntil` with the
deadline's nanosecond ticks. Without a cooperative scheduler `RealIo` blocks the thread in
`clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME)`, restarted on EINTR. Under one (the
interpreter, and native runs admitted with `io.tasks`) a wait not yet due records a
deadline-only interest under the entered token in the cooperative Readiness
(`Readiness::timer`) and returns `Blocked`; `host_waits::may_park` includes it, so the task
parks, the same request is reissued when `ready`, and a cancellation finishes the token,
which removes the timer, and completes `WaitError::Canceled(cause)` with no host request. `Readiness::wait` with only timers left sleeps until the
earliest. The scheduler orders timer polls by the original `SleepUntil` ticks, including
when several are overdue; equal deadlines retain suspension order. Host `Instant`
rounding cannot reorder ties, and replay uses the same request deadlines. `State::sample` validates replayed draws and waits and, after a completed wait,
refuses a later monotonic sample below its deadline. `instant.checked_add` is pure. Tape
forms are `random_bytes` and `sleep_until` requests and `random` and `wait` completions.

## Bounded arenas

`arena.bounded(cap, body)` (MEM-6) charges every block placed in its arena a logical size
before the block exists; a charge past the remaining budget is the failure `OutOfSpace`
(rule MEM-6) at the constructing site. The charge is target-independent: it reads the type,
never a physical layout, and no header, extent, slab, segment or padding is charged.

| value | logical bytes |
|---|---|
| `bool`, `u8`, `i8` | 1 |
| `u16`, `i16` | 2 |
| `u32`, `i32`, `f32` | 4 |
| `u64`, `i64`, `f64` | 8 |
| `u128`, `i128` | 16 |
| `()`, `Never` | 0 |
| a block reference (`box T`, `String`, `Bytes`, `List`, `Array`, `Map`, `Set`, `SortedMap`, `Shared`, ...), an opaque nominal (a capability, handle or `Arena`) | 8 |
| a function or closure value, `Iter[T]` | 16 |
| a tuple, record or resource | the sum of its fields |
| a variant | 4 plus its largest variant's fields |
| `Inline[T, n]`, `Simd[T, n]` | `n` times `T` |
| `Packed[T]` | `T` |

| block placed in the arena | charge |
|---|---|
| `box T` | `T` |
| a `List` or `Array` literal of `n` elements of `T` | `n` times `T` |
| a `Map[K, V]` or `Set[K]` of `n` entries | `n` times `K` plus `V`, or `K` |
| a `String` or `Bytes` | its length |
| a closure environment | the sum of its captures |
| `copy_to(a, x)` | every block of `x`'s tree at its length, one charge before anything is copied |

A copy charges each reference to a shared block again, so sharing is unobservable. A `List`
or `Array` block in an arena grows there (runtime ABI 11): a push charges its element before
consuming it, and a copy-on-write copy of a shared one is charged its length. Spare capacity
and abandoned storage are never charged, so a List's storage stays within four times its
charged bytes plus four elements. A `Map` or `Set` in an arena does too (runtime ABI 12):
its block (`LAYOUT_KEYED`, a fixed layout tracked like a List, its 32-byte prefix leaving the
extent words unused) charges nothing itself, its entries List at payload offset 0 is
tracked and charged as any List, so an insert of a new key charges one entry through the
entries' push and the first write after a copy-on-write copy of the shared block charges the
entry count, and its index is neither tracked nor charged. `map.empty()` and `set.empty()` construct
that block where their relation places them (PROV-4), as a literal does. An Io read into an
arena List (`stdin.read`, `file.read`, `net.read`) is charged its maximum bytes before its
effect (runtime ABI 13): compiled code first calls `hirsel_rt_v2_arena_reserve`, which charges
them and copies or relocates the List as a push does, so the read appends in place. An
`@soa` List (`LAYOUT_COLUMNS`, descriptor version 9) is tracked too: a push charges its row and
copies a shared block at its capacity, and the compiled column copy regrows a full one in its
arena through `hirsel_rt_v2_arena_regrow` (runtime ABI 14). A sequential `Packed` List
(`LAYOUT_SEQUENTIAL`, descriptor version 10) is tracked as well: a push charges its element's
logical size, and `hirsel_rt_v2_block_grow` (runtime ABI 15) regrows a full unshared one in its
arena and empties the old block. A callback's own `OutOfSpace` becomes `Error(OutOfSpace)` once its cleanup has run, unless that
cleanup failed too: then the failure propagates with the `OutOfSpace` primary (FAIL-3). The
reference interpreter (`hirsel-interp`, `src/bounded.rs`) charges from
`hirsel_types::logical`; H and native execution charge from layouts
(`hirsel_lir::layout::block_charge`, held to the type table by a cross-check test).

Natively (runtime ABI 7) an arena record carries `remaining` (offset 48, `u64::MAX`
unbounded), `bound` and `exhausted`. `hirsel_rt_v2_arena_bound` sets the budget when the
bounded token is made, counting from the failures recorded then. Compiled code tests each arena
construction's charge against `remaining` and lowers it inline; an excess calls
`hirsel_rt_v2_arena_exhausted`, which records the failure and marks it as the arena's own.
A deep copy into a bounded arena charges the tree first from each descriptor's `logical` word
(descriptor version 6; per element times the payload's u64 count with
`LAYOUT_LOGICAL_PER_ELEMENT`) and returns `STATUS_EXHAUSTED` without copying when it does not
fit. `hirsel_rt_v2_arena_recover` forgets the failure only when it is that arena's own and the
one record since its bound. A copy of a block with no logical size (a flat encoding or an
environment) into a bounded arena is an invariant fault.

## Library and Io

`operations::manifest` joins registry-produced rows against all reviewed identities in `native-tests/library/operations.tsv`, including exact declaration digests and passing/cleanup conventions. Missing, duplicate, stale and foreign identities fail closed. M2 entries name their feature; remaining aggregate/backend gaps explicitly use NotImplemented. Effects retain registry conservatism. The scalar operations (`operations::SCALAR_INTRINSICS`) are `Instruction` rows: LIR instructions since schema revision 6, with no runtime entry. So are the inline library operations (`operations::INLINE_OPERATIONS`: the sequence lengths, which import lowers to `SeqLen`, and the legalized `simd.add` and `simd.splat`, and `forget`, which consumes without cleanup); a library body's row is `Body`, and each library lane edits only its own operations' rows. The four f32/f64 bits/from_bits operations are among them: `Convert` in its `Reinterpret` mode (LIR revision 23), a bitcast with NaN canonicalization. Formatting, parsing and reductions are ordinary std source, with no host algorithm entry or boxed universal value.

`io::Adapter` is the typed Rust Io boundary. The integration tests keep their
typed request builders and handle wrappers in `tests/support/library.rs`. `RealIo::grant` pins an explicit Linux directory descriptor and captures stdout; `grant_with_output` adds an explicit writer and records actual committed prefixes. All descendant opens resolve relative links component by component from pinned
directory descriptors. Absolute targets and parent steps beyond each grant root
are refused; narrowed grants retain their own pinned roots. Entries are classified
with fstatat(AT_SYMLINK_NOFOLLOW) and pinned with O_PATH before any data open. Directory grants retain separate inode identities, and File narrowing compares the opened inode. Walk sorts UTF-8 bytes, follows confined links, and returns FileFault for escaping
links, link/directory cycles and special files. Writes prepare replacement storage before publication so zero-byte errors preserve destinations; successful or partially failed writes publish exactly the recorded prefix.

`SimIo::new(initial_tree,tape)` always enforces exact replay, including an empty tape. Request/completion mismatches, missing events and unconsumed events are adapter errors. Acceptance precedes stdout/tree/handle publication. Tape events contain typed requests, completions and explicit capability fault inputs; neither backend consults ambient time, seed or environment. A canceled commit still consumes and aborts the owned atomic handle, as interpreter cleanup requires.

Network: `net.rs` holds the core network data, the
`NetProvider` seam and grant policy, which runs in `io::State` before any provider call,
so a denied or invalid request issues no syscall, DNS query or TLS byte (tests/it/net.rs
counts provider calls). Streams and listeners are handles branded by their Net; a
ConnectTls stream moves no plaintext before its handshake. `net_host.rs` is the libc
provider, ported from Rust std's Unix socket code and Zig's getaddrinfo lookup: nonblocking
connect with `poll` and one deadline across resolved addresses, SO_RCVTIMEO/SO_SNDTIMEO,
MSG_NOSIGNAL writes. Network outcomes are provider inputs recorded as completions (tape v4,
`net/tape.rs`); `SimIo` replays them with no sockets after re-deriving every grant decision
from its bound rules (`set_net_rules`; launch reads the root Net's `io.net` grant JSON from
`HIRSEL_RT_NET` and binds `net.script@2` peers through `set_net_script`). TLS upgrades the granted
socket through rustls in `net_tls.rs`; subsequent network reads and writes carry
plaintext. `tls.close` sends close_notify before closing on every outcome; implicit
drop only closes. PEM bytes pass through `Adapter::tls_roots` to the host provider
for its next handshake, outside the tape request, whose SHA-256 must match. Replay
and scripted providers ignore the bundle and use only the digest. Scripted worlds
attach independently at the same provider seam. `net_abi.rs` exports the stream
operations at the Hirsel ABI (`NET_EXPORTS`), plus `hirsel_rt_v2_net_read_into`, the
entry of `$intrinsic.NetReadInto` that `net.read_into`'s compiled body calls with its
buffer size. `net.resolve` allocates its `List[SocketAddr]` under descriptor 0, the
runtime's plain pointer-free sequence descriptor. `hirsel-pipeline-tests`
`rt_net_native` records a compiled program against a loopback server and replays it
both natively and independently in the interpreter, with an altered-byte control.
The test-only `tests/support/net.rs` translator reconstructs independent interpreter
script peers from native typed completions. `tests/support/io.rs` compares every
completion, surviving grant, final tree and stdout, then strictly replays the
interpreter's own tape. Corruption controls keep the fixture tape separate from
the comparison tape, including external fault inputs. Partial native writes supply their accepted prefix to the
script; read chunks and EOF remain exact. TLS success, name mismatch and handshake
failure are representable; other TLS failures are refused until the approved peer
schema can express them. PEM events require the original trust bundle and verify
its SHA256 digest. Certificate names are merged across the entire recording;
a fixed peer cannot both accept and reject the same name.

Launch owns a stable `io::Binding`, installs `io::hirsel_rt_v2_io_close` as Config.close_handle, and explicitly binds it with RunCtx.bind_io_data. The adapter must stay at its bound address and outlive every release on that context. Close-handle runs only while it is bound, with exclusive adapter access; unbind before dropping it. There is no global/thread-local fallback. This private RunCtx field changes no Config or C ABI.

`Map` and `Set` are native LIR bodies in `hirsel-rc::library::map`. Public length queries lower to entry-sequence length; membership probes keys without copying values, and ordered removal moves the value and rebuilds only the private index; the compiled seed law in `hirsel-pipeline-tests/tests/rc/rc_maps_native.rs` replaces the Rust Map model. Sequence consumption is implemented by compiled library bodies in `hirsel-rc`; their C and H checks, native target tests and resource failure campaigns replace the Rust Cursor model. The six consuming open/next manifest rows are Body.

## Tasks

`tasks.rs` is the structured-concurrency scheduler (CONC-1, CONC-2, FAIL-2) and `fiber.rs` its
stackful fibers: guarded 8 MiB lazily committed stacks, registered with Valgrind, and a
callee-saved register switch for x86-64 and AArch64. The scheduler is cooperative and
single-threaded, with the reference interpreter's schedule:

- one FIFO queue of runnable tasks;
- a yield after every capability operation (`yield_now`, called by the Io entries and the task entries);
- a task that must wait (a join, a scope exit) parks with what it waits for;
- parked tasks are polled at every yield and whenever the running task stops; timer waits
  use deadline order (ties by suspension order), other waits use park order;
- a successful poll performs the wait's effect (a join's poll moves the result) and queues the task.

Host requests park too (`host_waits.rs`). Every capability request goes through
`host_waits::issue`. A canceled task's stdin read, print or waiting network request
(resolve, connect, accept, read, write, TLS) completes `Canceled` without reaching the
adapter (CONC-3), as the interpreter's HostLibrary does. A stdin or network request that the
adapter answers `AdapterError::Blocked` parks its task (`Wait::Host`): a network request after
its yield, a stdin read at once, as the interpreter parks them. Its poll re-issues it when the
task is canceled before the request committed an effect, or when the adapter finds it ready,
so the effect and its tape event happen at the poll, and the resumed operation does not yield
again. With nothing runnable the scheduler sleeps in the adapter's `wait` on every parked
descriptor and the earliest deadline, then polls again.

All tasks of a run share its one heap. A value crossing tasks moves by copying its root, and
the checker's sendability rules keep each counted value owned by one task. What is a task's own
in `RunCtx` is swapped at each switch:

- its failure chain with spawn paths;
- its finalizer shield;
- its release worklist;
- its innermost region;
- its measurement site.

A threaded scheduler must reintroduce per-thread heaps and transfer moved graphs between them.

The compiled bodies of `task.scope`, `task.async` and `task.await` (`hirsel-rc::library::tasks`)
call the `$intrinsic.Task*` entries (`hirsel_rt_v2_task_*`). A result crosses as a whole
`Option[R]` image, so the runtime never reads a result's layout:

- A child's `$native.task_main` closure moves the spawned function out of its environment, calls
  it and drops it, so its captures die with the child's frame. It then hands its `Some(r)` slot
  to `hirsel_rt_v2_task_finish` and stays in that call holding the result.
- A join's poll copies the image into the awaiter's `None` slot and writes `None` back. A discard
  leaves `Some(r)`.
- Either way the child's own compiled cleanup drops what its slot holds, and the runtime then
  releases the closure environment.

`task.cancel` and `task.detach` are runtime primitives. A detached child drops its result when
it completes, or at the detach if it already has; only live children keep scope rows.

`channels.rs` is the run's unbounded FIFO channels (CONC-6), behind `channel.open`,
`sender.dup` and the compiled bodies of `channel.send` and `channel.receive`:

- A message crosses as the block of a one-element `List[T]` in an `Option` slot, so the runtime
  moves block references and never reads a payload's layout.
- A send fails `Canceled` when the running task is canceled, else `Closed` when no receiver is
  left.
- A receive returns `Canceled`, else the oldest message, else `Closed` once no sender is left,
  else parks; its poll performs the receive.
- Sender and Receiver are run-local handle words with bit 63 set. Their drop's handle close
  reaches the scheduler instead of the Io adapter, and the last receiver's close releases the
  messages nobody received, oldest first.

`sync.rs` is the run's mutex and atomic cells (CONC-4, CONC-5, FAIL-4), behind the compiled
bodies of `mutex.create`, `mutex.with` and `atomic.*`. A mutex's cell holds its payload as the
block of a one-element list, moved out of and into `Option[List[T]]` carriers as a channel
message is, so a Map, String or List payload needs no layout knowledge here; the run's end
releases the blocks the mutexes still hold. The layout of `Mutex[T]` requires a copyable payload,
whose release is unobservable (a mutex over a resource stays M2). An atomic's cell holds its
scalar payload as the `Some` image of an `Option`, which the runtime copies. A mutex body may not
yield, so no task observes a mutex another holds. A failing body kills its mutex, which keeps the
failure chain; a later `mutex.with` fails FAIL-4 continued by that chain.

`task.scope_until`'s compiled body is `task.scope`'s over `hirsel_rt_v2_task_scope_enter_until`
and `hirsel_rt_v2_task_scope_exit_until` (CONC-3). The entry takes the `Deadline` (clock handle,
then the Instant's ticks) and, before its yield, arms the scope's timer through
`host_waits::arm`: a `SleepUntil` under a token of its own, issued whatever the task's
cancellation. A completed one (a reached deadline, or an unreadable clock) expires the scope at
once; a blocked one stays armed. `poll_parked` first asks `host_waits::expired` for the armed
tokens a poll finds ready, by deadline with ties in arming order, and expires their scopes: the
scope's row is canceled `DeadlineExceeded`, each child too, and the children of the scopes its
owner entered inside it `Parent`. Armed deadlines keep `host_waits::wait` sleeping. The exit
joins as usual, then disarms a timer that has not fired (no tape event) and writes whether the
deadline fired, from which the body builds `Expired(r)` or `Finished(r)`. The tape therefore
holds a scope deadline as the `sleep_until` request, `blocked` and `ready` events of a timer
wait, exactly as the interpreter's HostLibrary records it.

`task.check_canceled`'s compiled body reads the running task's cancellation through
`hirsel_rt_v2_task_canceled` (0, or 1 + the `CancelCause` ordinal), which yields as the operation
does, and builds `Ok(())` or `Error(Canceled(cause))`.

`isolate.rs` is `task.isolate` (FAIL-2). Its compiled body reads the running task's failure
record count (`isolate_begin`, which yields as the operation does) and calls its callback; a
failed callback has unwound when its cleanup reaches the `Isolate` terminator, whose
`hirsel_rt_v2_isolate_catch(ctx, start)` (runtime ABI 10) moves every record since that count
into the caught chain, the task's own failures attributed to its spawn path. The body builds
the `Failure` value innermost first from the `isolate_*` entries: kind (from the rule:
MEM-3 `Depth`, MEM-6 `OutOfSpace`, FAIL-4 `DeadMutex`, else `Check`), rule ID, definition path,
line, column and spawn path, each a fresh block. A record holds compiler codes, so a program
registers its failure table before the entry runs
(`hirsel_rt_v2_failure_table`, called by `main` of the launcher object from either backend):
one pointer-free byte string holding each site code's definition path and one-based position
(0 and 0 for a site inside a library body, which only a resource failure reaches), sorted by
code, and each program rule code's rule ID.

## Launch and the Hirsel-ABI Io entries

`hirsel_rt_v2_launch(entry, result_size, layouts, layout_count, config)` (`src/launch.rs`) binds
the linked program's immutable descriptor registry and runs one compiled
`fn(rt: Io, args: Args) -> Result[(), E]` in a fresh context with real Io
granted on `HIRSEL_RT_ROOT`, and exits 0 for `Ok`, 1 for `Error`, 2 for a
language failure, 3 for a refused launch and 4 for a Blocked run (CONC-8: every task waits
and none can proceed; the run stops there without cleanup, and its record says `blocked` and
`audit unavailable`). With `HIRSEL_RT_OBSERVATION` it
writes the observation record (`hirsel-observation 1`, the status, the result
bytes or the failure chain as kind, rule and site codes, and the task-exit
audit in a diagnostic or measurement archive, or `audit unavailable` in production). For harness launches without a run record, `HIRSEL_RT_TAPE` receives the real Io completion tape
(`hirsel-io-tape 7`, which also reads version 6: run identity digest, independent hash/Random/Test seeds, for production recordings the canonical run record and its admission time, launch arguments, initial tree, typed completions and a complete-count trailer); `HIRSEL_RT_REPLAY` runs on simulated
Io that accepts exactly a recorded tape and refuses a mismatch, a missing event
and an unconsumed one. Each completion is appended and flushed before delivery; an abort preserves its confirmed prefix. A missing trailer marks an unknown suffix and refuses complete replay or any resumability claim. Native stdout uses raw OS writes, so EPIPE and EBADF become recorded OutputError outcomes; generated main ignores SIGPIPE before entry. Replay divergence exits 3 with confirmed stdout, including during handle cleanup.

A `RuntimePrimitive` manifest row names a symbol exported at the Hirsel ABI v1
(`(ctx, [caller destination], [result pointer], parameters...) -> status`).
`operations::HIRSEL_ABI_EXPORTS` lists each such export with its physical
signature, and the driver checks every call against it before linking. These include stdout/print, all directory and file leaves, Path/Args, MonotonicClock/WallClock/Random,
Test check/check_case/fail/dir, IEEE bit primitives, Unicode cursors and text entries.
`test.for_all`, `test.equal` and `test.approx` are ordinary std@1 source;
expect_error awaits the approved general divergence contract. `Path` is an
inert u64 into the context's intern table; file and atomic handles close on drop.
Taken bytes and handles are consumed on every outcome, including status 1.
`file.walk` owns a counted environment of sorted results and a position, with a
runtime descriptor and step thunk; it stops after its first Error.
`hirsel_rt_v2_launch_zero` calls an eligible `fn() -> Result[(), E]` at its
three-pointer ABI without acquiring a real directory or an argv snapshot.
`hirsel_rt_v2_launch_test` grants only Test and accepts the same explicit config.
`launch::Config` (the C `hirsel_rt_v2_launch_config`) is separate from allocation
`context::Config`: hash, Random and Test seeds are independent u64s, followed by
the 32-byte run identity. Generated main passes a constant config; only hash_seed
reaches allocation. No seed is read from an environment variable. A tape records
all four fields, and replay rejects any mismatch before entering the program.
 The ten `$intrinsic.OrderCompare_*` rows are compiled
Body entries in `hirsel-rc`, with callable FunctionRef symbols and no raw C order
exports. SortedMap empty/len are Instruction rows, insertion/get/remove/open/next are Body
rows; its owned cursor finalizer is a private library body.

`iter.from` and `iter.map` are Body rows whose inline constructions own compiled
step thunks in `hirsel-rc`; `iter.next` is an Instruction row lowering to one
indirect call. No runtime iterator model or operation export implements them.

Unicode views (SEM-3) inline to a Copy of the String slice. `views.rs` exports
UnicodeScalars and ScalarPositions open/next at the Hirsel ABI:
open borrows (owner,start,length), next mutates the exclusive ViewCursor address.
The four-word cursor holds that slice and a relative position; it never owns the
source. Positions includes the terminal boundary of an empty string. Repr(C) Option
records match derived layouts on both targets in tests/it/views.rs. GraphemeView
and its Unicode16 segmentation execute ordinary std source; official source
vector tests live in hirsel-driver/tests/stdlib_graphemes.rs.

`text.rs` implements string.from_bytes, string.slice,
string.as_bytes, bytes.slice, string.position,
string.byte_offset at their declared Hirsel ABI. Fresh results use
`hirsel_rt_v2_bytes_descriptor` (Sequence[u8]: u64 count, then bytes). UTF-8
conversion transfers the same slice into either Result arm; slicing shares one
owner credit and preserves its origin. bytes.replace is ordinary std source.
StrPos's nominal u64 storage travels by read-only address. SEM-3 exports return
status 1 with output unwritten; emission records the manifest's rule at the call
site. `hirsel-pipeline-tests/tests/front/rt_text_native.rs` checks the full native matrix, C callers under
ASan/UBSan on x86-64 and qemu on AArch64, and generated drop glue with zero
live blocks after every one of 1000 calls at O0/O2 in both runtime modes.

The byte bridges retain one Slice owner credit and its exact interval. Freezing
List[u8] is a checked LIR Slice construction in hirsel-rc and has no runtime
entry. Position Option initializes both tag and inactive bytes; Some holds the relative offset without another owner credit.
Path text reads only the bound lexical intern table and emits no host request.

Ordinary tape snapshots retain physical directories/files and relative link metadata.
Replay resolves aliases inside each pinned directory grant, so writing through an alias
updates its target. Traversal through invalid link targets and access to byte-invalid host names return
operation faults; they do not reject tape admission. Exclusive temporary creation uses the
kernel's O_EXCL|O_NOFOLLOW decision before link expansion. Commit checks that its
source is the prepared regular inode. Ordinary tapes model ordered program effects,
not arbitrary concurrent mutations of host directory entries.

`host_paths` implements the filesystem obligation of `HostAuthority::authorize`.
`PinnedDirectory` owns the acquired mount descriptor. Admission passes named
selectors, `Access` rights and the exact pins to `RealIo::from_pinned_mounts`;
an empty iterator acquires no directory. `mount_pins` borrows every retained pin,
and `prepare_recording_with_files` checks all of them before recorder effects.
Directory acquisition matches only a configured selector or its descendants;
missing mounts return FileError::NotGranted even when another mount grants Dir.
Narrowed directories retain their parent's read/read_write rights. Write, remove,
atomic creation, append and commit refuse read grants before storage calls.
`SimIo::set_mounts` configures the same independently enforced replay policy.
Snapshots use virtual selectors and retain same-resource/contained mount aliases,
so their publications share storage while each issued grant remains confined.
`RecorderDestination::pin/open` checks every grant by physical device/inode
ancestry before creating or truncating tape/tool output, follows existing aliases,
and exclusively creates new leaves relative to pinned parents. A file beside a
grant in their shared parent is isolated and accepted. `RecorderDirectory::pin/check`
also rejects grants inside directory-owned recorder storage, for journal adapters.
Every bind mount view requires matching descriptor identity; unavailable physical
ancestry refuses admission. Existing file inodes are checked against the granted
physical trees, including hard links and file bind mounts. Tape, observation and measurement
outputs are pinned before entry; no output path is reopened after entry.
The launcher also pins inherited regular-file stdout/stderr sinks before opening
recorders and refuses identical recorder inodes. Pipes, terminals, sockets and
closed descriptors do not collide with regular files. File-backed provider
adapters pass their acquired pins to RecorderDestination::open_with_files.

Native launches stream without retaining duplicate output/event histories in memory.
The public RealIo adapter retains history by default for tests and local observers.
`harness/output-bench.c` compares this ordinary path and ordinary recording
against identical raw C writes; neither mode claims durability.

## Foreign calls

An extern call (PROG-4) first calls `hirsel_rt_v2_foreign_enter(ctx)`, which records the
run context for its thread. A C callback thunk reads it back with
`hirsel_rt_v2_callback_context`, which ends the process when the thread has none, and a
callback that fails calls `hirsel_rt_v2_callback_failed`, which ends the process: the C
frames between the callback and the extern call cannot run cleanup. The launchers strip
the `standalone::UNREPLAYABLE` bit from the compiled root mask and refuse a recording or a
replay when it is set.

## Timing, diagnostics and measurement archives

Cargo features are independent link-time choices, all off by default:

- `system-allocator`: each counted heap block gets one libc `malloc` and one
  matching `free`, bypassing slabs and free lists. Small requests keep the exact
  class capacity and physical alignment. A fixed-size layout's block (descriptor
  `LAYOUT_NO_EXTENT`, no wide cursor, at most 8-aligned; never a sequence payload,
  `rt_program::no_extent`) is bare: the malloc is
  its header and class capacity, the header is the allocation, and its capacity
  is its descriptor's. Other blocks keep the large prefix (per-block capacity for
  compiled queries, a wide cursor's shadow, an over-aligned payload inside an
  over-allocation whose original pointer is retained). A bare token reused for a
  layout that cannot be bare moves its bytes to prefixed storage of the same
  capacity. Production does not list bare blocks, so one still live at task exit
  stays with the process; diagnostic and measurement archives list them, count
  them as live blocks and backing (`large_bytes`) and return them at exit. The
  ordinary eight-byte header, descriptor registry, count/drop behavior and
  reuse-token key remain.
  Regions keep their bump semantics; the system-allocator comparison disables placement.
- `diagnostic`: builds Stats, backing-call and saturation counts, and the exit
  leak audit. Launch-time `CONFIG_DIAGNOSTIC` still selects poisoning separately.
  Correctness tests and native matrix tools select this archive, including their
  poisoning-off configurations. C Stats consumers define `HIRSEL_RT_ENABLE_STATS`.
- `memcheck`: Valgrind memcheck client requests (`src/memcheck.rs`, transcribed from the
  vendored `third-party/valgrind` headers) for the runtime's own blocks, for the
  `native --sanitize` campaign; production archives contain none. Each slab is a mempool of
  its live class blocks, with a 16-byte redzone after every slot, and each lent region segment
  a mempool of its reservations, with a 16-byte redzone after each: free slots, redzones, a
  reset token's payload until its reuse, a relocated builder's old storage, unreserved region
  tails and the segments of an exited region or arena are unaddressable. Compiled small
  allocation, free and arena bumps all take their runtime entries (null pages, a zero
  inline-free flag, a null region limit), so emitted code carries no requests and is the
  object production links; inline dups stay. A slab or segment returned with blocks still
  live drops them with its pool.
  `hirsel-driver`'s `tests/it/memcheck.rs` links a C program against this archive and
  launches it as the campaign does: a read of a released slab block must exit 97, and its
  twin that reads only before the release must exit 0.
- `measurement`: builds Stats/auditing plus the counters below. Direct launch
  reads `HIRSEL_RT_MEASUREMENT` once to open its report before entry; this is a
  measurement-tool output and is outside semantic RunConfig and recorded tapes.
  Rust callers pass an admitted file to `RunCtx::set_measurement_output(file)`.

Production exports no Stats function, records no backing-call/saturation counts
and never scans live storage at exit. Its observation says `audit unavailable`.
Its allocation metadata still tracks free-list availability and pool retention
needed by the allocator. `tests/it/abi.rs` reads Buck2's independently built production archive (or
builds it separately under Cargo)
and checks private/exported symbols for absence of measurement, backing counters,
Stats and exit-scan code, and bytes for absence of the measurement environment
name/report header. Runtime Rust tests enable `diagnostic` only as a dev feature.

The driver selects archives with `RuntimeBuild { diagnostic, system_allocator,
measurement, memcheck }` and includes that choice in the content key. Linking a measurement
archive also sets LLVM's `EmitInputs.measurement`: every inline count edge gets
its hook once, including shared/immortal edges, and a spent glue budget lets the
runtime record the dying edge. A measurement archive keeps the compiled fast
paths' dup bound and inline-free flag zero and its pages null, so every dup,
allocation and free still reaches the runtime and is counted there; detach remains a
runtime call. The optional hooks are declared in `include/hirsel_rt_measurement.h`,
separately from the production runtime ABI. No uniqueness read is an event.
A timing object has no measurement imports; before/after
SHA-256 for the unchanged PROV-1 copied-prefix corpus object at this cutover is
`3ea17b189286b4a93483fc68e89f03f149528dd0bb61f562ac20f387dd5f5f28`.

The exit report is newline-delimited text, starting `hirsel-measurement 1`.
There is one `class CAPACITY ALLOCATIONS FREES` row per `CLASS_SIZES` entry,
then `class large ...` for capacities outside that schedule. Bins are logical
payload capacities, independent of allocator metadata/storage kind/alignment.
They count physical block allocations/frees, including sequence relocation;
reset/reuse retains the same allocation. The remaining rows are `KEY VALUE`:

- `count_increments`, `count_decrements`: holder-operation attempts, including
  static/immortal no-ops, final-credit drops and reset's consumed donor credit;
- `cow_detaches`, `deep_copies`: shallow-detach/deep-copy calls; `copied_blocks`
  and `copied_bytes`: blocks and capacity bytes copied by those walks;
- `reuse_hits`, `reuse_misses`: non-null token reuse and null-token allocation;
- `live_bytes`, `peak_live_bytes`: live/draining/raw heap payload capacity bytes;
- `region_high_water_bytes`, `region_pool_high_water_bytes`: simultaneously lent
  and idle segment bytes, including segment headers/unused capacity; and
- `arena_high_water_bytes unavailable`: native arenas are dependency-owned.

The report snapshots before forced task-exit reclamation, so an unbalanced run
retains its live-byte evidence. A bad report path refuses launch before entry;
a write failure terminates environmentally. The output file is consumed once,
so the launcher's task exit followed by context destruction cannot overwrite it.

`tests/it/measurement.rs` hand-computes all counters on a shared pair, shallow and
sharing-preserving deep copies, reset/reuse and nested regions. The native tests
run every named conformance smoke case through all four allocator/measurement
choices, compare binarytrees C runtime-harness output on both targets, verify
inline count telemetry, compare the production object's hash with ordinary
emission, and prove that a leaked block fails the diagnostic observer while its
freed twin passes. The source benchmark's box-pattern access remains spec #98;
its existing source/expectations are untouched.

Runtime replay and fake Test trees reconstruct their own local file identities
from initial entries and ordered publications. Open retains an identity, narrow
compares it with the currently resolved entry and transfers or closes it, and
replacement never reuses an identity even for identical bytes. Atomic appends
preserve the prepared identity and commit moves it. Exact replay validates these
changes before publishing state; identities are not host inode values or new
tape fields. Disk narrowing continues to compare device/inode metadata.

Compiled capacity queries read the existing repr(C) metadata through the C
header's read-only offsets: small headers identify their 64 KiB slab's pool,
large headers retain a prefix pointer before the header, and immortal headers
retain an inline Extent. Offset assertions in alloc.rs and the hirsel-lir
header-transcription test bind these loads to both allocator feature builds.

Measurement launches also write JSON source counts beside the existing text totals
(`<HIRSEL_RT_MEASUREMENT>.sites.json`); `HIRSEL_RT_PROFILE` selects an explicit JSON
path. Both destinations are pinned before entry. Schema 1 contains `totals` and a
`sites` array. Each site has `id` (u64), `package`, `file`, one-based `line` and Unicode
scalar `column`, `function`, initiating LIR `operation`, and `counts` keyed by counter
kind. Kinds: allocations, frees, count_increments, count_decrements, cow_detaches,
deep_copies, copied_blocks, copied_bytes, reuse_hits and reuse_misses. Absent kinds are
zero. Alloc/free totals sum physical class bins; copied_bytes retains physical capacity
units. Every total equals the sum of site counts. Site 0 denotes runtime work with no
emitted source scope; gauges remain in the text report because peaks do not sum.

The object embeds each site's immutable metadata; measurement-only enter/leave hooks
scope the site over emitted calls. Runtime walks and generated release glue inherit
that site, while callbacks can install their own and restore it on return. The existing
`Counters` API and text format are unchanged. Production builds cfg-elide all source
accounting, output parsing and hooks along with the original measurement code.

`io_net_script.rs` independently implements the frozen scripted-peer state machine.
Its socket-convention vectors and script tests live in `tests/it/net_script.rs`;
the tests use the public NetProvider seam. Host policy and stream branding belong to the typed Io
adapter; the script provider owns peer queues and byte state and never opens sockets.
Unbounded unanswered connects and stalled reads produce a distinct Blocked signal.
A `Fails(f)` peer fails every handshake with `Tls(f)` (State closes the taken stream as
for any handshake failure), and a `Truncate()` step reads as `Tls(Truncated())` on a
secured stream and as end of stream otherwise.

`test.net` decodes the nested core peer values through `test_net_abi.rs` and creates
an isolated ScriptNet in State, with peer-derived rules. Every request and result
uses tape v4; replay reconstructs script worlds and checks their outcomes before
publishing state. `RealIo` and `SimIo::set_net_script` configure the root script world
before entry, independently of explicit `set_net_rules` policy.

`io_net_script::ScriptIo` is a cloneable socket-free adapter for driver-owned
oracles. It shares State's network policy and ScriptNet worlds with native
execution, accepts typed network requests/projection/close, and never calls
production allocation/count/drop helpers or acquires host network authority.

Type-store test fixtures use `hirsel_types::testing::Types` through the test-only
`testing` feature; layout derivation and validation remain in this suite.

Type-store test fixtures use `hirsel-layout` for type-to-layout and ABI derivation.

`host_launch::HostGrants` implements production RunConfig authorization: it pins every
explicit directory mount, sets exact projection and clock grants, and installs the
validated Net rules and optional scripted peers in RealIo. Native source-run launches
consume `--hirsel-run /absolute/run.json`, verify its embedded identity and seeds, and
admit it before entry. Args come from that record. The differential harness retains its
explicit fixture launch profile. Listener listen/accept/local_addr exports use the same
Net policy path as streams, including the listener's brand on accepted streams.
`ScriptNet` serves scripted listeners (test.net's listeners, `net.script@2`'s
`listeners`): accept takes the address's next client, then `ConnectionAborted`. A write
contradicting a client's Expect, and a close or drop leaving one unmet, complete as
`Checked(false)` after the Test report. Their exports (`net.write`, `net.write_all`,
`net.close`, `tls.close` and the handle close thunk) have no manifest failure rule and
record the failure themselves at the fixed runtime codes of
`hirsel_core_contract::RUNTIME_RULES` (CAP-2 for a refused handle, LANG-9 for a Test
failure), at site 0 for the caller's failure edge to anchor, or the drop's anchor.

Standalone launch uses `hirsel_rt_v2_launch_standalone`, bound to semantic code
and the exact entry. `--hirsel-run /absolute/run.json [args...]` selects the shared
RunConfig parser; trailing process Args replace the record snapshot before fresh
admission. `-- [args...]` supplies only Args; no record or delimiter grants no
external roots. Compiler adapters receive the root table [Io, Args, Env, Unsafe,
Test] and forward only declared parameters, in source order. The standalone path
always uses admitted host grants and never the conformance fixture profile.

`--hirsel-grants grants.json [args...]` binds a template to the standalone
executable's embedded program and entry, then follows the same validation,
argv snapshot and host admission path as `--hirsel-run`.

Production record and replay are explicit launch inputs, never environment variables.
`--hirsel-record <tape>` before either launch input streams the admitted run's tape
(`host_launch::record`): the header binds the canonical record, its admission time and
the mounts' initial tree before entry, and each completion is flushed before delivery.
The tape must lie outside every granted mount and inherited output file.
`--hirsel-replay <tape>` alone replays it (`host_launch::replay`): it parses the tape,
checks its run record against the header and recorded admission (`Recording::bound_run`),
checks program, entry, launch kind and roots against the executable, then runs SimIo
with the record's projections, mounts, Env snapshot, clocks and Net rules. Replay needs
no grants and acquires no host resource; a recording of other code refuses before entry.
Source-run launches (`hirsel run --native`) accept the same two spellings beside
`--hirsel-run`, and the interpreter's host launch uses the same helpers, so a tape
recorded by either engine replays in the other.

Stdin and stderr are independent Io projections. RealIo pins explicitly supplied
Read/Write providers, retries interrupted reads, captures accepted stderr prefixes
and records every typed stdin completion. SimIo replays those completions without
a reader and rechecks their handle and maximum. Env reads use the exact admitted
name-to-optional-value snapshot: missing keys are denied, null keys are unset.
Host input tape forms live in host_io_tape.rs; fixed ABI thunks share ordinary
byte-list COW growth with network reads.

Incremental File requests keep a read position and provider mode in the logical
handle. Host reads use bounded pread on the pinned descriptor; create truncates a
regular file and pins an O_APPEND writer. Replay retains inode contents across
unlink/replacement, checks delivered byte prefixes, and publishes cursor changes
only after tape agreement. File read grows the caller's List[u8] with the existing
sequence allocator; LLVM uses the fixed operation ABI declarations.

FileError's portable conditions map from host ErrorKind and are derived independently
by the memory tree and checked-heap model. Tape result/fault records preserve each
condition and committed write prefix. FileFault remains the fallback and confinement
refusal; output and stdin keep their own fault domains. The Result ABI keeps the
existing FileFault/Canceled/NotGranted ordinals and appends the portable nullary variants.

`net_host::CooperativeNet` reuses HostNet's socket conventions and immediate operations
with nonblocking sockets, retained connect/write/TLS progress and readiness tokens.
`readiness::Readiness` holds every parked host operation's descriptor interest under its
token and exposes zero-wait polling and an idle-only wait to the interpreter's driver.
`host_stdin::CooperativeStdin` pins a close-on-exec duplicate of descriptor 0 at admission:
a pipe, socket or character device becomes nonblocking (restored on drop) and an empty
read records POLLIN interest; a regular file or block device is read directly and always
ready; a closed descriptor 0 reads as end of input.
DNS runs outside the evaluator and signals a descriptor; relative timeouts retain one
deadline across readiness retries. Partially published writes/TLS exchanges cannot be
reported as effect-free cancellation. HostNet remains the native blocking provider.
`HostGrants::cooperative` (the interpreter, always) and `HostGrants::new` (native, when the
record grants `io.tasks`) install CooperativeNet and CooperativeStdin over one Readiness,
which `RealIo::set_cooperative` also receives; without tasks a native run keeps the blocking
providers. RealIo answers the `Adapter` wait methods from that Readiness and records a
`blocked` completion for every request that parks (with whether it already committed part
of its effect) and a `ready <wait> <polls>` event for every ready poll: the wait's creation
ordinal and the failed polls since it parked or was last ready (`readiness::WaitLog`).
SimIo serves both forms, so replay parks and readies tasks as the recording did and never
waits; an idle replay whose next event is not a parked wait's `ready` is missing input.

`HostGrants::read_revocations` installs an explicit immutable revocation-list
snapshot before admission. Without it a grant requiring revocation evidence refuses.
Ordinary tapes retain the accepted reference, file digest and decision beside the
admission time; replay reads none of the current host evidence.

Scripted port-0 listeners bind their client queue to the first ephemeral listen on
that IP. Each world allocates from 49152 upward, skips script-reserved and bound
ports, and shares the sequence through narrowed Nets; independent worlds restart
it. Outbound connects do not consume ports. Listen completions retain the assigned
address for replay.

Filesystem lifecycle operations share the descriptor-confined parent resolver.
Metadata pins the final entry with O_PATH|O_NOFOLLOW; read_dir classifies direct
children without opening special files. Ordinary rename uses pinned parents and
renameat, while atomic commit retains its prepared-inode validation. Empty
directories, entry kinds and optional metadata have explicit simulated state;
host timestamps and directory sizes are recorded metadata inputs. Denied writes
return before storage activation. Tape v6 adds typed entries, metadata, directory
mutations and the DirectoryNotEmpty/CrossDevice errors.

Snapshots retain hard-link equivalence through run-local canonical names, without
exposing host inode numbers. Simulated mount pins and issued directory grants
retain identity across namespace changes, including empty removed directories.
The checked heap model keeps logical handle paths separate from the current
resolved directory roots, so held files and atomic temporaries follow their Dir.

Zero-size flat List elements keep a stored count after their byte-extent
prefix. Codec reconstruction preserves that count even when no element bytes
are written; zero-size record fields retain their ordinary layout identity.

Standalone `--hirsel-run` and `--hirsel-grants` accept inline JSON when their
argument starts with `{`, otherwise a path. Both forms use the shared RunConfig
input selection and schema validation before binding checks and admission.

Every compiled source program registers complete runtime diagnostic locations.
Standalone launches report the failure chain on stderr; observed tool launches
leave reporting to the driver. Reports remain outside program Io and tapes.

Failure records retain an optional original operation site independently of the
primary attribution. Semantic observations compare both locations. Runtime ABI 17
passes both site codes to failure_begin and failure_reanchor; a reanchor fills an
unknown attribution or operation once, preserving already anchored causes.

The semantic conformance harness can explicitly select `HIRSEL_RT_VIRTUAL_CLOCK`
with a u64 nanosecond start. Its RealIo samples and cooperative timer readiness share
one virtual reading: runnable work never advances it, and an idle wait advances it
to the earliest remaining timer. The ordinary park/ready tape protocol records and
replays those waits. This setting is consumed only by the harness launch branch;
production admission uses the selected RunConfig's clock grants.
