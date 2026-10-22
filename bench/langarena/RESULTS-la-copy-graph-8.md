# CacheSimulation copy traversal, 2026-10-22

Baseline: current main `35d4a54a`. Native LLVM release, unchanged LangArena
`3c9f84ddaeec834c0631747cb789e8cfd602f993` Rust reference and full `run.js`
(`values=5000`, `size=2500`, `iterations=5000`, checksum `3263751556`).

The brief's `Copies`/SipHash table is already absent: `1641553b` removed it
because MODE-2 requires one fresh ordinary block per holder. Reintroducing a
memo would violate uniqueness. Both ordinary diamonds becoming exclusive trees
and explicit Shared diamonds retaining pointer identity have runtime tests.

The current-main profile instead attributes 6.76% of samples to copy redirection
and 5.93% to `holder_views`. Its loop queues one work item per element, causing
scratch growth on every copied collection. Keep one descending cursor per loop
instead, preserving traversal order, variant selection, column indices, arena
precharge, shallow detach credits and Shared boundaries. Scratch now depends
on descriptor complexity, independently of the number of elements.

| Whole-process metric | Before | After |
| --- | ---: | ---: |
| Hirsel instructions, median of 3 | 27,080,690,642 | 26,734,715,386 |
| Rust instructions, median of 3 | 12,574,913,247 | 12,571,590,448 |
| Hirsel / Rust instructions | 2.153549 | 2.126598 |
| Host allocation calls | 89,413 | 27,257 |
| Host requested allocation bytes | 2,817,940,872 | 678,996,040 |
| Logical Hirsel block allocations | 33,218,773 | 33,218,773 |
| Copied Hirsel blocks | 21,190,685 | 21,190,685 |
| Copied Hirsel bytes | 2,200,296,416 | 2,200,296,416 |

Instructions fall 1.28%; host allocation calls fall 69.5%, requested bytes 75.9%.
The host counter interposes successful glibc malloc/calloc/realloc/aligned
allocation requests on the benchmark process; it counts scratch and slab
requests, whereas the separate runtime measurement archive counts logical Hirsel
blocks. Wrapper process counters are excluded. Allocation traffic is cumulative,
not RSS. The interposer source is retained with the evidence.

Measurements use `compare.py`'s `Session.sample` and task runner, CPU7,
`perf stat` instructions/cycles, three alternating Hirsel/Rust samples per
version, including preparation, warmup and checksum. All PMU counters report
100% running. Profiling uses `perf record -F 997 -g --call-graph dwarf,16384`
on the release task runner before any implementation change; no lost samples.
No CPU timing lock was acquired and no wall-clock speedup is claimed.
The compiler executable is identical between measurements; only the runtime
archive changes. Before compiler, runtime archives, executables, configs,
grants and source hashes were saved outside build output before editing.

Raw profiles, per-sample counters/SMT activity/checksum logs, allocation counter
source and binaries: `/workspace/code/hirsel-briefs/la-copy-graph-8/{before,after}`.

Reference and semantic differences:

- Rust `a7c37f9f0ee5`, `library/alloc/src/slice.rs:425-432` clones elements
  directly without queuing an element-sized worklist. Hirsel uses descriptor
  cursors in reverse order to preserve its existing walk and avoid native
  recursion for arbitrarily deep graphs.
- CPython `c1c4c18dfc7c`, `Lib/copy.py:121-126,158-161,171-177` memoizes object
  identity; `Lib/test/test_copy.py:284-293` tests a shared diamond and
  `:395-404` a self-cycle. Hirsel's ordinary holders must become independent
  storage (MODE-2), and ordinary cycles cannot be constructed. Explicit Shared
  roots retain the same root and gain exact credits without traversing contents.
- Bounded arenas retain their existing subtree-charge memo, since repeated
  source edges must be charged independently before allocation; this task
  uses the heap and does not exercise that table.
- No benchmark source, language rule, runtime ABI or public API change.

Validation: the copy selection passes 19 existing tests before the change,
20 after; the new wide-list regression fails on the old walk with 12 host
allocation calls and passes with a four-call upper bound after. Six root tests
pass, including exact counts and genuine ordinary sharing rejection. Four
selected corpus cases execute and pass in the interpreter. The copy-charge,
nested-root, SoA-column and aliased-list freeze controls pass four native LLVM
matrix cases and four
Cranelift parity cases (8 Buck2 actions); each LLVM case runs four
configurations, native images/executables and real-Io recording/replay. Both
LangArena quick and full checksums pass.

Setup failures are retained: an unsupported bare libtest filter (zero tests),
initial fixture descriptor registration ordering, and short native IDs (zero
cases). The corrected commands execute the intended tests without waivers.
