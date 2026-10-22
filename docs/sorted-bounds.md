# Sorted list boundaries

`std@1/sorted` borrows a `List[T]` and a value of `T`. Its comparator has the
same signature as `std@1/list.binary_search`:
`by: @noalloc fn(T, T) -> i32 uses {}`. Negative, zero and positive mean before,
equal and after in the caller's ordering; the magnitude is irrelevant.

- `lower_bound(xs, value, by)` returns the first index whose comparison is
  nonnegative.
- `upper_bound(xs, value, by)` returns the first index whose comparison is
  positive.
- `equal_range(xs, value, by)` returns the half-open tuple `(lower, upper)`.
  An absent value gives `(i, i)`, its insertion position.

A missing boundary is `list.len(xs)`. All three return zero boundaries for an
empty list without calling the comparator. Descending lists use a comparator
that orders larger keys before smaller keys. The comparison signs along the
list must form a negative prefix, an equal run and a positive suffix; sorting
by that comparator establishes this precondition. There is no linear validator.
A comparator that changes its ordering between calls violates the precondition.

The searches keep a half-open unknown interval `[lo, hi)`. Each call examines
only its actual midpoint, computed as `lo + (hi - lo) / 2`, and removes at least
half of the remaining interval. `equal_range` finds the lower boundary, then
searches `[lower, len)` for the upper boundary. It never scans duplicates.
For length `n > 0`, each boundary uses at most `floor(log2(n)) + 1` comparisons;
the range uses at most twice that count. Additional storage is constant. The
functions and their callbacks are `@noalloc` and `uses {}`. They borrow elements,
do not clone values, and leave list order and contents unchanged.

Comparators are ordinary fallible Hirsel calls: the first language failure
propagates immediately, with its original source and ordinary resource cleanup.
There are no speculative calls, calls at sentinel indexes, or final equality
probes. Callback order and count are not promised to match `list.binary_search`.
This API does not turn a language failure into `Result` or suppress it.

## Duplicate keys with resources

The executable example is
[`SEM-5-sorted-bounds-7-owned-run.hir`](../conformance/SEM/SEM-5-sorted-bounds-7-owned-run.hir).
Its resource-bearing `Item` records hold a key, identity and stdout handle; their
destructors report each identity. The relevant borrowing calls are:

```hirsel
let lower = sorted.lower_bound(xs, xs[3], compare)
let upper = sorted.upper_bound(xs, xs[3], compare)
let range = sorted.equal_range(xs, xs[3], compare)
// Keys [0, 0, 1, 1, 1, 1, 2, 2] give lower=2, upper=6, range=(2, 6).
```

The target can be an element of the same list. No owned sentinel record or
cloned resource is needed. The fixture verifies that every identity remains
at its original index and is destroyed exactly once after the search.

## Prior art and adaptations

Go checkout `/tmp/ref-go` was read at the exact clean commit
`ebe2feefc400f813aefdc8a036e66d2619da6649`:

- `src/sort/search.go:9-17`: monotonic predicate contract and `n` sentinel.
- `src/sort/search.go:58-73`: half-open partition invariant and binary narrowing.
- `src/sort/search_test.go:15-62`: empty, singleton, duplicate, absent,
  descending and synthetic billion-element/overflow boundary vectors.

Copyright 2010 The Go Authors. All rights reserved. Adapted under the
BSD-3-Clause license retained in [`third-party/NOTICES.md`](../third-party/NOTICES.md).

Adaptations: explicit lower and upper operations use the existing Hirsel
three-way borrowed comparator rather than Go's index predicate. Unsigned
`lo + (hi - lo) / 2` replaces Go's unsigned sum-and-shift to respect Hirsel's
checked arithmetic. A private midpoint helper lets tests execute the production
expression near `u64::MAX`. Synthetic large-domain tests use that helper in a
bounded Rust harness, without allocating billion-element lists. The tuple range
and suffix upper search are additive convenience APIs. Go's panic/nil and
ownership behavior is replaced by Hirsel's ordinary failure and cleanup rules.
No language rules, TIR/LIR constructs, ABI or existing list implementation change.

## Verification inventory

The lane owns five corpus programs and their five `.hir.json` manifests:

- `conformance/SEM/SEM-5-sorted-bounds-7-vectors-run.hir`
- `conformance/SEM/SEM-5-sorted-bounds-7-owned-run.hir`
- `conformance/MEM/MEM-14-sorted-bounds-7-lower.hir`
- `conformance/MEM/MEM-14-sorted-bounds-7-upper.hir`
- `conformance/MEM/MEM-14-sorted-bounds-7-equal.hir`

The three failure cases test the first lower/upper midpoint and the first
upper-phase midpoint of `equal_range`, respectively. They assert the callback's
failure site and a complete, nonduplicated destructor identity ledger. Native
parity runs use the diagnostic runtime's leak audit and compare against H.

`crates/hirsel-driver/tests/it-stdlib/sorted_bounds_7.rs` adds five tests:
550 generated ascending/descending list queries against an independent linear
oracle; 168 traced searches of short duplicate runs and all-equal lists that
count actual comparator entries (including zero calls for empty input); five direct near-maximum midpoint vectors plus five
bounded synthetic domain searches; resource borrowing; and failure cleanup.
The private trace fixture and manifest are
`crates/hirsel-driver/tests/fixtures/sorted_bounds_7/trace.hir{,.json}`.
Generated tests with a five-million-step limit stay in the Rust harness; corpus
limits remain unchanged.

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test root//:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=sorted_bounds_7
.buck2/bin/buck2 test -c hirsel.conformance_paths='conformance/SEM/SEM-5-sorted-bounds-7-|conformance/MEM/MEM-14-sorted-bounds-7-' root//:gate-conformance-selected-shard-1
.buck2/bin/buck2 test -c hirsel.native_filters='conformance/SEM/SEM-5-sorted-bounds-7-vectors-run.hir|conformance/SEM/SEM-5-sorted-bounds-7-owned-run.hir|conformance/MEM/MEM-14-sorted-bounds-7-lower.hir|conformance/MEM/MEM-14-sorted-bounds-7-upper.hir|conformance/MEM/MEM-14-sorted-bounds-7-equal.hir' root//:gate-native-cranelift-changed
```

The final command selects exactly the five corpus IDs and runs each on H, LLVM
and Cranelift. The other additive integrations are one `mod sorted_bounds_7;`
line in the documented `modules.rs` registry, one std package source entry,
five append-only corpus registrations, and a third-party attribution row.

The focused Rust inventory grew from zero lane tests to five passing tests
(459 other harness tests filtered out). All five lane corpus cases pass:
four passed in the first frozen source run, and the resource-success case
passed after its expected destructor stdout was declared. Native selection
used exact case paths, executing exactly these five IDs; every LLVM and
Cranelift diagnostic-runtime execution agreed with H (ten native executions).
The two verification chains recorded SHA-256 hashes for 11,043 source/build
inputs before and after, with no changed hashes. No performance timing claim
or whole-campaign result is made.
