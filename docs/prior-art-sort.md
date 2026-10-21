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
