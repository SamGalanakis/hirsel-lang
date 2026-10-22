# Graph/Maze A* — la-astar-9, 2026-10-22

Both ports remove measured source overhead while preserving upstream search.
Baseline: `8bedce64`; upstream LangArena:
`3c9f84ddaeec834c0631747cb789e8cfd602f993`.

Graph separates its A* loop from mode dispatch, reserves two score arrays and
uses upstream's i32 score width. Maze reserves its three score/parent arrays
and restores their upstream i32 width. Both private heaps move a hole with
copyable entries instead of swapping both entries at each level. Heap keys,
neighbor order, strict g/best-f admission, duplicate frontier policy and first
target-pop termination are unchanged. Graph's shared BFS/DFS Entry stays i64.

## Ten-round instruction verdict

Full unchanged `run.js` inputs, seed **9009**, 10 interleaved rounds for each
original/candidate/Rust pair (60 checksum-validated processes). Median counts:

| Task | Original instructions | Candidate instructions | Rust instructions | Candidate/original | Hirsel/Rust before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Graph::AStar | 61,278,166,884 | 22,275,625,420 | 14,907,579,079 | 0.364 | 4.111 → 1.494 |
| Maze::AStar | 25,382,539,038 | 14,147,574,378 | 5,601,759,490 | 0.557 | 4.531 → 2.526 |

<!-- Graph::AStar: instruction reduction 63.65%; cycle C/O 0.319; timed interval C/O 0.305; median RSS KiB original/candidate/rust 318,184/257,094/83,968. -->
<!-- Maze::AStar: instruction reduction 44.26%; cycle C/O 0.403; timed interval C/O 0.384; median RSS KiB original/candidate/rust 129,746/106,480/195,854. -->

These are whole-process inherited-tree `perf stat cycles,instructions` counts,
including preparation, warmup and kernel work. Both counters ran **100%** in
every sample; no scaled partial-PMU verdict. Affinity: CPU23, AMD Ryzen 9 5950X,
SMT sibling CPU7; conventional `/tmp/hirsel-micro-cpu23.lock` acquired
immediately and retained for this session. CPU31 was never used. Shared-host
SMT contention remains possible, so wall-clock results are supporting evidence
only, with no cross-session or published-host speed projection. Runtime
interval/cycle ratios and process RSS are retained with the raw session.

## Attribution before choosing the change

Instruction-sampling profiles of unchanged current source identify Graph
search (61.94%), push (7.10%), pop (0.99%); Maze astar (48.86%), push (4.29%),
pop (5.41%). Sampling used `instructions:u`, period 2,000,000, no lost samples.
Graph's mode branch appears inside every score-initialization iteration.

Upstream-derived instrumentation, one full-size search (diagnostic only):

| Task | Heap pushes | Heap pops | Peak frontier | Relaxations | Walkable neighbor visits |
| --- | ---: | ---: | ---: | ---: | ---: |
| Graph | 23,842 | 1,173 | 22,670 | 23,841 | 25,625 |
| Maze | 162,100 | 162,082 | 31 | 162,099 | 324,182 |

Neither full workload happens to repeat a relaxation; fixtures explicitly do.
An independent dual-heap model follows upstream events and compares every
old/candidate heap array and popped key to Rust BinaryHeap. Logical entry
writes fall 442,410 → 257,554 for Graph and 1,351,394 → 998,870 for Maze;
comparisons remain 240,520 and 925,014. These are model writes, not machine
store or allocation counts.

Per-search scalar initialization remains 1,000,000 slots for Graph and
5,880,000 for Maze. Payload bytes halve: 8,000,000 → 4,000,000 and
47,040,000 → 23,520,000 respectively. Arrays reserve once rather than growing
18 times per Graph lane or 20 times per Maze lane. Maze heap entries retain
16-byte padded size despite the score narrowing; no aggregate packing gain
is claimed. Full Graph scores are bounded by 999,998 and full Maze scores by
1,962,793, with parent indices below 1,960,000: all fit i32.

Separate measurement-archive builds (LLVM O3, same source, regions-on-whole):

| Task | Physical runtime allocations, before → after | Peak live runtime bytes, before → after |
| --- | ---: | ---: |
| Graph | 1,915,320 → 1,874,520 | 153,879,120 → 145,830,752 |
| Maze | 6,900 → 2,112 | 151,697,584 → 102,276,960 |

All allocations are freed. Both before/after have zero deep copies, COW
detaches, copied blocks and copied bytes. Counters cover physical runtime
heap operations over the whole process, including setup; attribution is site0.
Sequence-growth memmoves and heap entry stores are outside the deep-copy
counter. The allocation deltas are exactly 34 fewer growth allocations per
Graph search × 1,200 searches, and 57 fewer per Maze search × 84 searches.
Instrumented allocation binaries never enter the instruction comparison.
Rust's separate glibc interposer observes 1,891,131 successful host allocations
and 5,547,409,076 requested bytes for Graph; 4,224 and 2,299,170,246 for Maze.
Its allocator boundary differs from Hirsel's physical runtime objects, so
these counts cannot form an object-for-object ratio.

## Correctness

| Check | Original | Candidate | Rust reference |
| --- | ---: | ---: | ---: |
| Graph test.js checksum | 36 | 36 | 36 |
| Graph run.js checksum | 1,406,400 | 1,406,400 | 1,406,400 |
| Maze test.js checksum | 6,771 | 6,771 | 6,771 |
| Maze run.js checksum | 2,906,799 | 2,906,799 | 2,906,799 |
| Private H fixtures | 53/53 | 53/53 | — |
| Private LLVM fixtures | 53/53 | 53/53 | — |
| Private Cranelift fixtures | 53/53 | 53/53 | — |
| Graph/maze input/path fixtures in upstream Rust | — | — | 51/51 |
| Small/full BFS, DFS, maze BFS/generator importers | 8/8 | 8/8 | 8/8 |

There are two additional Python model checks. The candidate Buck2 harness
executes all five Python tests, including 159 actual Hirsel fixture executions.
Baseline H and both model checks passed before fixing a native-only CLI setup
error; both baseline native backends then passed in the separate scoped run.
No zero-selection result is counted. Fixtures cover no path, singleton start
= target, equal priorities, repeated relaxation, duplicate keys, and multiple
routes. Maze asserts complete paths, including tie-selected midpoint, not only
length. A nonadmissible Graph fixture returns the four-edge route despite a
two-edge alternative; BFS is not its oracle. Full Rust Maze path is 30,661
cells with midpoint (525,631), product 331,275; 84×30,661+331,275=2,906,799.

Reproduce the registered fixtures:

```sh
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test root//:gate-langarena-astar-9
```

The harness appends private tests to temporary copies of the actual modules;
it does not widen production visibility or commit generated source. Registry
addition is confined to the existing custom Python harness generator.

## Identity and retained attempts

Original executable SHA-256: `a9c405d674d5e9d7390384aa301416128c7e220e1f91ac271da8ced546ac63dd`.
Candidate executable SHA-256: `352f8064d7dda5158409b3cd1228c4aca930d3b592eac9f24cedc5ac3c6aae2c`.
Rust executable SHA-256: `468817173ea176c7c80ee1ddd3e4b0233eecf53a209419de392ec080e0f39f4f`.
Compiler executable SHA-256: `f7ec897cad1d2c251d1964b3cc28b553ce655164de5bce048745fe900a47b4c2`.
Full original consumed-input manifest SHA-256: `3913613d3b679e2ff9ea86bdac553b9783b88ca84a853c2870ee35cb4f3cf255`.
Interleaved consumed-input manifest SHA-256: `cd5aa2be2b568ce26f8233d6d3d00c8a22d5ce1ce8a1cede21bad60a5960aa4c`.
Rust source/dependency/compiler/sysroot rebuild manifest SHA-256: `ddda5d2d95bcb3d4ae609ad490b941ad7214333309e5526de21e0bda716e8d6b` (3,002 inputs, unchanged rebuild binary).
Raw completed campaign JSON SHA-256: `869968e1bb7f5094cbeeec289d8393bd15983789a5b3fa552bef03b82341f2e7`.
All 3,134 original compiler/stdlib/toolchain/source inputs, candidate inputs,
reference sources, configs and binaries were hashed before consumers and
verified unchanged afterwards. Edits/formatting preceded the freezes. Raw
logs, profiles, counts, scripts and manifests are retained in the lane's
external `la-astar-9` evidence directory; no other owner's outputs were changed.

Preserved failed/setup attempts, excluded from performance ratios:

- `build.py --output` is unsupported (exit2); used the Python build API.
- Cargo's wrapper placed the first Rust build outside its source tree; the
  first launch used the wrong path (exit127), then an owned explicit target
  directory was built and validated.
- New test script initially lacked executable permission; then native tests
  rejected interpreted-only `--max-steps`. Fixed both setup errors; original
  H53 and later both native53 runs passed.
- First candidate failed LANG-4 because `heuristic` shadowed a local; the
  frozen successful candidate uses `start_h`.
- First campaign called the wrong perf parser after one passing process;
  the corrected session uses distinct files and all ten rounds.
- A premature Maze annotation read an unfinished second diagnostic profile;
  both perf recordings and the failed/complete annotations are retained.

## Reference adaptations and licenses

Algorithm sources: LangArena `rust/src/benchmarks/graph_path.rs:218–315` and
`maze.rs:352–468`, at the revision above. Test checksums come from
`test.js:181–188,227–234`. LangArena's MIT license and Konstantin Makarchev's
copyright notice remain in `bench/langarena/LICENSE` and `UPSTREAM`.
The existing Rust binary-heap reference is
`a7c37f9 library/alloc/src/collections/binary_heap/mod.rs:807–882` (MIT/Apache-2.0).

Existing adaptations retained: row-major u64 Maze vertices replace coordinate
pairs; safe list accesses and copied hole assignments replace unsafe Rust
holes; Graph keeps the shared i64 heap payload for BFS/DFS while restoring its
A* score arrays to i32. Early-stop downward sifting retains the port's original
heap arrangement and exact pair order, verified against the reference heap.
The benchmark's fixed start/target and generator, neighbor order, heuristic,
wrapping checksum, warmup and stop condition remain unchanged. No closed set,
BFS replacement, filled/truncate dependency, shared compiler/runtime rewrite,
language/TIR/LIR/runtime-ABI or public API change was introduced.
