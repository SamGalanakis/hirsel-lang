# Graph adjacency memory: session 01a16201

Port **bounded `i32` adjacency IDs**, following pinned LangArena Rust, and reserve the outer list once. Keep 64-bit host indices/frontier records and the existing BFS/DFS/AStar algorithms, duplicates, insertion order, PRNG draws, AStar `target-v` heuristic, i32 score arrays, reserves, strict pruning and hole-based heap unchanged. Vertices outside `1..2147483647` now return a configuration error before allocation.

**Memory win with an instruction tradeoff:** prepared live allocator capacity halves. BFS instructions increase; no speedup is claimed. RSS is supporting evidence, with allocator variability reported explicitly.

## Frozen comparison

Base `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`; Rust `3c9f84ddaeec834c0631747cb789e8cfd602f993`. Native Hirsel LLVM release, unchanged Rust release (`rustc 1.99.0`); no LTO. Ten rounds, seed 20261022, three participants interleaved on CPU3. All **90/90** full-size `run.js` outputs pass the original checksum and `Summary: ..., 1, 1, 0` oracle. Every event has 100% PMU running coverage. Counters cover the entire process and descendants, including startup, graph preparation, warmup, timed searches and checksum; allocations use separate instrumented builds.

CPU31 was reserved and was never used. The ordinary-CPU instruction fallback used the nonblocking `/tmp/hirsel-micro-cpu3.lock`; lock wait was zero and no second lock was held. Shared-host wall times/cycles remain raw diagnostics: **wall-time ratios/rankings are open**. This is a three-participant direct frozen-binary adaptation of `compare.py:Session.sample`, rather than a whole-suite/incremental-build campaign. The graph-only registry changes compilation scope only; main, harness, full run.js/test.js, warmup, timed work and checksum boundaries are byte-identical. Exact argv, stdout, events, affinity observations and frozen inputs are in the raw artifact. These are current measurements, not the historical RESULTS baseline.

| Case | RSS before MiB | RSS after MiB | Rust MiB | After/before RSS | Instructions before B | After B | Rust B | After/before instructions | User instructions after/before |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Graph::BFS | 207.348 | 123.275 | 78.430 | 0.5945 | 10.5754 | 11.3694 | 8.4608 | 1.0751 | 1.1119 |
| Graph::DFS | 195.178 | 112.455 | 78.547 | 0.5762 | 11.4027 | 11.5707 | 5.9763 | 1.0147 | 1.0306 |
| Graph::AStar | 239.176 | 161.316 | 82.000 | 0.6745 | 22.5115 | 22.3912 | 15.1373 | 0.9947 | 1.0016 |

Ratios use medians of participant samples. AStar RSS varies markedly: before 239.137–272.258 MiB, after 156.246–179.813 MiB; its whole-process instruction change is small and user instructions are essentially flat. Do not attribute that small instruction difference to the port. BFS/DFS instruction increases are real costs of this change, including checked per-edge signed-ID conversion. Memory savings are independently proved by payload/class/backing measurements, not inferred from allocator/cache noise.

## Memory attribution

The same generated graph has **10,566,682 directed adjacency entries**, preserving both directions and duplicates; final PRNG state 116650. Rust reports 14,275,112 reserved row slots and outer capacity 500,000. Its exact requested live graph storage is 69,100,448 bytes: 12,000,000 bytes of Vec headers plus 57,100,448 bytes of row capacity. Its requested live payload is 42,266,728 bytes; spare row capacity is 14,833,720 bytes. Rust allocator accounting uses a separate System wrapper, not the scored binary.

| Prepared graph component | Before bytes | After bytes | Meaning |
| --- | ---: | ---: | --- |
| Live vertex payload | 84,533,456 | 42,266,728 | Edge count × 8 or 4; excludes unused slots |
| Row sequence length/capacity words | 8,000,000 | 8,000,000 | 16 bytes × 500,000 rows |
| Outer pointer payload | 4,000,000 | 4,000,000 | One 8-byte row reference per vertex |
| Outer spare capacity | 1,210,112 | 0 | Reserve removes amortized growth slack |
| Row spare/class rounding capacity | 43,079,920 | 15,637,480 | Reserved row capacity beyond live elements and sequence words |
| Outer sequence words | 16 | 16 | Separate from vertex payload |
| Live graph allocator capacity total | 140,823,504 | 69,904,224 | Above components; excludes block headers |
| All live owned allocator capacity at prepare | 140,824,480 | 69,905,200 | Graph plus 976-byte boundary/runtime objects |
| Current owned heap backing | 177,832,024 | 98,568,536 | Slabs + large backing, not resident bytes |
| Slabs | 172,621,824 | 94,568,448 | Occupied and empty slabs |
| Entirely idle slabs | 22,806,528 | 15,335,424 | Included in slabs, not additive |
| Backing allocation calls to prepare | 2,645 | 1,444 | Runtime backing calls, not block allocations |

Each live graph sequence also has an 8-byte counted block header: 4,000,008 bytes total, outside live capacity. Backing also includes free slots, slab metadata/alignment and large prefixes. The counts come from `ctx_stats` and every slab class at a stopped single-worker stdin boundary, using the existing measurement runtime via GDB; no runtime/ABI change. GDB snapshots and instrumented diagnostics are excluded from scored PMU/RSS samples. The region counters are zero for this workload; dependency-owned arenas remain outside measurement coverage.

| Phase/case | Before idle RSS KiB | After idle RSS KiB | Before post-prepare RSS KiB | After post-prepare RSS KiB | Before one-search peak live bytes | After one-search peak live bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| BFS | 6240 | 6208 | 197364 | 112164 | 157,106,128 | 86,186,848 |
| DFS | 6188 | 6156 | 197316 | 112148 | 142,452,688 | 71,533,408 |
| AStar | 6280 | 5884 | 197408 | 112148 | 145,801,440 | 74,882,160 |

Preparation-only peak live capacity is 140,824,560 → 69,905,280 bytes (zero warmup/zero searches). Post-prepare VmHWM equals post-prepare RSS in these diagnostic runs. The one-search figures include preparation and frontier/array reallocations; their excess over preparation is unchanged: BFS 16,281,568 bytes, DFS 1,628,128 bytes, AStar 4,976,880 bytes. These are live allocator **capacity** peaks, not simultaneous live vertex payload or RSS. The scored peak RSS table is whole-process VmHWM; the separate reset-high-water diagnostic below isolates one-search resident peaks. Do not obtain those peaks by subtracting peaks from separate processes.

Separate one-search phase diagnostics reset `/proc/PID/clear_refs` to 5 after preparation, then read VmHWM at a blocking boundary immediately after the timed search. This isolates the resident high-water mark during search (including the already resident graph), rather than subtracting process peaks. Zero warmup and one search are attribution-only; the scored 90 runs retain the full original counts. The Rust wrapper adds System request counters and the same reset, with graph destruction after search; its end snapshot is not a live-graph RSS comparison.

| Case | Before search-phase peak KiB | After search-phase peak KiB | Rust search-phase peak KiB |
| --- | ---: | ---: | ---: |
| BFS | 207,716 | 122,572 | 78,592 |
| DFS | 198,308 | 113,136 | 78,848 |
| AStar | 201,640 | 116,480 | 82,176 |

Rust diagnostics: requested-live preparation 69,100,996 bytes including 548 bytes of process bookkeeping; peak live with one BFS/DFS/AStar search is 69,609,188 / 69,863,140 / 73,363,140 bytes. System-wrapper counts include allocation/reallocation requests, exclude allocator metadata, and do not measure allocator reservation or resident pages. Their coverage differs from Hirsel runtime block counters.

BFS consumes 499,488 frontier pops; at most 914 entries are live in the Rust deque (capacity 1024). The Hirsel head-index list retains consumed entries, explaining its larger frontier peak. DFS has 5,281,374 pops with at most 28,567 pending entries; AStar has 1,173 pops and at most 22,670 pending entries. These separate Rust observations add only frontier length/capacity counters to the pinned methods. No deque change is claimed: adjacency width dominates the measured live-capacity saving, and adopting a different frontier without cost evidence would mix changes.

| Full run.js block allocations | Before | After | Peak live capacity before bytes | After bytes |
| --- | ---: | ---: | ---: | ---: |
| BFS | 1,856,951 | 1,872,849 | 157,135,440 | 86,216,160 |
| DFS | 1,855,859 | 1,871,757 | 142,482,000 | 71,562,720 |
| AStar | 1,874,520 | 1,890,418 | 145,830,752 | 74,911,472 |

All counted blocks are freed at exit; COW detaches/deep copies are zero. Counts cover the owned runtime heap including preparation, warmup and all configured searches. They exclude host/std bookkeeping and backing allocator calls; large-bin allocation counts do not expose cumulative allocated bytes, so no fabricated whole-process allocated-byte total is reported. Class capacities, live capacity and current backing bytes above are distinct metrics. Narrower rows follow a different class-growth schedule and slightly increase block allocation counts despite using much less live/backing memory.

## Traversal validation and provenance

New Buck target `root//:hirsel-cli-test-la_graph_rss_10`, selector `vectors::`, executed **3/3 Rust harness tests before and 3/3 after**, each running **56 Hirsel tests**: **168 before → 168 after** across H, LLVM and Cranelift. The 54 independent fixtures compare path-length results and complete popped-vertex order for all three algorithms. Five generated fixtures additionally compare every adjacency row and final PRNG state/draw count. Fixtures include singleton/disconnected/self/duplicate/cyclic/tie graphs and all 45 retained AStar Graph cases, including its deliberately nonadmissible longer result. Heap priority/tie/duplicate tests and exact BFS20/DFS28/AStar36 checksums are separate controls. Bounds 0 and 2147483648 are rejected by the sealed release candidate.

`bench/langarena/tests_astar_9.py` changes only its explicit Graph fixture literal suffix from u64 to i32; Maze and heap controls are retained. The adjacent `root//:gate-langarena-astar-9` target passed all 5 Python tests, including 53 unchanged controls on each of H, LLVM and Cranelift (159 backend cases). After validation, private helper files moved below `tests/support/la_graph_rss_10`, with an explicit `#[path]` in the existing standalone root; this avoids duplicate Cargo discovery and shared-registry checks requiring a main.rs. Helper/fixture behavior is unchanged; ordinary landing compilation covers the path move. No shared harness main or generated build file is committed.

Reference structure and vectors: LangArena `rust/src/benchmarks/graph_path.rs:9–118,157–188,218–290` and `test.js:165–188` at the pin above. MIT copyright Konstantin Makarchev; retained full license at [LICENSE](LICENSE). Hirsel adaptations: lists/head-index FIFO instead of Vec/VecDeque (pre-existing); u64 list indices require checked reads from bounded i32 IDs; checked conversion replaces Rust `as` at ingress; i64 frontier records/returns remain; explicit outer reserve matches Rust with_capacity. No graph flattening, algorithm swap, PRNG adjustment, AStar heuristic correction, language rule, IR or runtime ABI change.

## Reproduction and retained evidence

- [Raw session, full stdout, counters, allocator reports and identities](la-graph-rss-10/session-01a16201.json.gz). SHA-256 `33e36791d9b9d34b69298bf5e4e174a46e3317618d719ca0739080f4be11e0c7`.
- [Source, compiler, runtime and binary hashes](la-graph-rss-10/identities.json). Every frozen consumed input was verified after the ten rounds.
- `python3 crates/hirsel-cli/tests/support/la_graph_rss_10/make_vectors.py --upstream /tmp/ref-langarena` regenerates goldens by compiling the exact pinned Rust source with pop observers and no algorithm replacement.
- Decompress the raw artifact, select its `session` object, then `python3 bench/langarena/la_graph_rss_10.py --verify measurements.json`. The runner also accepts frozen participant command/cwd/input manifests for reproduction.
- Full local logs and immutable participants: `/workspace/code/hirsel-briefs/la-graph-rss-10/session-01a16201/`; named command logs `/tmp/la-graph-rss-10-*.log` are copied into that archive before handoff.

Preserved failed attempts: diagnostic helper copy ETXTBSY; diagnostic grant/output overlap refusal; selector-buffering diagnostic stopped with owned PID only; GDB callback attempts stopped/refused, followed by a successful command-list snapshot; initial new fixture ownership error (`copy adj` required). None is counted as a passing execution, used in ratios, or hidden by overwriting the original log. No completed baseline samples were rerun merely for setup movement.

## Remaining work

BFS retains consumed frontier entries and pays ~7.5% more whole-process instructions (11.2% user) here. Minimal reproduction is the unchanged Graph::BFS run.js case: 500000 vertices, jumps20, jump_len1000, 40 iterations/checksum50400. A follow-up can compare the existing deque against the frozen head-list, retaining pop order and seeded instruction/allocation evidence; a separate compiler range proof could remove checked nonnegative conversions without weakening bounds. DFS pays ~1.5% whole-process instructions (3.1% user); AStar user instructions are flat. No additional optimization credit is claimed. Wall-time ratios remain open; isolated one-search resident peaks are reported above.
