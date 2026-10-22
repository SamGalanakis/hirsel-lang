# BWT rank and inverse buffers — la-bwt-9

Baseline: current fetched main `8bedce64`; upstream LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`. Native LLVM O3 release, LLVM 21.1.8. No language, compiler, runtime ABI, shared helper, sort implementation or public API change.

## Change and reference

Port follows upstream `rust/src/benchmarks/compress.rs:32-123` (cyclic rank doubling), `159-197` (stable inverse mapping), and `225-244` (full decoded-data equality). Upstream MIT license and copyright are retained in `bench/langarena/LICENSE`; project distribution notices are unchanged.

The initial counting pass orders equal bytes by suffix index. Equal rank pairs must already have equal first ranks, so by induction stable ties remain ordered by suffix index in every doubling round. The suffix itself can supply the tie key for the existing noncapturing, unstable `list.sort`. Removing the redundant previous-order field reduces Pair from four u64 fields (32 bytes) to three (24 bytes), with identical stable equal-rotation ordering and primary index.

Pairs snapshot both old ranks for every suffix before sorting. After sorting, all new ranks can overwrite the existing rank list: every suffix is written once, and subsequent pair construction begins only after the complete rank pass. A local running rank replaces the previous indirect rank lookup. The pair allocation is retained across rounds. The number of doubling rounds and the comparison keys remain unchanged.

Encode and inverse output lists reserve n bytes and append each final byte directly, removing the scalar zero-fill followed by overwrite. Inverse counting, stable next mapping and traversal are unchanged. Shared zeros/frequencies/copy helpers and all Huffman/LZW/arithmetic branches are untouched.

Adaptations/deviations from upstream: Hirsel List uses existing counted allocation and u64 collection indices; the comparator carries rank pairs with suffixes because its API cannot capture rank storage. Rank/pair allocations are reused and outputs are initialized on append rather than Rust’s fresh zero-filled Vecs; these measured storage changes preserve the rank-doubling algorithm. Counts remain u64 (upstream inverse uses i32 counters). No narrower indices, early termination, alternate transform, custom sorting, unsafe allocation or pending list API is used.

## Frozen instruction session

Ten seeded interleaved rounds, seed 20261022, CPU 5, nice -n 10 and tools/capped (900 s, unchanged resource caps). CPU31 was never used. CPU5 convention lock `/workspace/tmp/hirsel-micro-cpu5.lock` was acquired immediately and held for all rounds. The session never held another CPU lock. All 60 task executions passed the original full-size run.js checksums: encode 3600000; decode 216100216. Both perf instructions and instructions:u, cycles and cycles:u have 100% running coverage in every sample. Counters include the pinned whole process, startup, preparation and warmup; compiler/build processes are excluded. Decode therefore includes one encode during preparation. RSS is whole-process peak. The source, std, toolchain/runtime, manifests, grants, compiler/helper and executable identities were verified unchanged after the final consumer.

Instruction counts are the verdict. Kernel costs include allocation/paging and vary with host load; user-only instruction medians are reported separately. Cycles/time are supporting observations only: SMT sibling activity is retained in raw samples, and no wall-clock speedup or cross-host projection is claimed.

| Task | Rust instructions | Before instructions | After instructions | After / before | Before / Rust | After / Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Compress::BWTEncode | 5,007,841,120 | 12,779,228,893 | 8,240,767,370 | 0.645 | 2.552 | 1.646 |
| Compress::BWTDecode | 8,720,459,802 | 33,814,441,690 | 30,608,967,437 | 0.905 | 3.878 | 3.510 |

| Task | User instructions before → after | Cycles before → after | Peak RSS KiB before → after |
| --- | ---: | ---: | ---: |
| Compress::BWTEncode | 8,998,069,074 → 8,004,942,758 | 7,356,152,906 → 2,586,323,312 | 12,572 → 10,908 |
| Compress::BWTDecode | 29,623,716,317 → 27,586,266,704 | 15,110,503,342 → 13,307,791,054 | 62,342 → 46,848 |

## Allocation and copy coverage

Allocation counts come from separate instrumented native binaries built from the same frozen source snapshots, compiler, runtime archives and whole-program configuration. They cover Hirsel’s managed heap, including preparation, warmup and harness parsing; they exclude Rust/libc allocator operations, compiler allocations and native dependency arenas. Instrumented runs are not timing samples. All allocations are freed.

| Task | Managed allocations before → after | Runtime copied bytes before → after |
| --- | ---: | ---: |
| Compress::BWTEncode | 1,785 → 597 | 0 → 0 |
| Compress::BWTDecode | 1,471 → 1,432 | 0 → 0 |

Runtime copied_bytes counts COW/deep-copy/bulk runtime copies; zero does not count scalar stores, list element construction or sort moves. Both phases have zero COW detaches/deep copies. Source-level output stores change from zero-fill + overwrite (2n) to append (n) for encode and inverse; inverse next-map zeroing remains. At n=100000 encode has 17 doubling rounds: retaining pairs and overwriting rank removes 33 managed allocations per transform, or 1188 over 36 warmup+timed transforms. At n=1000001, decode preparation has 20 doubling rounds and removes 39 allocations; inverse’s own allocation count is unchanged.

## Correctness

`tests_bwt_9.py` computes complete cyclic rotations with Python’s stable sort, independently of rank doubling, and checks all transformed bytes, primary index, encode/inverse round trip, and inverse from independently supplied transformed data. There are 59 unique bounded vectors: empty, one byte, all equal, ABRACADABRA, banana/BANANA, odd/even lengths, periodic/equal rotations, sorted/reversed bytes, NUL/ff/high bytes, every ternary {0,1,255} string of lengths 1–3, and seeded binary inputs through length 31. A separate Rust oracle extracted unchanged upstream BWT functions and passed all 59 expected transforms and indices.

Frozen baseline: 59 vector tests passed on each of interpreter, LLVM and Cranelift (177 actual Hirsel tests); existing codec_vectors control passed (1 test). Candidate: 60 tests passed per engine (180 actual Hirsel tests under three passing Python tests in Buck2 gate-langarena): 59 vectors plus exact upstream test.js encode 200 / decode 100204 and full decoded equality, with a count assertion rejecting zero selection. The new Python class is registered in the existing documented custom gate-langarena harness, not a Rust main.rs. Candidate existing codec_vectors control passed (1 test). The existing NativeProtocol.test_all_implemented_quick_checksums Buck2 integration control passed (1 Python test validating all 47 registered quick task checksums). Total scoped Python tests: 4 passed; full-value Hirsel executions: baseline 177 → candidate 180, plus 1 existing codec control in each phase. All controls ran nonzero intended selections.

Reproduction commands (exact selectors):

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test root//:gate-langarena -- --test-arg=BwtOracles
.buck2/bin/buck2 test root//:gate-langarena -- --test-arg=NativeProtocol.test_all_implemented_quick_checksums
```

## Profiles, limitations and retained attempts

Baseline full-size cycles:u profiles were taken separately on CPU5 before editing. Encode: bwt_encode 27.41%, partition_range 15.81%, partial_insertion 13.88%, quicksort 5.67%. Whole-process decode: preparation sort partial_insertion 36.22%, partition_range 13.62%, bwt_decode 28.53%, bwt_encode 5.75%, frequencies 4.45%. Skid/unresolved/kernel buckets prevent complete attribution. These singleton profiles are diagnostic, not speedup evidence. Remaining standard PDQ sort costs were routed with exact frozen profiles to la-sort-self-8; that lane acknowledged them as a follow-up and changes neither list.sort nor PDQ internals.

All evidence is retained under `/workspace/code/hirsel-briefs/la-bwt-9/`: before/after source snapshots, compiler/helper, native inputs, timing and allocation binaries, baseline identities, final-counts.json, per-sample raw perf/RSS/stdout, allocation site ledgers, generated independent vectors, pinned Rust sources/oracle and scripts. SHA-256 identities include every consumed tracked/untracked source/manifest, and final-counts.json records before/after frozen verification. Reports are per-session; shared historical RESULTS.md is unchanged.

Preserved unsuccessful/setup attempts: setup-1 copied toolchain symlinks rather than their referents (failed before any accepted build); initial direct oracle launch omitted HIRSEL (three setup errors); next attempt used a noncanonical package name (three manifest failures); initial existing-codec selector used a dot rather than the harness path format (zero tests, not counted as a pass); first profile passed an absolute config path outside grant-relative lookup (exit 1). Each was corrected and retained with its own log. Trials a–e and their singleton counts are separate and ineligible as final ratios. No failing attempt or zero selection was removed from evidence.

## Executable and dependency identities

| Input | SHA-256 |
| --- | --- |
| cli | `f7ec897cad1d2c251d1964b3cc28b553ce655164de5bce048745fe900a47b4c2` |
| native-build | `6f66e39a65f4b54d9bf5b6916a4d20ca42333c153a00b14ff5bfe5f8e968a0f4` |
| before | `acbee3c0775d3fb50193b2f3ec34df4ea8eecace0baa6976986c4b0c0f063c78` |
| after | `372e7c71bd47f63b787f8609161c3809391c8bbc1a083d64f31b104dcae44e7c` |
| before-alloc | `311e15583339936a1c88dbbf59fe6acff3e8778fa476f29a8afa234c1a54feed` |
| after-alloc | `587d86725a03f0a16a55f03a581bff9f1b93ea301e77124a2835514e4a5378b3` |
| rust | `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3` |
| before-src/stdlib/std@1/list.hir | `f7090f563aa5796d7c8d5153e4adbe6f8e1e48dc8c34d4ec6ce2e6addad5df6b` |
| after-src/stdlib/std@1/list.hir | `f7090f563aa5796d7c8d5153e4adbe6f8e1e48dc8c34d4ec6ce2e6addad5df6b` |

Frozen before source tree: `e70e41f05ecd82dad15d6b8a28aecf3d699330795c342aa7721af2a681134c4a`.
Frozen after source tree: `50f9dd0570e97e3fe6123430e9cd2f14b6d2724a4d975e3497fef9956ff9c674`.

No mandatory BWT functionality remains open. Standard sort cost is an acknowledged independent follow-up. Wall-clock timing is intentionally not promoted to a speedup claim; use the retained locked session only with its SMT activity and whole-process coverage.

## Landing integration

The first tools/land workspace build refused the inherited main-branch E0063 in checked_shadow_cost_7.rs:259 (missing Body.scoped_parameters). Per the live host rules, reuse the already tested owner repair e7d816f036401a7c86d19930f53410118b74bd8b unchanged: one empty scoped_parameters field and removal of its exact known-failure row. Its seven focused checks already passed; no BWT source or measurement input changes and no repeated BWT campaign. The failed 1463-second build log remains separately retained as logs/la-bwt-9-land-attempt-1.log.
