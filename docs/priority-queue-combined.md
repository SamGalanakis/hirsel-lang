# Combined priority queue operations

To retain the largest three values seen so far, fill a min-heap and then use
`push_pop`. The returned owner is the item excluded from the retained set.

```hirsel
@noalloc
fn compare(a: i32, b: i32) -> i32 uses {} {
  if a < b { -1 } else if a > b { 1 } else { 0 }
}

pub fn main(t: Test) -> Result[(), ()] {
  let mut best: unique priority_queue.Heap[i32] = priority_queue.empty(compare)
  let input: Array[i32] = [4, 1, 7, 3, 8, 8, 2]
  for value in input {
    if priority_queue.len(best) < 3 {
      priority_queue.push(mut best, value)
    } else {
      let excluded = priority_queue.push_pop(mut best, value)
    }
  }
  failure.check(priority_queue.pop(mut best) == Some(7), Bounds())
  failure.check(priority_queue.pop(mut best) == Some(8), Bounds())
  failure.check(priority_queue.pop(mut best) == Some(8), Bounds())
  Ok(())
}
```

For a fixed-size queue that must accept every new value, `replace_top` returns
`Some(old_minimum)`. On empty it inserts and returns `None`. `push_pop` on empty
returns the input and leaves the heap empty. For nonempty heaps it returns the
input when its comparison with the minimum is <= 0, including equal priorities.
Otherwise it replaces the minimum and returns the old owner. The comparator is
owned by the heap. Returned resources transfer once; retained resources drop
when the heap is released. A comparator failure uses normal failure cleanup,
with no rollback guarantee. Both operations use at most one downward sift.

## Sources and adaptations

- CPython `938144efad4659f7aa66bf9ef362fd3c602c4bbc`,
  `Lib/heapq.py:148-169`: replace root, sift once, return displaced owner;
  `Lib/test/test_heapq.py:134-185,243-262`: top-k, empty, singleton and equal-input
  behavior. Random vectors use independent sorted multisets with fixed seeds.
- Rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
  `library/alloc/src/collections/binary_heap/mod.rs:308-319,838-884`: root mutation
  followed by downward repair; `library/alloctests/tests/collections/binary_heap.rs:152-161`
  supplies the 10-to-8 root update exposing 9.
- Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
  `src/container/heap/heap.go:79-117`: Fix/down with checked child choice and swaps;
  `src/container/heap/heap_test.go:188-202`: replace minimum 10 by 210.
- The existing safe downward helper remains based on Zig `bff490a2`,
  `lib/std/priority_queue.zig:59-78,138-156`; its min ordering reverses comparator
  arguments without negating `i32::MIN`.

Deviations: the empty `heapreplace` exception becomes insertion plus `None`, as
specified by Hirsel's API. Python references and Rust unsafe holes become typed
unique owners and checked replacement/swaps, preserving every owner on failure.
The stored three-way comparator replaces Python `<`/Rust `Ord`; `push_pop` tests
input <= root explicitly. Only root updates are exposed, so Go Fix's upward
fallback and arbitrary-index updates are unnecessary. The existing Zig sift
schedule is retained, including its tie choices; no stable order is promised.
A private predicate ends comparison loans before moving either owner. Tests use
integers and fixed seeds instead of Python's floating random stream, and omit
foreign dynamic-type errors and Rust's leaked PeekMut behavior, which have no
corresponding API here.

## Reproduce the standalone experiment

`bench/priority-queue-combined/run.py /absolute/evidence-directory` builds one
Hirsel binary and a Rust `BinaryHeap<Reverse<u64>>` control, with separate
comparison-counting diagnostics. It compares `push_pop` to guarded push+pop,
and `replace_top` to pop+push, using identical LCG inputs, duplicate keys,
construction, final drain and full-output hashes. Every executable is checked
against a sorted-multiset oracle before sampling. Ten seeded interleaved rounds
run for k=1,16,64 and 8192 updates, pinned to CPU31 under the existing caps and
nonblocking timing-lock attempt. If the CPU is reserved by a long campaign,
only instruction and allocation evidence is used. Setup and finalization are
included. Separate diagnostic
runs count actual comparator invocations, including construction and drain.
Total and user instructions/cycles are retained; instruction ratios are
session-local. A cycle or wall-clock speedup requires an exclusive timing session.
Historical LangArena results are unchanged.

## Measured result

`replace_top` reduced full-process instructions by 12–30% versus pop+push in
these bounded workloads. `push_pop` used 6–7% more total instructions than the
guarded push+pop sequence (50–74% more user instructions). The additive contract
is implemented; a general heap speedup is not established.

On the 2026-10-22 session, all 180 fresh-process samples completed with matched
full-output hashes and 100% running hardware counters. Each row has ten seeds
and interleaves the combined operation, its existing-API sequence, and Rust.
The Hirsel `--release` profile emits LLVM O2; Rust uses the pinned compiler with
`-O`. Full process setup, input generation, construction, drain, output and
cleanup are counted. k=1,16,64; 8192 updates per process. Diagnostic Callgrind
runs use a separate LLVM O0 build of the same pure source; Rust comparison
counters use a separate instrumented build. No comparison instrumentation is
present in timed binaries. Single-element controls check 8192 compare-and-replace
guard calls and zero comparisons for unconditional replacement.

The CPU31 lock was occupied by a long campaign. Under the host timing-lock rule,
nonblocking lock attempts allow instruction-only evidence to proceed. Wall-clock
timing remains a follow-up. Ratios below divide median instruction counts in
each interleaved row; lower is better.

| Operation | k | Combined / existing instructions | User instructions | Combined / Rust instructions | User instructions |
|---|---:|---:|---:|---:|---:|
| push_pop | 1 | 1.0741 | 1.7411 | 6.7167 | 4.4550 |
| push_pop | 16 | 1.0655 | 1.6607 | 6.6841 | 4.5029 |
| push_pop | 64 | 1.0612 | 1.5045 | 6.7707 | 4.6370 |
| replace_top | 1 | 0.8847 | 0.6344 | 7.2300 | 7.2574 |
| replace_top | 16 | 0.7636 | 0.4572 | 7.0610 | 6.6894 |
| replace_top | 64 | 0.7003 | 0.3799 | 7.0721 | 6.6551 |

Full-process instruction medians (total / user) and actual comparator medians:

| Operation, k | Existing instructions | Combined instructions | Rust instructions | Comparisons existing / combined / Rust |
|---|---:|---:|---:|---:|
| push_pop, 1 | 11,235,312 / 1,125,114 | 12,068,231 / 1,958,977 | 1,796,740 / 439,724 | 8198 / 8192 / 8192 |
| push_pop, 16 | 11,461,301 / 1,209,494 | 12,211,536 / 2,008,614 | 1,826,948 / 446,072 | 9059 / 8816 / 8811 |
| push_pop, 64 | 11,709,876 / 1,441,945 | 12,426,482 / 2,169,454 | 1,835,319 / 467,857 | 12355.5 / 11585 / 11437.5 |
| replace_top, 1 | 15,106,842 / 4,841,602 | 13,365,654 / 3,071,356 | 1,848,627 / 423,204 | 0 / 0 / 0 |
| replace_top, 16 | 18,201,109 / 7,821,042 | 13,898,439 / 3,576,104 | 1,968,332 / 534,590 | 79467.5 / 16810 / 16804.5 |
| replace_top, 64 | 20,132,344 / 9,693,082 | 14,098,694 / 3,682,708 | 1,993,562 / 553,364 | 130084 / 19166 / 19023.5 |

Separate production Callgrind profiles retain deterministic full-process
instruction counts and exact allocation-function entry counts. Allocations below
mean `hirsel_rt_v2_alloc` entries, not host bytes or whole allocator statistics.
Each profile also records libc malloc/calloc/realloc entries for all participants.

| Operation, k | Existing Callgrind Ir | Combined Ir | Rust Ir | Runtime allocation entries existing / combined |
|---|---:|---:|---:|---:|
| push_pop, 1 | 1,247,106 | 2,080,926 | 496,620 | 5 / 5 |
| push_pop, 16 | 1,331,528 | 2,130,600 | 502,881 | 5 / 5 |
| push_pop, 64 | 1,563,785 | 2,291,261 | 524,960 | 7 / 6 |
| replace_top, 1 | 4,963,570 | 3,193,434 | 480,223 | 18 / 14 |
| replace_top, 16 | 7,942,678 | 3,698,076 | 591,393 | 18 / 14 |
| replace_top, 64 | 9,814,844 | 3,804,356 | 610,230 | 18 / 14 |

Cycles (total/user) are retained in the raw artifact and are descriptive only;
no wall-clock or cycle speedup is claimed without the exclusive timing session.


This is a bounded standalone experiment. Startup and runtime overhead matter
at these sizes, and shared-host cycle counts are noisy. These measurements
do not establish a LangArena improvement or attribute historical ratios to heap
operations. The full retained artifact includes per-cell raw perf/stdout/stderr,
exit status, SMT snapshots, comparator profiles and source/compiler/std/runtime
SHA-256 identities.

Compiler executable SHA-256: `78d7040c5ae0238a419eca627ffb7765795092f8572c38674234768794dc3b8e`.

Priority queue source SHA-256: `86094c6bad34f3d3e0e58060d810170700c0eed993a83b3d8eb0ac908852127c`.
