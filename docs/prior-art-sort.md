# Sort prior art

`list.sort_stable` ports Zig's WikiSort at commit
`bff490a23889750de7096d9057c0a432efc929a8` (MIT), cloned for this work.
The source is `lib/std/sort/block.zig`: Range, Iterator and Pull at lines 7–104;
internal-buffer discovery, extraction, block tagging, rolling and redistribution
at 320–755; constrained rotation merge at 758–803; internal swap merge at
806–839; block swap and biased binary searches at 842–983.
The implementation is in `stdlib/std@1/list_block.hir`, in the existing `list`
module. It replaces the recursive rotation merge with the cache-free block
algorithm: O(n log n) time and O(1) auxiliary space.

The vectors in `packages/fixtures/list_sort_vectors@1/vectors.hir` port
`lib/std/sort.zig:178–222` (two nine-element IdAndValue cases), 225–252
(ten stability fuzz trials), 254–330 (six u8 and seven i32 input/output pairs),
and 471–513 (all twelve binary-search assertions). The thirteen literal sort
vectors run against both `sort` and `sort_stable`. The search case also checks
Hirsel's first-equal guarantee. Each fuzz trial has its own fixture,
checks reverse IDs among equal values, and verifies the ID permutation.
Fifteen additional block cases check the exact stable permutation, both sort
directions, power-of-two boundaries and one/few/many distinct keys.

Deviations from Zig:

- The external `[512]T` cache is disabled. Its uninitialized storage, copies and
  overwritten owners cannot represent arbitrary move-only Hirsel elements.
  Zig explicitly supports the zero-cache internal-buffer path (block.zig:590–593).
  Every element remains in the input and moves through ownership-preserving swaps.
- Adjacent stable insertion replaces the tagged sorting networks for initial
  groups of at most seven elements, and the hard-coded two/three-element path.
  This avoids the auxiliary mutable order array under `@noalloc` and uses the
  existing ownership-preserving insertion helper. The run boundaries are Zig's.
- The two Pull descriptors are local records, instead of an array, avoiding
  collection storage; the rational run iterator otherwise follows Zig exactly.
  Discovery, extraction, block merging and redistribution are separate helpers
  so the ownership checker does not retain the entire large merge function
  at once (the initial monolithic port exceeded the 2 GiB std-precheck limit).
- List indices are u64; the existing three-way comparator is tested for `< 0`
  instead of taking a context and a boolean less-than callback. Debug-only
  comparator validation is omitted: Hirsel has no compilation-mode-dependent
  std behavior, and callers must provide a consistent strict ordering.
- The integer square-root helper is Zig `lib/std/math/sqrt.zig:36–61`
  specialized to u64, including its highest power-of-four initialization.
- Fuzzing uses ten fixed seeds (0–9), each drawing a size in [0,1000) and keys
  in [0,100) with std.prng, so corpus execution is reproducible. Upstream uses
  the test runner's random seed. The ten full-sized trials, three large block
  cases and the existing 257-element stability case run as Rust integration
  tests with a 30M-step bound: they exceed the corpus's 1M-step limit. The
  grouped 63/64/65 case is split into one corpus program per size.
- Binary-search callbacks reverse Zig's context/item comparator direction.
  The three interval queries are represented as point intervals, and the string
  length query as a four-byte string, because Hirsel's existing API takes a
  needle of the element type. Input ranges, query values and results are exact.
  u8 character vectors use their numeric byte values. The public API is unchanged.

## Unstable partition storage

The existing pattern-defeating quicksort and its comparator sequence remain in
`stdlib/std@1/list.hir`. Its 64-position block queues use the bitmap mechanism
from LLVM libc++ commit `fc9215428138c4d9238486c2bdbea192b6107b65`,
`libcxx/include/__algorithm/sort.h:354–399` (`__swap_bitmap_pos`,
`__populate_left_bitset`, `__populate_right_bitset`) and `452–480`
(`__swap_bitmap_pos_within`). A set bit represents one misplaced element.
Trailing-zero counts consume paired positions in ascending order; leading-zero
counts consume remaining positions in descending order. This replaces two
initialized `Inline[u64, 64]` arrays with two `u64` values.

Rust std was also reviewed at commit
`6a58677913ae3e443146503de3933abef0b2eda3`,
`library/core/src/slice/sort/unstable/quicksort.rs:90–152` (partition contract
and dispatch), `244–337` (cyclic branchless Lomuto), and `341–360` (simple
Lomuto). Its cyclic moves and different comparison sequence are not ported:
Hirsel must keep every owner in the list and preserve observable callback
failure, including the first failing site.

Deviations from libc++:

- Keep the existing Hirsel scans, block sizes, pivot selection, recursion,
  heapsort fallback and exact comparison/swap order. Only queue storage changes;
  libc++'s whole partitioner and comparator-purity dispatch are not adopted.
- Compare through the public three-way noalloc callback, with ordinary failure
  propagation. Classify all selected left positions before the right positions,
  then swap. No callback is skipped or assumed infallible.
- Use checked source indexing and ownership-preserving `list.swap`; no unchecked
  pointer operations, temporary element copies, backend assumptions or validator
  exceptions are introduced. Each shift is by a block offset below 64, and each
  subtract-one mask update is guarded by a nonzero test.
- Clear the highest remaining bit with XOR rather than constructing a mask below
  that bit. This keeps the highest-bit case within ordinary checked u64 shifts.

Existing Zig literal vectors, six input patterns and stable-sort fixtures remain
independent controls. Added record and failure fixtures pin a 129-element
pre-change equal-key permutation and the first of two distinct callback failure
sites. The permutation pin is a regression control for this storage change,
not a new public stability guarantee.

### Partition pivot loans

Block classification now borrows `xs[begin]` once per nonempty left or right
block, following libc++'s borrowed `__pivot` parameter in the same pinned
`sort.h:372–399`. The loan's scope ends before the first ownership-preserving
swap. All element index arithmetic, comparisons, bitmap updates and swaps keep
their original order. Empty blocks do not read the pivot.

This needs no callback-purity promise. `xs` enters through exclusive unique
storage; its elements are borrowed read-only by the comparator (OWN-1/OWN-3).
No swap or other list mutation occurs within either classification block.
The pivot index is the nonempty range's `begin`, and every scan index lies
strictly above it and below `end <= len`. Hoisting that checked pivot read
therefore cannot expose a bounds failure before a previously observable
callback failure. Ordinary comparator failures still propagate at their
original comparator source sites. Generic elements are borrowed, never copied
or moved into a temporary owner. Stable sort and the public signatures are
unchanged.

LLVM can remove the repeated pivot bounds check and address calculation, and
vectorize a concrete infallible integer comparison where its existing rules
permit it. Fallible or generic callbacks retain their ordinary call sequence;
no compiler, IR, runtime or callback contract changes are involved.

## Allocating stable driftsort

`list.sort_stable_fast` ports Rust core's stable driftsort, revision
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`. References under
`library/core/src/slice/sort/` are `stable/mod.rs:30–140` (entry and scratch
policy), `stable/drift.rs:22–310` (powersort, lazy runs, eager fallback),
`stable/quicksort.rs:19–183` (bounded recursion, ancestor equality and stable
partition), `stable/merge.rs:10–70` (shorter-run buffered merge),
`shared/pivot.rs:17–117` (adaptive pseudomedian), and `shared/mod.rs:20–49`
(strict descending and nondecreasing run detection).

The allocation is acceptable for this explicitly allocating API: Hirsel permits
ordinary bounded heap allocation outside `@noalloc`; exhaustion follows normal
allocation failure and cleanup. `sort_stable` remains the allocation-free
WikiSort, with its original signature and comparator order. Choose it when
allocation is prohibited; choose `sort_stable_fast` for throughput. Sort::Self
now uses the allocating API because upstream Rust calls `Vec::sort`, which
also allocates scratch. Its input, clone, PRNG and checksum are unchanged.

Deviations from Rust:

- Sort original u64 indices, then apply the permutation with ownership-preserving
  swaps. Rust's uninitialized T scratch, pointer copies and panic hole guards
  cannot be expressed for arbitrary move-only Hirsel T. No owner enters scratch.
  For inputs above 20, all callbacks finish before owners move; a callback failure
  drops the scratch and retains the original input owner set and order. Smaller
  inputs use the existing stable adjacent-swap insertion, retaining a permutation
  at every failure point. Ordinary failures propagate immediately; no callback is
  reordered by a purity assumption. The old noalloc API's callback order is intact.
- Allocate one n-entry index list and one scratch list of
  `max(48, ceil(n/2), min(n, 1_000_000))` u64 entries. This applies Rust's 8 MB
  full-scratch policy to indices, with at most two list allocations after the
  small-input path, and initialized entries instead of MaybeUninit. The extra
  index list is required by ownership. Rust's 4 KiB uninitialized stack buffer
  is omitted; the powersort stack is three initialized Inline arrays of 66
  entries. No type-size specialization or zero-sized-type detection is used.
- The internal smallsort threshold is 32, with stable insertion shifts of u64
  indices in place of Rust's type-specialized smallsort (`shared/smallsort.rs`).
  The public <=20 path uses adjacent swaps instead of temporary T moves.
- Stable partition uses checked source loops instead of unsafe pointers,
  pointer arithmetic and four-element unrolling. Conditional writes with separate
  left/right cursors replace Rust PartitionState's destination-base selection:
  checked index arithmetic in that selector raised task instructions from 15.89B
  to 18.94B in the measured alternative. Both forms place exactly the same
  indices; pivot comparison order and the skipped self-comparison follow Rust. The pivot
  is an index into the untouched input, so Freeze/interior-mutable-type pivot
  copies and marker specialization are unnecessary under Hirsel's read-only
  comparator loans. The merge uses checked indices and explicit remainder loops
  instead of raw pointers and a panic guard. Scratch failure needs no owner repair.
- Test vectors adapt `library/alloctests/tests/sort/tests.rs:18–22,450–565`
  (length boundaries, stable occurrence ids and patterns) and `599–638`
  (failure retains the original set). Eight deterministic distributions replace
  random seeds; small sizes are bounded for the interpreter. A separate native
  1,100,000-element vector runs all eight patterns above the full-scratch cap.
  Move-only resources replace Rust Clone/Drop test values, with a finalizer ledger and two distinct
  comparator failure sites. Observable interior mutation of a borrowed T is
  unavailable in Hirsel's comparator contract. Tests compare interpreter, heap
  evaluator, LLVM and Cranelift, including terminal failure and cleanup.
