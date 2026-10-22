# Sort::Self: allocating driftsort, 2026-10-22

The current-main baseline is 35d4a54a, not the historical 9f161d96 campaign.
The native LLVM release profile reproduces WikiSort's cost: merge_internal
46.88%, block_swap 20.81%, merge_blocks 15.60% of sampled user instructions.
The fix adds list.sort_stable_fast, preserving list.sort_stable's noalloc
contract. Sort::Self selects the allocating API to match upstream Vec::sort.
The task input, clone, PRNG, iterations and exact checksum are unchanged.

Reference: Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f, core stable
sort/{mod,drift,quicksort,merge}, shared/{pivot,smallsort}. Exact source ranges,
allocation rationale and every adaptation are in docs/prior-art-sort.md.

## Measurement protocol

Release artifacts, compiler, runtime archives, source hashes, config and grants
were frozen outside buck-out before mutation. Full run.js Sort::Self uses
400,000 i32 values, 10 warmup and 50 measured iterations; checksum 61757772.
Runs invoke bench/langarena/compare.py's Session.sample task runner, pinned
to CPU 31, with perf stat instructions and instructions:u (100% PMU coverage).
User instructions are the verdict; kernel instructions vary with contention.
No CPU timing lock was acquired, and wall-clock/cycle ratios are not claimed.

The first nine samples (three seeded rounds, baseline/initial/Rust) are
retained as diagnostic. A separately frozen ten-round run of the selected-base
partition alternative is also retained in full: 30/30 checksum passes and
100% coverage. Its median user instructions were 35,432,858,091 baseline,
18,941,653,354 selected-base, and 4,942,139,533.5 Rust. The initial conditional
write used 15,885,640,403 (three-round median). The checked destination-base
arithmetic increased instructions; the accepted implementation restores the
conditional write and keeps Rust's eager mode. The final single control uses
a separately named binary and source identity; it is not pooled with either
prior candidate or described as another timing campaign.

## Accepted result

Final user instructions: **15,883,457,275**, versus baseline ten-round median
35,432,858,091 and Rust median 4,942,139,533.5. Final **H/R = 3.214x**,
a **55.17% reduction** from current-main WikiSort. The final control
passes the exact full-size checksum with every counter at 100% PMU coverage.
Final allocation control: 524 allocations / 524 frees, zero live bytes,
peak 9,630,352 bytes; exact full-size checksum passes.

Final executable SHA-256: `d5fe3ae75c5a8b924c48409f632fae9db461293853e7b5fe5f0711a6f6b93b95`.
Baseline: `3e09dd61dddf1031cffe30446fe7eb21c31af53bdd1b6881cdd7f6062d89b06f`.
Rust: `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3`.

## Allocation controls

The baseline and initial/refined candidates all pass the full checksum.
Baseline: 404 allocations / 404 frees, peak 3,230,320 bytes, zero live bytes.
Driftsort: 524 allocations / 524 frees, peak 9,630,352 bytes, zero live bytes.
The increase is exactly two scratch allocations per sort, including warmup.
No deep copies or copied blocks occur. Counts are from separate measurement
builds, not the production binaries used for instruction counts.

## Validation

Four new Rust harness tests cover 152 small pattern/length pairs, stable
occurrence ordering, permutation integrity, exact first callback failure,
all 129 resource finalizers and zero live heap bytes. Interpreter, heap
executor, LLVM and Cranelift agree. A native 1,100,000-element vector runs all
eight patterns beyond the full-scratch allocation cap, on LLVM and Cranelift.
The affected run passed 22 tests: four new driftsort controls and 18 existing
sort/failure/ownership controls. The final conditional-write restoration passed
all four affected controls. No conformance fuel limit is raised.

Remaining follow-up: CPU-lock wall-clock comparison. Instruction parity with
Rust remains open; checked index indirection and the final owner permutation
are deliberate costs of retaining move-only ownership without new IR/runtime
constructs.
