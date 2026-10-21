# Distribution compiler: fat LTO and PGO

Measured at f49299b5 with Rust 1.99.0 / LLVM 23.1.1 on the same Linux x86-64
host. Three complete seeded interleaved rounds (seed 20261021), CPU29 (SMT
sibling CPU13), HIRSEL_JOBS=1, one tools/capped command at a time. Each sample
records perf cycles/instructions and user counts plus /usr/bin/time peak RSS.
The verdict uses user instructions, compiler-bench's existing 1% threshold.
No wall-time ratio is claimed; ten rounds are required for wall-time evidence.

The 24 cells are `compiler-bench/run --quick`: seven examples plus syn3k,
each in check, debug Cranelift and release LLVM mode. The new option excludes
Rust pairs and larger scale programs. Native program caching is disabled so
repeated builds actually compile. All candidates use the same sibling std
precheck, native toolchains and runtime archives, from the same source tree.

The prior release profile used opt-level 3, no debug information, no cross-crate
LTO, bitcode disabled, and max(2, reserved CPUs) codegen units for measured
first-party crates (16 otherwise). The new, separate dist profile embeds
bitcode, uses fat LTO and one codegen unit. Dev/release, checked tools, build
scripts, proc macros and native runtime profiles retain their existing settings.
The CLI's allocator was already mimalloc v3: mimalloc 0.1.52, libmimalloc-sys
0.1.49; Buck2 compiles c_src/mimalloc/v3/src/static.c. Its allocator options stay
unchanged. No target CPU, panic strategy or hot/cold text grouping changes.

## Fat LTO alone

All 24 cases improve by 3.34–7.10% user instructions, geometric mean -5.21%.
Peak RSS ranges from -1.44% to +2.35% (geometric mean -0.24%). The largest RSS
increase is todo-api LLVM release: 934.5 -> 956.5 MiB. Unstripped compiler size
is 58,381,192 -> 48,865,088 bytes; size is descriptive, not an acceptance gate.

Instructions are billions; RSS is MiB; each value is the median of three runs.

| Program / mode | Release instructions | Dist instructions | Change | Release / dist RSS |
| --- | ---: | ---: | ---: | ---: |
| hello / check | 5.636 | 5.383 | -4.49% | 386.2 / 386.0 |
| hello / clif | 10.665 | 9.969 | -6.53% | 631.6 / 629.0 |
| hello / llvm-o2 | 10.498 | 9.803 | -6.62% | 633.0 / 631.0 |
| todo-api / check | 14.540 | 13.602 | -6.45% | 579.1 / 576.1 |
| todo-api / clif | 118.233 | 111.451 | -5.74% | 885.9 / 883.3 |
| todo-api / llvm-o2 | 163.405 | 156.807 | -4.04% | 934.5 / 956.5 |
| kvstore / check | 13.906 | 13.039 | -6.23% | 544.0 / 542.8 |
| kvstore / clif | 99.988 | 94.122 | -5.87% | 851.5 / 850.8 |
| kvstore / llvm-o2 | 134.929 | 129.142 | -4.29% | 912.2 / 906.5 |
| logq / check | 13.819 | 12.950 | -6.28% | 544.6 / 543.1 |
| logq / clif | 43.406 | 41.498 | -4.40% | 718.7 / 717.1 |
| logq / llvm-o2 | 55.152 | 53.311 | -3.34% | 739.7 / 736.5 |
| csvjson / check | 7.708 | 7.262 | -5.79% | 413.4 / 407.5 |
| csvjson / clif | 58.596 | 55.595 | -5.12% | 758.1 / 753.9 |
| csvjson / llvm-o2 | 78.510 | 75.615 | -3.69% | 784.5 / 782.3 |
| mdtoc / check | 5.977 | 5.695 | -4.71% | 389.5 / 389.4 |
| mdtoc / clif | 20.514 | 19.276 | -6.04% | 685.5 / 681.3 |
| mdtoc / llvm-o2 | 24.395 | 23.179 | -4.99% | 691.9 / 689.6 |
| bench-lite / check | 12.601 | 11.846 | -5.99% | 535.7 / 534.2 |
| bench-lite / clif | 26.558 | 25.281 | -4.81% | 688.9 / 686.9 |
| bench-lite / llvm-o2 | 32.126 | 30.877 | -3.89% | 695.9 / 694.0 |
| syn3k / check | 30.307 | 28.156 | -7.10% | 714.1 / 712.6 |
| syn3k / clif | 246.016 | 233.305 | -5.17% | 1165.7 / 1164.8 |
| syn3k / llvm-o2 | 356.448 | 344.450 | -3.37% | 1310.8 / 1308.5 |

The comparison reports zero instruction regressions in 24 metrics. Python
compiler-bench tests: 11 -> 13; Buck2 checked-profile tests: 2 -> 2, both passing.
Canonical TIR and native artifacts match across all eight programs and both
backends. One initial composite check reported a syn3k LLVM mismatch; two
baseline rebuilds and a focused release/dist/PGO rerun all produced the identical
ELF SHA-256 a539b051b5170417203926a3d59a2a338e4701183bad63acc1bcd9fbcd380b90,
with empty diagnostics. The failure did not reproduce; no compiler fix is inferred.
The comparison harness now retains diagnostics and each artifact hash separately.

Borrowed from oxc-project/tsrs notes/perf-pgo.md:14-32 (fat LTO, one codegen unit).
Differs because Buck2 target flags and a dedicated 1-CPU/4-GiB final-link reservation
replace Cargo's build plan. Developer compile reservations remain unchanged.

## PGO on top of fat LTO

PGO improves every measured case by another 8.44–18.80% instructions,
geometric mean -13.51%. Combined with LTO, the reduction against release is
12.56–22.68%, geometric mean -18.01%. Combined peak RSS ranges from -1.55% to
+0.04%, geometric mean -0.80%; no material memory regression. Final compiler
size is 49,153,128 bytes, with symbols retained. Both comparisons report zero
instruction regressions across all 24 metrics.

| Program / mode | Release / dist / PGO instructions (G) | PGO vs release | Release / dist / PGO RSS (MiB) |
| --- | ---: | ---: | ---: |
| hello / check | 5.636 / 5.383 / 4.929 | -12.56% | 386.2 / 386.0 / 385.0 |
| hello / clif | 10.665 / 9.969 / 8.921 | -16.36% | 631.6 / 629.0 / 623.4 |
| hello / llvm-o2 | 10.498 / 9.803 / 8.755 | -16.60% | 633.0 / 631.0 / 627.6 |
| todo-api / check | 14.540 / 13.602 / 11.949 | -17.82% | 579.1 / 576.1 / 574.5 |
| todo-api / clif | 118.233 / 111.451 / 91.677 | -22.46% | 885.9 / 883.3 / 878.4 |
| todo-api / llvm-o2 | 163.405 / 156.807 / 137.443 | -15.89% | 934.5 / 956.5 / 927.3 |
| kvstore / check | 13.906 / 13.039 / 11.485 | -17.41% | 544.0 / 542.8 / 541.9 |
| kvstore / clif | 99.988 / 94.122 / 77.422 | -22.57% | 851.5 / 850.8 / 845.8 |
| kvstore / llvm-o2 | 134.929 / 129.142 / 112.820 | -16.39% | 912.2 / 906.5 / 904.0 |
| logq / check | 13.819 / 12.950 / 11.379 | -17.66% | 544.6 / 543.1 / 542.4 |
| logq / clif | 43.406 / 41.498 / 33.695 | -22.37% | 718.7 / 717.1 / 710.0 |
| logq / llvm-o2 | 55.152 / 53.311 / 45.645 | -17.24% | 739.7 / 736.5 / 732.1 |
| csvjson / check | 7.708 / 7.262 / 6.531 | -15.26% | 413.4 / 407.5 / 412.2 |
| csvjson / clif | 58.596 / 55.595 / 45.661 | -22.08% | 758.1 / 753.9 / 746.7 |
| csvjson / llvm-o2 | 78.510 / 75.615 / 65.872 | -16.10% | 784.5 / 782.3 / 773.2 |
| mdtoc / check | 5.977 / 5.695 / 5.196 | -13.06% | 389.5 / 389.4 / 389.6 |
| mdtoc / clif | 20.514 / 19.276 / 16.279 | -20.64% | 685.5 / 681.3 / 674.8 |
| mdtoc / llvm-o2 | 24.395 / 23.179 / 20.227 | -17.09% | 691.9 / 689.6 / 686.1 |
| bench-lite / check | 12.601 / 11.846 / 10.486 | -16.79% | 535.7 / 534.2 / 533.2 |
| bench-lite / clif | 26.558 / 25.281 / 21.119 | -20.48% | 688.9 / 686.9 / 680.3 |
| bench-lite / llvm-o2 | 32.126 / 30.877 / 26.761 | -16.70% | 695.9 / 694.0 / 689.7 |
| syn3k / check | 30.307 / 28.156 / 24.331 | -19.72% | 714.1 / 712.6 / 711.2 |
| syn3k / clif | 246.016 / 233.305 / 190.211 | -22.68% | 1165.7 / 1164.8 / 1159.4 |
| syn3k / llvm-o2 | 356.448 / 344.450 / 302.416 | -15.16% | 1310.8 / 1308.5 / 1304.3 |

`tools/toolchain-dist/build --pgo` is the reproducible distribution pipeline;
`compiler-bench/run-main` uses it. It trains the instrumented compiler on all
24 quick workloads, requires a nonempty profile from each, merges with matching
checksum-pinned LLVM tools, then builds with profile-use. Both passes retain the
same platform hash bfbd7803d9312439, crate metadata, reservations, fat LTO and
strip setting. A declared, ignored profile artifact participates in Buck2 remote
action keys. Mimalloc's C code is not instrumented. Build manifests retain the
compiler/profile/precheck hashes. Generated profiles are never committed.

All 24 training workloads passed (peak 1265 MiB); all 64 baseline/PGO correctness
invocations passed, covering diagnostics, exported TIR and native ELF bytes.
Captured PGO-use diagnostics contain zero profile hash mismatches. Audited CLI
argument files confirm identical metadata between generate/use and the final
profile-use flag. Startup usage probes direct LLVM's output to /dev/null rather
than leaving a stray profile in the repository.

Borrowed from tsrs .github/scripts/pgo-train.sh:1-58 and notes/perf-pgo.md:128-169:
instrument/train/merge/use, nonempty workload profiles, matching symbol metadata.
Differs because Buck2 artifacts and configuration hashes replace Cargo target
folders. Training is CPU-selectable with explicit lock-wait/acquired progress;
CPU31 was occupied by another lane, so this experiment used CPU29.

## BOLT: declined

LLVM BOLT 23.1.1 was tried on a PGO compiler linked with --emit-relocs.
`llvm-bolt hirsel -instrument -o hirsel.inst
--instrumentation-file=profiles/hirsel --instrumentation-file-append-pid`
reached the tools/capped 4 GiB memory limit; the kernel killed it after 27.1 s,
exit 137, before training. The cap was not raised and no BOLT candidate exists,
so there is no speed claim or BOLT shipping step. Relocation flags are not
retained. Prior art: tsrs .github/scripts/bolt.sh:36-75 and
notes/perf-build-level.md:66-115. No rejected CPU/huge-page/allocator knobs were retried.

This changes the cost of executing compiler work; it adds no resident engine,
query cache or native ABI. The architectural work remains with its existing lanes.
