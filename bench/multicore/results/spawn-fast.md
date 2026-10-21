# L2 fork/join kernel

Ten seeded, interleaved rounds on CPUs 31 and 30, width 2, 10,000 immediately
joined children returning `i + 1` per sample. Every sample checks the triangular
checksum. The same optimized native-runtime test executable runs the ordinary
fiber path (`flags=0`) and the compiler-proved immediate/op-free path (`flags=3`).
Instruction counts are the verdict; elapsed times are ratios within this session.

| Fast / ordinary, median paired ratio | Value |
|---|---:|
| Instructions | 0.187705 |
| Cycles | 0.152767 |
| Elapsed time | 0.198405 |

[Raw samples](spawn-fast.json) include the executable digest, seed, CPU set and
all 20 counter samples. Reproduce with `bench/multicore/spawn-fast --help`.
This is B1's fork_join algorithm on F1's explicit native admission seam. It
isolates spawn machinery; it does not measure a compiled Hirsel program's
production Free provider, p99 latency, or recording overhead. That admission and
native replay remain F1-owned. Both controls use L2's stack pool, so the comparison
isolates avoiding the ordinary fiber and scheduler path after stack pooling.

The compiler's op-free proof rejects unknown/indirect operations and destruction
that can release roots or run user code. Fiberless unjoined children currently
require a trivially droppable result; other adjacent children use the immediate
join's destination, including owned results. Ordered execution and replay retain
fibers. Non-adjacent, unproved, or foreign-affine cases use the ordinary path.
