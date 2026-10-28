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
- A run, not the process, is the unit of failure (`program.rs`, `stop.rs`, runtime ABI 20). The
  generated `main` passes one `hirsel_rt_v2_program_descriptor` (entry, descriptor registry,
  failure table, semantic digest, entry name, roots, flags `UNREPLAYABLE`, `STANDALONE`,
  `ESCAPES` and `FUELED`, and
  a launched executable's bound run configuration) to `hirsel_rt_v2_main`, which loads it into
  a `Program`; a run's context reaches its own program's failure table, so runs of several
  programs share a process. A run's root task runs on a fiber, and Blocked, an adapter refusal,
  a replay divergence, a codec limit and a failed serializable check are `Stop`s: the runtime's internal functions return
  `Result<_, Stop>` up to the entry compiled code called, which switches back to the launcher's
  stack; the launcher abandons the run's fibers, destroys its context and receives a
  `RunOutcome`; a failed check's `Stop::Failed` ends the run as a failed entry with the
  check's recorded failure and no cleanup (FAIL-3); like a stopped run it is
  abandoned, with no exit audit. Only the standalone launcher turns an outcome into a process exit status
  (`RunOutcome::exit_code`). Invariant faults, host exhaustion, PROG-4 violations, stack
  overflow and a stop inside a foreign call still end the process.
- A `host::Host` admits a bound run once (`Host::admit`: grants, mounts, revocations, clock and
  the process's standard input) and starts it (`Host::start`, then `RunHandle::wait`) with its
  own `Sinks`: the granted stdout and stderr and a `report` stream for the run's failure,
  PROG-3, refusal and stop reports. `Sinks::process()` are the process's streams;
  `Sink::capture()` keeps a run's output in memory. Standard input is one take-once
  `ProcessStdin` token, held from admission by the run whose record grants `io.stdin` and
  returned when its Io drops; admission refuses a second holder. `Execution` groups the runs
  that decide CONC-8 together: each started run begins its own until callee runs (`Selection`
  for calls, runner-core) join their caller's. In the interpreter's host launch the runs of an
  execution share one `Readiness`: a callee's tokens carry its tag in bits 48..61
  (`Readiness::tagged`), each run takes only its own (`ready`), `deliver` readies a caller's
  runner wait when its callee ends, and `has_ready` lets the driver ask every run before any
  waits.
- Runner calls are tape events (RUN-11): `runner_call` (the runner, form, path, grants,
  allowlist, call key, digest and argument bytes) completes with `runner <outcome> <bytes>
  <callee instance|none>`; `runner_cancel` and a callee's last event `call_return
  ok|err|failed <bytes>` complete `ready`. The header records the `run_instance`.
  `tape_links::check` is the joined check over a set of tapes (`hirsel tapes check`): each link
  with both ends present must agree on digest, argument, form, path, callee instance and
  outcome bytes; a link with one end missing is unchecked.
- A launcher that reads the observation record also reads how a run stopped (`stopped
  refused|diverged|environmental ...`) and, from the file named by `HIRSEL_RT_REFUSAL`, a launch
  refused before entry (`refused <reason>`, a `RunRefusal`'s JSON when the host refused it);
  the executable then writes no report of its own. Without them it reports on its report sink.
- Stops and Rust frames: a stop switches from the frame that raises it to the run's driver, and
  the frames between are never unwound. Every place Rust enters compiled code holds only raw
  pointers and plain integers across the call, so abandoning it leaks nothing the context
  does not own: the root entry (`launch::execute`, inside `stop::run_root`, which is the
  driver), task code (`tasks.rs`, each task on its own fiber, under scheduler frames) and
  finalizers (`finalize::run_finalizer`, from `release.rs`'s drain and step walks and from
  `hirsel_rt_v2_finalize`). Compiled code calls an isolate callback itself, with no Rust frame
  between. A stop inside a foreign call (C frames between) ends the process. A new Rust frame
  that calls compiled code must keep to plain data, or return the stop through `Stopped`.
- Foreign calls push their context and source site on the calling thread, then restore
  the enclosing call when they return. Callback thunks reject null context/Ptr/FnPtr
  arguments before invoking Hirsel. A non-null boundary violation ends the process
  through `foreign_violation`, reporting PROG-4 and the active extern's source site;
  nullable pointer options use zero as None and require no guard.
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
  the cap (1 MiB default) return at region exits and worker collection points,
  including runs with long-lived tasks; task exit returns every backing block.
- One immutable descriptor registry belongs to a run context. The driver assigns it before
  partitioning the linked program and every emitted object receives the same assignment.
  Runtime-owned bytes/file-walk descriptors reserve indices 0/1, empty environments use 2,
  and canonical layout-descriptor digests order the remaining indices. The run context
  appends one more runtime descriptor after the program's, the `List[Bytes]` payload
  (`text::BYTES_LIST_DESCRIPTOR`, at `text::bytes_list_index`), so runtime results can hold
  byte strings; a test checks its steps, flags and charge against the derivation on both
  targets. ABI v2 entries accept
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
  page of another worker's heap (its `owner` is not the context's `heap_id`, runtime ABI 26), a
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
- Release is iterative, with no allocation or recursion through data. A block is freed after
  the children its steps kill (post-order), except at a `RELEASE_LAST` (descriptor version
  11): the release that ends its value's program, through the arms of a last variant. When it
  kills its child, nothing of the block remains to run, so the block is freed at once and the
  child takes its place under the same parent (a tail release): a list's links are freed head
  first in one pass, never suspended, and a list built tail first, as constructors build it,
  is handed back by the LIFO free lists in its build order. A kept or masked root runs to its
  end.
  A last release of a payload with no finalizer or drop steps frees it directly,
  including an erased String/Bytes owner: there is no cursor or worklist to drain.
  Simple dead blocks retain their index and use the count word for the next step. Loop/column
  descriptors set WIDE_CURSOR: their complete 24-bit step and 40-bit mixed-radix cursor spans
  the header plus a pre-reserved four-byte slab shadow (or a large prefix shadow). The parent
  link uses the consumed reference slot. The root's bytes stay intact for Reset/Reuse.kept.
  A masked root that runs nothing (no finalizer, and the mask leaves no top-level step live:
  every counted field moved out) is not walked: its reset makes it the token and its release
  frees it at once. Compiled code makes the same test statically
  (`hirsel_lir::rt_program::masked_inert`) and resets such a unique heap block inline.
  Resumption reconstructs frames from immutable steps, preserving DROP-2. Diagnostic descriptor validation rejects control words aliased with consumed
  slots, allowing the deliberately shared null-pointer niche guard and words in exclusive
  arms of one ordinal tag (a nested tag in one arm may share bytes with a sibling arm's
  reference: only one arm runs). Finalizers and closes
  run shielded; cleanup keeps the first failure and records later causes. Unsafe-ffi finalizer
  failure terminates. Compiler-reserved inline continuations are separate 32-byte records.
- Compiled drop glue (hirsel-llvm) shares the count word under one written contract
  (`hirsel_rt_v2.h`, transcribed in `hirsel_lir::runtime_abi`): a plain task-local `u32` at
  `COUNT_OFFSET`; at `COUNT_UNIQUE` the block dies, `COUNT_IMMORTAL` is never written, any
  other count is stored one lower. The glue releases a dead block's fields itself, in the
  drop program's order and with the walk's tail releases, and returns the storage through `hirsel_rt_v2_free(ctx, block)`
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
  size/alignment; whole-block copies retain source capacity. A String/Bytes interval
  copies only its viewed bytes into a fresh byte sequence, resetting its start. Equal
  intervals share one copy within the graph; different intervals of one owner copy
  separately. Descriptor version 15 marks a slice's release with stride 24 without
  changing the holder scan or the entry ABI. Its visited map and work stacks are
  runtime-owned, so the native stack stays constant over any depth.
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
  its held slots double or meet the request, whichever is larger, within the host extent bound.
  This amortizes repeated incremental reservations such as direct formatting. Tracked arena
  reservations retain their hint-only behavior and exact logical charging. The count and the
  counted elements move bitwise into a block of the class that fits (the new
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
  count is the root count. Root counts are exact and atomic (runtime ABI 25, `roots.rs`, S-4,
  S-5): compiled code adds a root by a CAS below the dup bound, else `hirsel_rt_v2_root_dup`,
  and gives one up only through `hirsel_rt_v2_root_release` or a walk's `STEP_RELEASE_ROOT`
  (step 11; layouts say which slots are roots with `ScanStep::Root`/`DropStep::ReleaseRoot`).
  Past the run's finite maximum the header holds `ROOT_OVERFLOW` and the exact count lives in
  the run's overflow table, so a root never saturates. A release applies at once at width 1;
  a non-head ordered turn defers it to its retirement (`roots::retire`, the ordered
  executor's hook); in free mode a release that does not find the count at 1 joins the task's
  pending list, coalesced per root and cancelled by a later dup that reaches the runtime, which
  the next op's entered phase applies (Amendment F0-1: that phase's walks release nested roots
  at once). `thaw_turn` answers by the exact count read with acquire ordering, so an overflowed
  root is never the last. Diagnostic mode faults on an ordinary count operation reaching a
  root. `hirsel_rt_v2_freeze(ctx, root)` walks the
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
  frozen graph. A threaded scheduler also needs a frozen graph whose blocks no thread's heap
  reclaims before the last root's thaw or release.
- Keyed hashing (SEM-5, runtime ABI 21): `hash_begin(ctx)` reads the run's
  configured seed. Scalar leaves use one-shot `hash_bytes`; a composite creates
  one 48-byte SipHash-1-3 state with `hash_init`, streams its framed leaves with
  `hash_update`, and calls `hash_finish` once. Both native engines use the same
  framing and canonical Set order. The seed changes placement only.
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
  Map or Set carries an empty (unindexed) index. A Path node encodes its run-context
  identity's text and decodes by validating UTF-8 and interning it as `path.of` does.
  Program version 3 also derives private Map tag, cached-hash and live-count
  offsets; it skips tombstones and preserves the unchanged wire format. It walks
  private flat framing, active Packed extents and SoA columns in row order. Program
  version 4 (runtime ABI 23) adds the root layout's descriptor index and CHECKED nodes,
  whose checks are the codec's `hirsel_rt_v2_check_thunk`s: after materializing, decode
  re-walks the value as encoding does and calls each check after its value's inner
  occurrences; the first verdict 0 releases the value and writes `Error(Invalid)`.
  Program version 5 applies the private tombstone entry metadata to Set too,
  using the same optional key/unit pairs as Map.
  Flat reconstruction patches native byte extents separately from wire element
  counts; sequential blocks retain exact used bytes and SoA blocks retain rounded
  column capacity. Both walks are iterative. Past
  `hirsel_types::wire::Limits::default`'s byte and value budgets the run stops with
  `Environmental(CodecLimit)`, as the interpreter's operational limit ends it.
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
`State::execute_process` checks every `proc.run` and every stage of a `proc.pipeline`
(an empty one is InvalidInput) before any effect: the Proc and Dir handles, then `hirsel_run_schema::Command::check`: a canonical absolute program,
NUL-free arguments and a well-formed exact environment (InvalidInput), then the exact
rule and its environment names (NotGranted); a scripted world (`process.script@1`,
`test.process`) is served from its entries, and only a checked host run reaches the
adapter's provider; both are `run_stages`, a run being a one-stage pipeline
(`PipelineOutcome::into_run`). `RealIo` runs one with `run_host` over `host_command`: std `Command`
with a cleared environment plus the rule's pass-through names (`ChildEnv::Inherit`) or
exactly the command's variables (`ChildEnv::Exact`), `/` or the pinned Dir as working
directory. Each stage's stdout is the next stage's stdin, every stage's stderr is a
clone of one `O_CLOEXEC` pipe, and a stage that fails to spawn kills and reaps those
before it. Every host run, child and group keeps its children in `host_stages::Stages`,
which spawns them (a command with `own_group` makes the first stage lead a new process
group that every later stage joins), signals them (each unreaped stage, the last first,
then the group with killpg(2)), waits for them and reaps them (the last first, so the
group's leader, whose pid is its id, is reaped last), records each reaped status so no
reaped pid is signalled, and kills and reaps whatever is left when dropped.
`Stages::terminate` sends SIGTERM, polls with `waitid(WNOWAIT)` until every stage has
exited or `TERMINATION_GRACE` passed, then sends SIGKILL, which in a group reaches what
the stages started, and reaps. One `poll` loop feeds the first stage's stdin and drains the last stage's
stdout and the shared stderr, killing every child past `max_output` on either stream. A `Deadline` is the Instant's CLOCK_MONOTONIC ticks: a reached one is
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
grant JSON from `HIRSEL_RT_PROCESS`, for live runs and replay alike. `proc.narrow`
(`ProcessNarrow`, tape `process_narrow`: a rule count, then each program and its counted
names) mints a Proc of the merged requested rules in its parent's world when
`hirsel_run_schema::process_rule_within` covers each. Tape forms are `process`, `test_process` (runs and
children), `process_run` (with its deadline as `clock:ticks` or `none`, then its
command: program, directory, counted arguments, and `inherit` or counted variables) and
`process_pipeline` (a stage count, then each stage's command, which ends with the word
`own_group` only when it is set) requests and `process`
and `pipeline` (stdout, stderr, then counted statuses) completions (`host_io_tape.rs`). A recorded `TimedOut` needs a deadline, and
raises the latest monotonic reading to it as a completed timer wait does; a scripted
world times out only a deadline that reading already reached.

`proc.spawn` makes the same checks and mints a `Grant::Child` of its Proc, which every
child request names (CAP-2); the policy tracks whether its input is open. A scripted
world plays the entry with `hirsel_run_schema::ScriptChild`: a read at an `Expect` is
`AdapterError::Blocked`, and a contradicted or unmet `Expect` writes its Test report and
completes `Checked(false)` (LANG-9). A host child goes through `HostCall` to
the `process::ProcessHost` provider, `host_child::HostChildren`, three nonblocking pipes per child: a write or read that would
block records its pipe as the token's interest and parks (a write that accepted bytes is
committed, so it is no longer a cancellation point), and without cooperative admission it
polls that pipe. `child.wait` closes input, discards the remaining output and reaps, as a
`HostJob` under cooperative admission; at its deadline the child gets SIGTERM, then SIGKILL
after `TERMINATION_GRACE`, and the wait completes `TimedOut`. `child.kill` sends its
`Signal` with kill(2) to the unreaped child; `host_command` resets SIGINT and SIGTERM to
their default dispositions in every child, so a launcher that ignores them cannot make a
child ignore them. Dropping a Child (`Request::Close`) closes its
input, SIGKILLs and reaps it, then closes its outputs; a cancelled wait drops the Child
it took (`host_waits.rs`). Child tape forms are `process_spawn`, `child_write`,
`child_close_stdin`, `child_read` (with `stdout` or `stderr`), `child_wait` (with its
deadline or `none`) and `child_kill` (with `interrupt`, `terminate` or `kill`) requests, the dropped Child's `close`, and `spawned`, `child_unit`,
`child_bytes` and `child_exit` completions; `SimIo` replays them without spawning.
`proc.spawn_pipeline` makes every stage's checks and mints a `Grant::Child` whose `group`
holds its stage count and `max_stderr`; `group.write`, `group.close_stdin`,
`group.read_stdout` and `group.kill` are the child requests (and the child ABI entries) on
its handle, a standard-error read of a group is refused, and `proc.wait_group` is
`Request::GroupWait`, whose completion must hold one status and one at-most-`max_stderr`
standard error per stage. A scripted world plays the entries with
`hirsel_run_schema::ScriptGroup`, its stages connected. A host group (`host_child.rs`) is a
`HostChild` of several stages: the program holds the first stage's input and the last
stage's output, and a `Capture` thread drains every stage's standard error into its own
buffer from the spawn, SIGKILLing every stage (a copy of the `Stages` targets) once one
passes the bound; no stage is reaped before that thread ends, so every pid it signals still
names its stage. Every per-stage signal (a kill, the bound, a deadline's SIGTERM, a drop)
goes to the last stage first, so no stage can read an earlier stage's death as end of input
before its own signal arrives. The wait discards
the last stage's output, waits for the capture, reaps every stage and returns
`GroupOutput`, its stderr a runtime `List[Bytes]`. Tape forms are `process_spawn_pipeline`
(`max_stderr`, the stage count, then each command) and `group_wait` requests and
`group_exit` completions.

## Signals

`io.signals` is `Request::Signals` and `signal.wait` `Request::SignalWait` with the waited
kinds as `SignalKind::bit`s; the completion is `Completion::Signal`, the consumed kind or a
`SignalError` (`Canceled(cause)`, `NotGranted`, `InvalidInput`, checked in that reverse
order: an empty set, then a kind outside the grant, then cancellation). Tape forms are
`signals <io>`, `signal_wait <signals> <bits>` and the completions `signal <kind>`,
`signal canceled <cause>`, `signal not_granted` and `signal invalid_input`. The native entry
`hirsel_rt_v2_signal_wait` reads a `List[SignalKind]` of u32 ordinals and writes a
`hirsel_rt_v2_signal_result` (tag, then the kind's ordinal or the error's tag, then a
cancellation's cause).

`signals::SignalSource` is the admitted provider. `signals.host@1` takes the process-wide
`ProcessSignals` hold at admission (a second holder is refused `Unauthorized`), installs
`sigaction` handlers for the granted kinds at the first projection and restores the
replaced dispositions when the hold drops. The handler only enqueues: it stores an arrival
number in its kind's pending slot (atomics), saves and restores errno, and writes one byte to
a nonblocking close-on-exec self-pipe; a second SIGINT while one is pending sets `SIG_DFL`
and re-raises. A parked wait registers the pipe's read end as POLLIN interest in the
cooperative Readiness; `RealIo::signals_ready` decides readiness from the pending slots, not
the pipe, and drains the pipe so a spurious byte never spins. Without a cooperative scheduler
a wait blocks in `poll(2)` on the pipe. `signals.script@1` keeps its script and pending
arrivals in the source: `RealIo::wait` delivers the next arrival at an idle point, before
virtual time, and a default action stops the run `Stop::Signaled`, which the launcher reports
as `signaled <kind>` and realizes by restoring `SIG_DFL` and raising the signal
(`process_exit`); its recording gets no trailer. Replay (`SimIo`) installs nothing and
returns the recorded completions. The harness launch records under the cooperative park/ready protocol whenever a signals
grant is present, so a parked wait's interleaving with other parked requests replays; a
`Signaled` run's recording has no trailer, and the native gate checks that instead of replaying.

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

## Telemetry capture

`telemetry.rs` holds a run's `Capture`: where span identities come from (`Ids::Null`, the
grant seed's SplitMix64 stream shared with the interpreter through `hirsel_kernels::telemetry`,
or `Ids::Host`, the host's secure generator via `RandomSource::System`), the identities
already `Issued`, and the null provider's never-draining `Exporter` queue. A begin is a
sampled request: live, `telemetry_draw` chooses identities and `telemetry_sample` admits
them; replay admits the recorded ones, so a forged trace, a zero or a reused ID refuses.
Refused payloads (`TooLarge`, `InvalidParent`) return from the `hirsel_rt_v2_capture_*`
export with no request. `capture.context` reads the span's stored context with no request.
Event, finish, log and measure complete `unit ok`; the `close` of a dropped `Span` is its
abandonment. Exporter congestion is counted, never observed. Tape forms are `telemetry`,
`telemetry_begin`, `telemetry_event`, `telemetry_finish`, `telemetry_log` and
`telemetry_measure` requests and the `span` completion.

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
| a `Map[K, V]`, `SortedMap[K, V]` or `Set[K]` of `n` entries | `n` times `K` plus `V`, or `K` |
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
that block where their relation places them (PROV-4), as a literal does.
SortedMap uses the same `LAYOUT_KEYED` contract, with only its entries List at offset 0
and no index: `sorted_map.empty(by)` places both blocks in the expected arena,
and a vacant insert charges its entry before taking its arguments. An Io read into an
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
`hirsel_type_analysis::logical`; H and native execution charge from layouts
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
ConnectTls stream, or a stream accepted on a ListenTls listener (accepted streams keep
their listener's operation), moves no plaintext before its handshake. `net_host.rs` is the libc
provider, ported from Rust std's Unix socket code and Zig's getaddrinfo lookup: nonblocking
connect with `poll` and one deadline across resolved addresses, SO_RCVTIMEO/SO_SNDTIMEO,
MSG_NOSIGNAL writes. Network outcomes are provider inputs recorded as completions (tape v4,
`net/tape.rs`); `SimIo` replays them with no sockets after re-deriving every grant decision
from its bound rules (`set_net_rules`; launch reads the root Net's `io.net` grant JSON from
`HIRSEL_RT_NET` and binds `net.script@2` peers through `set_net_script`). TLS upgrades the granted
socket through rustls in `net_tls.rs`; subsequent network reads and writes carry
plaintext. Every TLS operation reaches rustls only through a `net_tls::TlsEngine` (material
checks, handshake starts, scripted handshakes) and the `TlsSession` each handshake returns,
which the host providers hold per stream. Only the TLS entries (`tls_abi.rs`, and the
interpreter's TLS operations) name the engine, handing it to the adapter
(`Adapter::tls`) before their material and request, and the policy layer and every world
take it from there; a TLS request without it is an invariant fault. With one function per
section in the archive and `--gc-sections`, an executable whose program calls no TLS entry
links none of rustls, ring, webpki's verifiers or the root set
(`link_kit::an_executable_links_each_host_provider_only_when_its_program_reaches_it`).
Admission only checks that a scripted `Serves` peer's server is well formed
(`hirsel_run_schema::tls::well_formed`); the scripted handshake loads it through the
engine (`TlsEngine::scripted_connect`), and an unloadable one fails that handshake. `tls.close` sends close_notify before closing on every outcome; implicit
drop only closes. PEM bytes pass through `Adapter::tls_roots` to the host provider
for its next handshake, outside the tape request, whose SHA-256 must match. Replay
and scripted providers ignore the bundle and use only the digest. `tls.accept` is the
server handshake (`TlsEngine::accept`: ALPN `http/1.1` only, no session cache or tickets,
the identity `tls::select` picks for the client's SNI, and the rustls client verifier of
a `Request` or `Require` trust set; the host holds either side as a
`TlsSession`). Its `TlsServer` passes through `Adapter::tls_server`; every world
loads it into State with the engine (the shared `hirsel_run_schema::tls::server`), the tape request
records only the chain and trust-set PEM SHA-256s, and the provider receives the loaded
server (`tests/it/tls_server.rs`). `tls.connect_as` passes its client identity through
`Adapter::tls_identity` the same way and the tape names its chain digest. `tls.peer` is a
recorded request whose completion is the DER chain the handshake verified, the server's on a
client and a client's on a server (`TlsSession::peer`).
Scripted worlds
attach independently at the same provider seam. UDP uses `UdpBind` and `UdpPeer`
rules and an originating-Net socket grant. `State::net_error` validates arguments
and authority before host cancellation; `execute_net` discards outside senders,
then truncates one admitted datagram with an explicit flag. `ScriptNet` keeps UDP
queues separate from TCP cursors. Host sockets use sendto/recvfrom, with
nonblocking readiness under cooperative admission. Tapes retain the admitted
prefix, peer and truncation flag, and replay checks their authority without
sockets. Unix stream sockets use exact-path `UnixConnect`/`UnixListen` rules (port 0) and
return ordinary Stream and Listener handles whose addresses are the unnamed `0.0.0.0:0`;
`net_host.rs` binds and connects `sockaddr_un` blocking. A cooperative connect is
nonblocking: facing a full backlog it queues nothing and Linux signals no room, so it keeps
its socket, parks on a 5 ms timer and connects again, cancellable throughout (the stale
probe is nonblocking too). It removes a stale socket only under `UnixListenRemoveStale`, and unlinks a
listener's file at close while its device and inode still match. `ScriptNet` keys
listeners by address string and port, so a socket path is one more scripted endpoint.
`net_abi.rs` exports the datagram and stream
operations at the Hirsel ABI (`NET_EXPORTS`), plus `hirsel_rt_v2_net_read_into`, the
entry of `$intrinsic.NetReadInto` that `net.read_into`'s compiled body calls with its
buffer size. `net.resolve` allocates its `List[SocketAddr]` under descriptor 0, the
runtime's plain pointer-free sequence descriptor. `hirsel-pipeline-tests`
`rt_net_native` records a compiled program against a loopback server and replays it
both natively and independently in the interpreter, with an altered-byte control.
The test-only `hirsel-pipeline-tests` `rt_replay/network.rs` translator reconstructs
independent interpreter script peers from native typed completions. `rt_replay.rs`
compares every
completion, surviving grant, final tree and stdout, then strictly replays the
interpreter's own tape. Corruption controls keep the fixture tape separate from
the comparison tape, including external fault inputs. Partial native writes supply their accepted prefix to the
script; read chunks and EOF remain exact. TLS success, name mismatch and handshake
failure are representable; other TLS failures are refused until the approved peer
schema can express them. PEM events require the original trust bundle and verify
its SHA256 digest. Certificate names are merged across the entire recording;
a fixed peer cannot both accept and reject the same name.

Launch owns a stable `io::Binding`, installs `io::hirsel_rt_v2_io_close` as Config.close_handle, and explicitly binds it with TaskCtx.bind_io_data. The adapter must stay at its bound address and outlive every release on that context. Close-handle runs only while it is bound, with exclusive adapter access; unbind before dropping it. There is no global/thread-local fallback. This private TaskCtx field changes no Config or C ABI.

`Map` and `Set` are native LIR bodies in `hirsel-rc::library::map`. Each container's public length is its live count, while its private optional entries may include tombstones. Removal clears one entry and index bucket; insertion compacts lazily in insertion order, reusing cached hashes. Metadata is neither charged nor tracked. Descriptor version 13 extends the five-word live-count program to private Map and Set entry Lists, so deep copy and detach charge only live key/value pairs and exclude private tags. The compiled seed law in `hirsel-pipeline-tests/tests/rc/rc_maps_native.rs` replaces the Rust Map model. Sequence consumption uses compiled library bodies in `hirsel-rc`; their C and H checks, native target tests and resource failure campaigns replace the Rust Cursor model. The six consuming open/next manifest rows are Body.

## Tasks

`tasks.rs` is the structured-concurrency scheduler (CONC-1, CONC-2, FAIL-2) and `fiber.rs` its
stackful fibers: guarded 8 MiB lazily committed stacks, registered with Valgrind, and a
callee-saved register switch for x86-64 and AArch64. The scheduler is cooperative and
single-threaded, with the reference interpreter's schedule:

- one queue of runnable tasks, from which the run's scheduler policy (`inputs.scheduler`,
  `hirsel_kernels::schedule`) takes the next task at every scheduling point (`tasks::pick`):
  the front under the default `fifo@1`, a seeded choice under `random@1` and `pct@1`, whose
  task priorities live in the task rows. A seeded choice among two or more tasks goes
  through `Adapter::schedule`: `RealIo` records it as a `schedule <count> <choice>` tape
  event and `SimIo` replays the recorded choice. Production launches take the policy from
  their run record; harness launches from `HIRSEL_RT_SCHEDULER` (`name@1:seed`), which the
  test runner sets per explored launch;
- a yield after every capability operation (`yield_now`, called by the Io entries and the task entries);
- a task that must wait (a join, a scope exit) parks with what it waits for;
- every yield and every stop of the running task is a scheduling point, which polls the
  parked tasks whose wait may be over: one that just parked, and those an event woke since
  (a child holding its result or completing wakes its joiner, its scope's exit and its
  scope's set waits; a send or a sender's last close wakes the receivers it can serve, in
  park order; a cancellation wakes the canceled tasks; the adapter reports the ready host
  requests). The cost of a point is what happened, not the parked count, and the polls that
  could succeed are exactly the ones a poll of every parked task finds; timer waits use
  deadline order (ties by suspension order), other waits use park order;
- a successful poll performs the wait's effect (a join's poll moves the result) and queues the task.

Host requests park too (`host_waits.rs`). Every capability request goes through
`host_waits::issue`. `io_checks` first checks arguments and authority through
`Adapter::preflight`, then completes cancellation without the requested effect. A consumed
stream still closes through the adapter, including scripted Expect checks and reports.
The same checks serve RealIo, SimIo and ScriptIo (the H model). After valid arguments,
a canceled task's blocking request (including file, process and timer calls) completes
`Canceled` without reaching the provider (CONC-3), as the interpreter's HostLibrary does.
A stdin or network request that the adapter answers `AdapterError::Blocked` parks its task (`Wait::Host`): a network request after
its yield, a stdin read at once, as the interpreter parks them, and the park tells the
adapter (`Adapter::parked`). While a host request is parked or a scope deadline armed, each
scheduling point asks the adapter once for the parked tokens that are ready
(`Adapter::ready`). A poll re-issues the request when the task is canceled before the
request committed an effect, or when its token is ready, so the effect and its tape event
happen at the poll, and the resumed operation does not yield again. With nothing runnable
the scheduler sleeps in the adapter's `wait` on every parked descriptor and the earliest
deadline, then polls again.

Every started task has its own `TaskCtx`, the pointer its compiled frames receive: the root's
is the context `hirsel_rt_v2_ctx_create` returns, a child's lies at the top of its fiber stack.
The context splits three ways:

- **`TaskCtx`, the task's own:** at offset 0 its view of the worker's compiled words (pages,
  idle slabs, dup bound, fuel, heap id) with the heap and heap epoch the pages were copied at; its
  failure chain with spawn paths and isolate scratch; its finalizer shield; its release
  worklist; its innermost region; its freeze/thaw stack; its foreign scope while switched out.
- **`Worker`, one per thread that runs Hirsel code:** its heap (size classes, large blocks,
  slabs, region pool and measurement) and idle fiber stacks. Runtime code reaches it only
  through `TaskCtx::worker` and `TaskCtx::heap`, which borrow the context; the suspending
  scheduler entries take `&mut TaskCtx`, so no worker borrow lives across a suspension.
- **`RunShared`:** the scheduler, channels and synchronizers, the Io binding, the launcher's
  driver, the failure table, the count bound, the hash seed, the scheduler policy and the heap
  registry. The binding's Path identities and launch arguments (`paths::Names`) are shared
  with it when the binding is bound, so interning a Path, ordering two and reading an argument
  never reach the binding: the intern table is a concurrent service behind one mutex, after
  lasso's `ThreadedRodeo`.

Heaps (design §6 Heaps, `heaps.rs`, `remote.rs`) live in the run's registry until the run
ends, each with a nonzero id that its slabs, large blocks and region segments carry. A free on
another worker's heap publishes the storage to its owner instead of touching the owner's
lists, as mimalloc's thread-free lists do: a small block on its slab's `xthread` list (whose
low bit marks the slab queued on its heap), a large block or region segment on the owner
heap's return list. The owner collects at its allocation slow path, trim points and every
switch-in; a slab goes back only when it holds no live block and no published one. A retiring
worker (`hirsel_rt_v2_worker_retire`) orphans its heap: any live worker's scheduling point
adopts it for one collection, and a new worker (`hirsel_rt_v2_worker_create`) re-adopts an
orphan before it creates a heap. Diagnostic registries sit per heap behind a lock, and a state
lookup searches the run's heaps. `proofs/heaps` model-checks the lists with loom; the
measurement archive counts `remote_frees`, `remote_collected` and `heap_adoptions`.

A switch moves the worker from one task to the next: switch-out folds the view's idle-slab and
fuel counters back into the worker and saves the thread's foreign scope; switch-in copies the
worker's 512 bytes of pages into the view only when its stamp is stale (another worker, or a
page change since), hands it the counters, and sets the thread's running context and foreign
scope. A runtime slow path that changes a current page updates the worker's words and the
running view under a new epoch. Compiled code reads no thread-local storage: every emitted
object is refused if it has a TLS section, symbol or relocation. One worker runs every task
of a run today; a value crossing tasks moves by copying its root, and the checker's sendability
rules keep each counted value owned by one task.

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

`channels.rs` is the run's unbounded FIFO channels (CONC-6), behind `sender.dup` and the
compiled bodies of `channel.open`, `channel.send` and `channel.receive`:

- A message crosses inline, as the `Some` image of an `Option[T]` slot. The compiled open
  passes `Option[T]`'s registry descriptor (LIR `ConstantValue::Descriptor`) and its `None`
  image; the channel keeps both. A send copies the image into the channel's ring of images (one
  allocation of `capacity * stride` bytes, the capacity a power of two that doubles when full)
  and writes `None` back into the sender's slot; a receive copies the oldest image into the
  receiver's `None` slot. Neither allocates once the ring has grown.
- A send never checks cancellation. It moves the payload into the queue, or returns `Closed`
  with the payload when no receiver is left, and still yields.
- A receive returns `Canceled`, else the oldest message, else `Closed` once no sender is left,
  else parks on the channel's list of waiting receivers; its poll performs the receive. A
  send, or the close of the last sender, touches the channel, and the next scheduling point
  wakes as many of its waiting receivers, oldest first, as it has messages (every one once
  no sender is left).
- Sender and Receiver are run-local handle words with bit 63 set. Their drop's handle close
  reaches the scheduler instead of the Io adapter. The last receiver's close detaches the ring
  and drops the messages nobody received in place through the descriptor's drop program
  (`hirsel_rt_v2_finalize`'s engine), oldest first; an element whose program runs nothing
  (a `u64`) is not walked.
- A scheduling point keeps the storage of its woken and touched lists across points, so waking
  receivers allocates nothing.
- Runtime ABI 28: `hirsel_rt_v2_channel_open` takes the `None` image and the `Option[T]`
  descriptor index, and `hirsel_rt_v2_channel_send`/`_receive` take `Option[T]` image slots
  instead of `Option[List[T]]` carriers.

`sync.rs` is the run's mutex and atomic cells (CONC-4, CONC-5, FAIL-4), behind the compiled
bodies of `mutex.create`, `mutex.with` and `atomic.*`. A mutex's cell holds its payload as the
block of a one-element list, moved out of and into `Option[List[T]]` carriers, so a Map, String or List payload needs no layout knowledge here; the run's end
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

Task sets (CONC-2) are scheduler rows holding their members' Task words in add order; a set word
is a run-local endpoint word with `tasks::TASK_SET` too, so its drop's handle close reaches the
scheduler, which drops only the row. A child takes its place in completion order when it holds
its result or fails, and `task.await_any` returns the unawaited member that came first, parking
(`Wait::Any`) until one has. `task.await_at`'s compiled body checks `$intrinsic.TaskSetHas` (CONC-2
when false), takes the member's word with `$intrinsic.TaskSetTake` and joins it as `task.await`
does. `task.cancel_all` cancels and detaches the members, then parks (`Wait::Drain`) until each
completed; `task.detach_all` only detaches them.

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
line, column and spawn path, each a fresh block. A record holds compiler codes, so a program's
descriptor carries its failure table (`program.rs`, `isolate::FailureTable`), which each of its
runs' contexts reaches: one pointer-free byte string holding each site code's definition path and one-based position
(0 and 0 for a site inside a library body, which only a resource failure reaches), sorted by
code, and each program rule code's rule ID. Since runtime ABI 22 it is fixed-width site and
rule records over a string pool that holds each distinct path once, read in place: loading a
program decodes and allocates nothing, and a lookup binary-searches the site records and
validates only the strings it returns.

### Free mode at width 1 (lane F0)

`free.rs` runs `tasks.free@1`'s protocol (`hirsel-free`) on this cooperative scheduler at one
permit, through F0's test channel `HIRSEL_RT_FREE_TEST` (`hirsel_free::session::TestHook`),
which the launch installs after the schedule and which replaces it with a seeded random one.
Every synchronising entry is then an op that yields: channel open, dup, send, receive and an
endpoint's close, mutex create, take, put and kill, the atomics, `thaw_turn`, spawn, join,
`await_any`, scope entry and exit, the other task and set entries, `task_canceled`, the
standard-stream prints (host writes on the `stdout`/`stderr` keys) and each task's end (a
child's when it comes to hold its result, else at its completion; the root's after its entry
returns). An op that cannot wait publishes (k, done); a waiting op
publishes (k, entered) when it parks and (k, done) once it acts. A receive and a mutex take
wait `Wait::Retry`, woken at every publish (a receive also at its cancellation), and act in
their own call; a join, a scope exit and a drain keep their native waits. A take waits because
every op yields, so another task's body can hold the mutex across its ops. A phase the cut does
not admit, or a task at its cut, parks `Wait::Final`. Releases stay immediate (deferral is lane
R3's): at width 1 an immediate release is by the sole holder or unseen until the releaser's
next op. `shared.thaw`'s compiled body asks `hirsel_rt_v2_thaw_turn(root) -> bool` (runtime ABI
24) whether its root is the last: the count test alone outside free mode, and in free mode an
op whose answer is recorded live and replayed (a replayed `Ok` waits until the root is the
last). A replay that leaves its tape writes `diverged` to the report and stops Diverged.

## Launch and the Hirsel-ABI Io entries

`hirsel_rt_v2_launch(entry, result_size, layouts, layout_count, config)` (`src/launch.rs`) binds
the linked program's immutable descriptor registry and runs one compiled
`fn(rt: Io, args: Args) -> Result[(), E]` in a fresh context with real Io
granted on `HIRSEL_RT_ROOT` (without it, the root Io grants no mount and no projection), and
returns the exit status of the run's outcome
(`hirsel_run_schema::RunOutcome::exit_code`), using the
[PROG-3 policy](../../docs/spec/programs.md#prog-3-entry) shared with interpreted
launches. Blocked reports retain parked tasks' spawn paths, source sites, wait
kinds and target ordinals, before abandoning their contexts. Observation version
4 carries those records in a `waits` JSON line beside `blocked` and
`audit unavailable`. Runtime ABI 22 carries existing caller-site arguments into
the blocking task/channel primitives; only the parked path stores the site.
The caller (generated `main`, or a host running several programs) decides what
to do with the outcome; no outcome exits the process.
`src/budget.rs` enforces both budgets (`budgets.limits@2`). The memory meter (`budget::Meter`,
shared by the run's heaps and region pools) charges logical storage only: a small block its size
class, a large one its request rounded up to 4 KiB, and a region segment the size an empty pool
would give it, so a stop never depends on what the pools held. A block is credited when it is
freed, on the freeing task, and a segment when its region exits. Under a memory budget compiled
code allocates and frees nothing inline (`Heap::set_metered`), and a region pool reuses only a
segment of exactly the canonical size. Fuel is the running task's chunk, the context word at
`HIRSEL_RT_V2_CTX_FUEL_OFFSET`. Code compiled with fuel checks decrements it at each fuel point
and calls `hirsel_rt_v2_fuel_refill` when it goes negative, which stores the next chunk from the
run's `FuelPool` and returns (runtime ABI 27): the whole budget at width 1, so exhaustion is
exact, and 64 Ki points without a budget. A spent budget raises
`Stop::BudgetExceeded` on the running context. The descriptor flag `FUELED` marks such code;
an executable without it refuses a fuel budget when binding its run (`ProgramFacts::fueled`;
`budgets.fuel`), and a memory budget needs no compiled checks. With `HIRSEL_RT_OBSERVATION` it
writes the observation record (`hirsel-observation 5`, the status, a default-action `signaled <kind>`, the result
bytes or the failure chain as kind, rule and site codes, and the task-exit
audit in a diagnostic or measurement archive, or `audit unavailable` in production;
version 5 adds `ended` for a failure that ended the run unaudited, FAIL-3). For harness launches without a run record, `HIRSEL_RT_TAPE` receives the real Io completion tape
(`hirsel-io-tape 7`, which also reads version 6: run identity digest, independent hash/Random/Test seeds, for production recordings the canonical run record and its admission time, launch arguments, initial tree, typed completions and a complete-count trailer); `HIRSEL_RT_REPLAY` runs on simulated
Io that accepts exactly a recorded tape and refuses a mismatch, a missing event
and an unconsumed one. Each completion is appended and flushed before delivery; an abort preserves its confirmed prefix. A missing trailer marks an unknown suffix and refuses complete replay or any resumability claim. Native stdout uses raw OS writes, so EPIPE and EBADF become recorded OutputError outcomes; generated main ignores SIGPIPE before entry. Replay divergence exits 3 with confirmed stdout, including during handle cleanup.

A `RuntimePrimitive` manifest row names a symbol exported at the Hirsel ABI v2
(`(ctx, [caller site, operation site], [caller destination], [result pointer], parameters...) -> status`).
`operations::HIRSEL_ABI_EXPORTS` lists each such export with its physical
signature, and the driver checks every call against it before linking. These include stdout/print, all directory and file leaves, Path/Args, MonotonicClock/WallClock/Random,
Test check/check_case/fail/seed/log/expect_snapshot/dir, IEEE bit primitives, Unicode
cursors and text entries. `test.log` and `test.expect_snapshot` append one escaped report
line (`hirsel_kernels::report`) to the Test report on stdout. `test.property`, `test.equal`
and `test.approx` are ordinary std@1 source;
expect_error awaits the approved general divergence contract. `Path` is an
inert u64 into the context's intern table; file and atomic handles close on drop.
Taken bytes and handles are consumed on every outcome, including status 1.
`file.walk` and `file.read_dir` own counted environments of bounded sorted directory pages, with a
runtime descriptor and step thunk; it stops after its first Error.
The ABI test pins that thunk's C signature and result layout to the derived
`IterNext` ABI on x86-64 and AArch64, including both source-site words.
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

`iter.from` is a Body row whose inline construction owns a compiled step thunk in
`hirsel-rc` (std's adapters, `map` among them, are source over it); `iter.next` is an Instruction row lowering to one
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
bytes.append, bytes.slice, string.position,
string.byte_offset at their declared Hirsel ABI. Fresh results use
`hirsel_rt_v2_bytes_descriptor` (Sequence[u8]: u64 count, then bytes). UTF-8
conversion transfers the same slice into either Result arm; slicing shares one
owner credit and preserves its origin. bytes.replace is ordinary std source.
StrPos's nominal u64 storage travels by read-only address. SEM-3 exports return
status 1 with output unwritten; emission records the manifest's rule at the call
site. `hirsel-pipeline-tests/tests/front/rt_text_native.rs` checks the full native matrix, C callers under
ASan/UBSan on x86-64 and qemu on AArch64, and generated drop glue with zero
live blocks after every one of 1000 calls at O0/O2 in both runtime modes.

`bytes.append` copies a borrowed byte interval with one geometric growth and a
bulk copy. Native import charges and prepares tracked arena destinations through
`ArenaPush` before the runtime leaf; an empty append skips preparation and the
leaf entirely. H models the append independently. Ordinary
heap destinations with spare capacity avoid the growth call. Native output borrows
already-valid String bytes through `Adapter::print`; RealIo writes them directly
when neither retained history nor recording is enabled. The owned request path
keeps recording, replay, fault injection and simulation behavior unchanged.

Owning byte bridges retain one Slice owner credit and its exact interval.
Stable local `string.as_bytes` views reuse the String storage without a holder. Freezing
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
Narrowed directories retain their parent's read/read_write rights; `dir.read_only`
(`ReadOnlyDir`, tape `read_only_dir`) reopens its parent's pinned directory through that
descriptor with read rights. Write, remove, atomic creation, append and commit refuse
read grants before storage calls.
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

An extern call (PROG-4) first calls
`hirsel_rt_v2_foreign_enter(ctx, site, previous, dispatch)`, which dispatches the call to its
effective thread (S-13, `foreign.rs`), records its context and source site on that thread,
and keeps its interval (the enclosing scope, its context, how it was dispatched and its
start when metrics are on) in 40 bytes of caller-owned stack storage at alignment 8;
`hirsel_rt_v2_foreign_leave(previous)` undoes it after C returns. The dispatch word carries
the callee package's trusted `threads` (`home`, the default, `serial` or `any`), `@blocking`
and the package's ordinal. In precedence order: a call inside an active interval runs on
that interval's thread; a `home` package's call runs on the home thread, blocking or not; an
`@blocking` call moves its task to a blocking-pool thread (`pool.rs`), which owns it until
the call returns, then hands it back; any other call runs on the caller's worker. A
`serial` package's gate is a logical object of the run: an outermost call takes it before C
is entered (waiting, a CONC-8 wait named `foreign.gate`, while another call chain holds it),
nested calls from its owner re-enter by depth, and the outermost leave frees it and polls
its waiters. Until lane W1, a run's code runs on one thread at a time: a thread hands the
run to another by posting the task to resume and waiting on its own idle fiber, so the
schedule stays the cooperative one wherever a call runs, and the run's fuel moves with the
run. A pool thread has its own worker and heap; blocks a callback allocates there are freed
remotely. A C callback thunk reads the current context back with
`hirsel_rt_v2_callback_context`, which ends the process when the thread has none and takes a
compute permit unless its thread holds one, and gives the permit back with
`hirsel_rt_v2_callback_leave` before returning to C; a blocking call holds none while it is
in C. A callback that fails calls `hirsel_rt_v2_callback_failed`, which ends the process: the C
frames between the callback and the extern call cannot run cleanup. `foreign::enable_metrics`
turns on the extern row's headroom counters (`extern_ns`, `pool_queue_ns`, `gate_wait_ns`);
without it a call pays one branch. Non-null pointer
results, callback arguments and writable pointer words are guarded by both backends;
zero calls `hirsel_rt_v2_foreign_violation(site)`, which reports PROG-4 and the source
location before terminating. A callback passes site zero to select the active extern.
Nullable pointer Options use zero as None without a guard. A program whose
descriptor sets `program::UNREPLAYABLE` refuses a recording or a replay. One that sets
`program::ESCAPES` (CAP-6) binds its run with `CodeReach::Escapes` and an empty path, so a record whose `admission.code` is
`admit.safe@1` refuses with `unsafe_code`, live or replayed; `hirsel check --safe`
prints the path.

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
  Rust callers pass an admitted file to `TaskCtx::set_measurement_output(file)`.

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
  and idle segment bytes, including segment headers/unused capacity;
- `remote_frees`, `remote_collected`, `heap_adoptions`: frees of another worker's block, large
  block or segment, what this heap's collector took back, and orphaned heaps a new worker took
  over (design §5 headroom row "Root traffic; remote frees");
- `meter_ops`, `metered_allocations`, `fuel_refills`: logical charges and credits and heap
  allocations under a memory budget, and fuel chunks handed out (headroom row "Meter, permits";
  zero without a budget or a fuel build); and
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
secured stream and as end of stream otherwise. A scripted client's `tls` answers
`tls.accept` through the shared `hirsel_run_schema::tls::scripted_accept`: a
`Certificate(names)` or `Presents(names, chain)` client sends its first DNS name as SNI,
accepts the selected identity when its leaf names one of `names` and otherwise sends
bad_certificate; a `Presents` chain is verified against a `Request` or `Require` trust set
at its latest notBefore and becomes the stream's `tls.peer` chain. A scripted peer answers
`tls.connect` through `tls::scripted_connect`: a `Serves(server)` peer presents the identity
its SNI selects, which must name the server name, verifies a `tls.connect_as` identity
against its client policy, refusing with the alert a rustls server sends, and its chain
becomes the stream's `tls.peer` chain; `test_net_abi.rs` reads its `TlsServer` with the
`tls.accept` decoder.

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

Standalone launch (`hirsel_rt_v2_main` with a `STANDALONE` program) binds semantic code
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
`--hirsel-record <tape>` before either launch input journals the admitted run at its
record's recording level (`host_launch::journal`): the header binds the canonical record,
its admission time and the mounts' initial tree before entry, and each completion is
flushed (`record`) or synced after its prepared request (`durable`) before delivery.
The tape must lie outside every granted mount and inherited output file.
`--hirsel-replay <tape>` alone replays it (`host_launch::replay`): it parses the tape,
checks its run record against the header and recorded admission (`Recording::bound_run`),
checks program, entry, launch kind and roots against the executable, then runs SimIo
with the record's projections, mounts, Env snapshot, clocks and Net rules. Replay needs
no grants and acquires no host resource; a recording of other code refuses before entry.
Source-run launches (`hirsel run --native`) accept the same two spellings beside
`--hirsel-run`, and the interpreter's host launch uses the same helpers, so a tape
recorded by either engine replays in the other when it has no budget stop.
Ordered CPU receipt v2 separates the portable retirement index, job ordinal,
initialized result bytes and stop kind from optional native descriptor, carrier,
Stretch and frontier fields. Every replay checks the portable fields. Native
replay additionally compares every native field when replaying a native receipt;
the interpreter validates its framing and checksum. A mapper rejected by budget
settlement publishes no result. Its native run outcome records the budget stop.
The interpreter records its own value-free CPU stop receipt after its completed
prefix. Fuel counts and cuts are specific to the engine and compilation, so a
native CPU tape stopped on fuel is refused by the interpreter.

Stdin and stderr are independent Io projections. RealIo pins explicitly supplied
Read/Write providers, retries interrupted reads, captures accepted stderr prefixes
and records every typed stdin completion. SimIo replays those completions without
a reader and rechecks their handle and maximum. Env reads use the exact admitted
name-to-optional-value snapshot: missing keys are denied, null keys are unset. The root
Env is `ENV_HANDLE`, with no row; `env.narrow` (`EnvNarrow`, tape `env_narrow`) mints a
`Grant::Env` row of names its parent grants, and reads through it see the same snapshot.
A launch without a run record reads the root snapshot from `HIRSEL_RT_ENV`.
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
with nonblocking sockets, retained connect/write/TLS progress and readiness tokens. Every
TCP stream it or HostNet connects or accepts has `TCP_NODELAY` set, so a small write (an
HTTP response, the request after a keep-alive response) is sent at once instead of waiting
for the peer's delayed ACK of an earlier segment.
`readiness::Readiness` holds every parked host operation's descriptor interest and deadline
under its token: the interests a provider recorded are registered in one persistent epoll
set, deadlines in a timer heap. `ready` reports the parked tokens whose descriptor epoll
finds ready, whose deadline passed, or which wait on nothing epoll can watch (a regular
file), with no syscall per parked token; the idle-only `wait` is one `epoll_wait` until the
earliest deadline.

Cooperative readiness follows mio's epoll selector (`src/sys/unix/selector/epoll.rs`):
registrations are edge-triggered and outlive the operation that made them, so an idle
keep-alive connection stays in the set without waking it. Two deviations: a registration is
keyed by descriptor (the union of its tokens' events, fanned out per token) because a read
and a write of one socket park under two tokens; and every park re-arms it with one
`EPOLL_CTL_MOD` (ADD on ENOENT) instead of registering once, because providers close their
descriptors without telling Readiness and a reused number must be registered anew. MOD on an
edge-triggered registration also re-reports readiness that arrived while no token watched.
Measured on the module itself (one request/response connection whose read blocks each
round, N idle parked connections, instructions per round user+kernel, median of 10
interleaved seeded rounds of 50,000, CPU 31):

| registration | 0 idle | 100 idle | 1000 idle | cycles (1000 idle) |
|---|---|---|---|---|
| level-triggered, removed when the last token finishes (MOD, ADD, DEL per wait) | 25,420 | 25,738 | 25,788 | 58,207 |
| edge-triggered, kept, re-armed per park (this) | 20,009 | 20,299 | 20,106 | 43,895 |
| edge-triggered, registered once (mio; needs every provider to deregister before close) | 18,925 | 18,843 | 19,079 | 41,297 |

Register-once would save a further 1,100 instructions per wait (5%), at the price of a
hang whenever a provider closes a descriptor without telling Readiness.
`host_stdin::CooperativeStdin` pins a close-on-exec duplicate of descriptor 0 at admission:
a pipe, socket or character device becomes nonblocking (restored on drop) and an empty
read records POLLIN interest; a regular file or block device is read directly and always
ready; a closed descriptor 0 reads as end of input.
DNS runs outside the evaluator and signals a descriptor; relative timeouts retain one
deadline across readiness retries. Partially published writes/TLS exchanges cannot be
reported as effect-free cancellation. HostNet remains the native blocking provider.
`HostGrants::cooperative` (the interpreter, always) and `HostGrants::new` (native, when the
record grants `io.tasks`) install CooperativeStdin over one Readiness, which
`RealIo::set_cooperative_net` also receives; without tasks a native run keeps the blocking
providers. RealIo starts with no host network provider: the entries that open host network
state (connect, resolve, listen, Unix connect and listen, UDP bind) hand
`net_host::provider` to the adapter (`Adapter::net`) before their request, and RealIo builds
CooperativeNet on that Readiness, or otherwise HostNet (as under the harness virtual
clock, whose Readiness parks only timers), on the first. The entries that
start a process (`proc.run`, `proc.pipeline`, `proc.spawn`, `proc.spawn_pipeline`) likewise
hand `process::host` over (`Adapter::process`); a later request on a handle they made finds
the provider in place. The interpreter, which reaches every operation, hands both over at
admission. So, with `--gc-sections`, an executable whose program opens no socket links no
sockets or resolver, and one that starts no process links no process spawning, and the
dynamic loader binds none of their libc symbols at start
(`link_kit::an_executable_links_each_host_provider_only_when_its_program_reaches_it`). RealIo answers the `Adapter` wait methods from that Readiness and records a
`blocked` completion for every request that parks (with whether it already committed part
of its effect) and, at a scheduling point, a `ready <wait> <points>` event for each ready
token, in creation order before any is re-issued: the wait's creation ordinal and the
scheduling points it waited through since its task parked or it was last ready
(`readiness::WaitLog`).
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

Runtime ABI 19 passes distinct attribution and operation codes through release,
reset, inline finalization, region exit and both cleanup thunk types. Existing
anchored failures remain intact while later field failures retain their own pair.
TaskSpawn alone takes the hidden pair among runtime primitives; a child starts
its callable with the actual spawn invocation as both codes.

TLS argument validation uses `hirsel-run-schema::tls` in every world. Adapters bind
PEM validity and its digest to the stream until its handshake completes or closes;
replay rederives it and keeps the existing tape identity. The host constructs its trust store with the same parser.

Filesystem breadth adds the low nine permission bits to Metadata (80-byte,
16-aligned Result ABI), `file.set_permissions` and `dir.remove_all`. Both mutations
check read_write before storage activation. Permission changes pin and classify
the final inode before chmod; final symlinks are refused. Recursive removal uses
no-follow directory opens and unlinkat with an explicit work stack; final and
nested symlinks are unlinked without accessing their targets. A failed removal
records its sorted depth-first deletion prefix as an entry count in Fault.partial,
which replay applies before returning the recorded error. Test trees use the same
request policy and preserve permission bits across stream writes and renames.
File durability (core-library.md, Durability): `file.sync` fsyncs the file, then every
directory from its open path's parent up to the mount root; `file.put_new` writes a hidden
`.hirsel-put-N` temporary beside the destination, fsyncs it under `Durable()`, links it at
the destination with linkat (never replacing), unlinks it and fsyncs the directory chain,
and compares the existing file's bytes on EEXIST. Test trees keep their durable state in
`FileIdentities` (`hirsel_kernels::disk::Durable`): stream and whole-file writes note the
last write, syncs and `put_new(Durable())` record listings and bytes, and `test.crash`
rebuilds the tree from it and marks every File and AtomicFile of the tree stale.

B15 (`tests/it/tape_property.rs`) round-trips generated fields across every
request and completion variant, including populated network and process scripts.
Exhaustive matches require a specimen for new variants. It catches omitted,
misframed and lossy fields; its ignored `full_gate_` version expands the draws.

B12 (`hirsel-pipeline-tests` `tests/front/rt_io_properties.rs`) compares real fd confinement, replay,
interpreter and an independent recursive reference on generated trees with
parent components, dangling links and cycles. Neither simulated oracle consumes
host faults. It catches disagreement between lexical normalization and physical
link traversal; its ignored `full_gate_` version expands trees and paths.

B13 (`hirsel-pipeline-tests` `tests/front/rt_io_properties.rs`) compares check precedence with the independent
interpreter under all condition subsets for cancellable requests, including
expired scopes, and explicitly classifies every request's cancellation policy.
It catches rights, argument and handle checks reordered across cancellation or
provider faults. The ignored `full_gate_` version broadens argument data.

Recording levels (`journal.rs`, determinism.md, Recording levels): `RealIo` performs effects
and passes every request and tape event through its `Journaled` gate, whose `Journal` is the
level the run record's `recording` member selects in `host_launch::journal`: `NoJournal`,
`TapeJournal` (the v7 `RecordingWriter`, byte for byte) or `DurableJournal` over a
`JournalFile` (the host's file with `fdatasync`, or `SimJournalFile` on the simulated disk's
crash model). The durable gate prepares each request before issue and puts a barrier after
each completion; a request that may park under cooperative scheduling is held (an
uncommitted `blocked` park) until `wait` or a batch bound closes the batch, and `finish`
withdraws a held request that was cancelled. `recover` reads a journal back for replay and
names the requests in doubt. The issuer of each request comes from `Adapter::issuer`, which
`host_waits::issue` and the interpreter's host library call only for a durable journal.
The trailer preserves the shared run outcome, including parked waits, complete failure
chains and interpreter diagnostics.

File mappings (`mapping.rs`): `file.map` maps a regular file read-only behind one
anonymous page that holds a static `Bytes` block (extent, immortal header, count and
capacity words), so `hirsel_rt_v2_mapped_view` lends the mapped bytes as a slice of that
block with no copy; dups and releases of it never write. One process-wide SIGBUS handler,
installed with the first mapping, answers a fault inside a live mapping range (a fixed
table of 4096) with a zero page and marks the range; any other SIGBUS goes to the earlier
disposition. `hirsel_rt_v2_mapped_check` reports truncation, sticky in the `Mapped` grant,
when the range was marked or `fstat` finds the file shorter than the mapping. Test trees
and replay serve views from the mapped identity's contents, zero past their end; a host
view copies its bytes only for a tape that keeps them (`State::capture_views`), and replay
lends the recorded view bytes as a metadata input is lent.

File watching (`watch.rs`): every `Watcher` has a `WatchQueue` that coalesces an event
equal to the last one queued and ends past 1024 pending events with one `Overflow`,
dropping later events until it is read. `State` keeps the queues of `test.dir` trees'
watchers, which `test.dir_event` fills by each watch's coverage. A host Dir's watcher is a
`HostWatch`: one inotify instance with a watch on the pinned path (through
`/proc/self/fd`) and, when recursive, on every directory below it, translated into events
with tree paths. `watcher.next` on an empty host queue records POLLIN interest under the
entered token and answers `Blocked`, so `host_waits` parks the task as it parks a stdin
read; without cooperative admission it blocks in `poll`. Replay returns each recorded
event as the host watcher's input.

### Cooperative parallelism profile

In a `measurement` runtime archive, also set `HIRSEL_RT_MC_PROFILE=1` and
`HIRSEL_RT_MEASUREMENT=/absolute/path/to/measurement`. The existing host-admitted
measurement descriptor receives an additional section at exit:

```text
mc-profile 1
mc {"task":[],"ns":1200,"operation":"task_spawn","after":0}
mc {"task":[0],"ns":9000,"operation":"task_finish","after":1}
```

Rows are in observed commit order. Task paths are spawn paths, including `[]`
for the root; `ns` is monotonic elapsed time in the preceding segment; `after`
is the one-based commit that made that segment eligible (zero means entry).
Admission, unblock, and direct cleanup handoffs record dependencies, so the
simulator cannot start children before spawn or continuations before wakeup.
Runtime entry stops the timer and return restarts it; suspended stacks and
host waits are excluded. Mutex acquisition and other non-yielding task-table
operations are boundaries too. Completion and implicit endpoint close are
internal commit kinds. Nested runtime helpers share the outer operation.
No tape event, scheduling decision, provider identity, or ABI changes. The
module, fields, and all call sites are cfg-elided without `measurement`.
The environment switch is off by default, including in measurement archives.

```sh
python3 tools/mc-profile.py /absolute/path/to/measurement
python3 tools/mc-profile.py --json --commit-ns 200 /absolute/path/to/measurement
python3 tools/tests/mc_profile.py
```

The offline model has N segment workers and one separate serial commit server,
including at N=1. Eligible segments start in recorded commit order; workers
become free on segment completion, even if that segment cannot commit yet.
Each task's continuation waits for its previous commit and any recorded
readiness dependency. Commit costs default to a stated fixed 200 ns, rather
than charging cooperative scheduler or fiber switch costs to parallel work.
The report gives makespan, speedup over the same model at N=1, all commit-server
wait (`stall_share`), wait with a later result already ready (`hol_ready_share`),
and commit busy share. Busy and stall partition the model's makespan.

Work includes segment work and commit costs. Span is the longest path through
segment dependencies and the serial commit chain (equivalently infinite
workers); work/span is the Cilk average-parallelism bound. Per-N speedup bounds
also use segment work/N, total work/(N+1), and span. A dedicated commit server
is an optimistic extra execution resource, not a measured N-core runtime.
Host input latency, cache transfers, deferred reference counts, and worker
handoffs are unmodelled. Measurement allocator accounting and timer overhead
also affect short segments. Use ten interleaved rounds on the same session,
retain perf cycles/instructions and SMT sibling activity, and treat the result
as conditional compute potential rather than measured multicore throughput.

The [2026-10-20 dataset](/workspace/code/hirsel-briefs/mc-profile/repo-evidence/reports/mc-profile-2026-10-20/results.json)
retains ten profiler-on/off pairs for each control and two app loads. Every
sample includes cycles, instructions, sibling activity, the per-N makespan and
speedup, and work/span bounds; the adjacent compressed logs retain all segment
durations. [Method and verdict](/workspace/code/hirsel-briefs/mc-profile/repo-evidence/reports/mc-profile-2026-10-20/method.json)
state the assumptions and source/artifact hashes. The snapshot has no conformance
fixture spawning more than four total tasks and no spawning workload benchmark;
the measured fixtures and resource-drop correctness control fill out the table
without claiming they are task-heavy benchmarks.

The original campaign scripts are archived beside the dataset under
`/workspace/code/hirsel-briefs/mc-profile/repo-evidence/`. They describe the
original source snapshot; restore their repository-relative layout with that
snapshot when reproducing the historical campaign.

The app uses its sealed O3 application object and measurement archive with a
small C host that selects the existing standalone admission flag. This supplies
its original grants and process stdin; it changes neither application code nor
the runtime ABI. The light load is 60 requests at 50/s, the backlog load 120 at
2,000/s; each keeps two event streams open and makes every third request a
create. Both use an open-loop client on CPU 7 and the server on CPU 31. The two
sessions' absolute times are never compared. All speedups normalize a trace
against that same trace's one-worker model. The observed sibling load was high,
so these remain conditional projections, not measured multicore throughput.

### Runner launcher

`runner.rs` is a runner's launcher (link design §2): `hirsel_runner_main`, which a
runner's link aliases as its `main`, runs a program image's `main` either in this
process (`exec <tooling> <image> <argv...>`) or in a forked child per request of a
single-threaded fork server on its standard input (`serve`). The server runs no
program code, admits no run and maps no image; each child joins its own process
group, takes the request's streams, closes the server's descriptors, restores
the server's startup signal mask, enters the request's directory and
environment, checks without loading it that the image is whole and that its
`Hirsel` note names this runner's GNU build ID, then `dlopen`s it and calls its
`main` (the generated launcher, which enters `hirsel_rt_v2_main`) and exits
through `exit`. A failure before that is reported to the request's tooling path
with exit status 125, never as a program outcome.

`out.write` and `err.write` accept borrowed Bytes; `out.print` and `err.print`
call the same byte-write entry points with their String slice. ABI revision 32
adds the two exports without changing existing signatures. Like Rust std's
`StdoutLock` and `StderrLock` `Write` implementations
(`library/std/src/io/stdio.rs:845` and `:1074`, rust-lang/rust
`19833af7224424e8b53e288097d56b6af43d5bcf`), output forwards the exact byte
slice. Hirsel additionally checks capability ownership and cancellation and
journals the accepted bytes through its existing Print/PrintStderr events.

### Startup and physical capacity

An unobserved production run discovers host CPU capacity at its first scoped
claim or ordinary child spawn. Its first Tasks projection installs the Ordered
objects while preserving the root's held memory and remaining fuel. Scalar
execution needs neither discovery nor Ordered objects. Replay uses its recorded
logical contract without discovering the current host. Explicit placement
sidecars and event traces retain their startup admission snapshots.

Production workers grow with runnable work when every started worker is busy,
up to the resolved width. An Ordered root starts on the caller. Explicit
physical-worker test and embedding requests retain their requested topology.
A scoped claim reserves its full width and storage, runs destination zero on
the caller, and starts other destinations only when they have prepared jobs.
The caller returns its ordinary credit before reserving the batch and takes
it back after the joined drain. All started workers join before normal teardown.

### Spawn hints and the worker stack pool

`TaskSpawn` carries a trailing `u64 flags` (runtime ABI 36). Bit 0 is a compiler
proof that the next operation on the normal path awaits this exact child; bit 1
is an op-free body proof, including transitive calls, captures and destruction,
with a trivially droppable result. Empty authority is not evidence. The native
compiler computes these private facts without changing the public function's
effects. Unknown calls and shapes fall back to ordinary scheduling.

Only the native Free driver uses the hints. For an immediate join it reserves
the child's row and spawn path, then lends the parent's physical fiber to the
child's independently owned `TaskCtx`. The running dispatch image names the
child until its return, so an operation can suspend and migrate it. The parent
is logically parked on the join; completion restores its image and context on
the child's final worker before retiring the child. An intervening operation,
including failure cleanup, materializes the deferred child with the original
identity before its own effect. Spawn still counts against the 128-op budget.
An op-free child can return a raw, trivially droppable result image without
retaining its frame. Ordered execution and replay ignore the hints. Recordings
retain separate task logs; native replay admission is still the Free driver's
responsibility.

All workers return idle stacks through `fiber::StackPool`. It preserves the
upper 64 KiB (including `TaskCtx`) and advises the lower usable pages with
`MADV_FREE`, falling back to `MADV_DONTNEED` when unsupported. The mapping and
inaccessible guard stay intact. This follows Go `src/runtime/mem_linux.go:40-68`
at `ebe2feefc400f813aefdc8a036e66d2619da6649`. Unlike Go's allocator, advice
failure never remaps the stack: retaining reclaimable idle memory is safe. The
fixed hot prefix fits Hirsel's fixed guarded mappings rather than Go's growing
stacks. Exclusive notification ownership follows Tokio's task harness
`runtime/task/harness.rs:160-229` at `ce685ee6916549ed7e52dbfd5430858bbdc734d3`;
Hirsel additionally saves a native stack and lends its physical image across a
nested child. Tests cover reuse, saved/migrated children and a cutter/host stop.
