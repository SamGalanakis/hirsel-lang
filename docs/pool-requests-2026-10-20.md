# Pool requests, 2026-10-20

Evidence window: 2026-10-19T21:16:54+00:00 to 2026-10-20T21:16:54+00:00.

Policy: maximum measured PSS × 1.2; 64 MiB rounding; 128 MiB measured floor.
Tests use peaks at the new CPU count when observed, otherwise the largest available peak.
CPU is total CPU seconds / total wall seconds, with one CPU through 1.6 cores and two above.
There is no fresh comparative critical-path calibration establishing a four-CPU win, so no generated request keeps four CPUs.
The 256 MiB default covers unmeasured compiles; no old calibration pins a request.

Guard limits include virtual stacks and are independent of resident reservations. Completed retries do not preserve geometric escalation overshoot.

All requests below use two CPUs. The table lists every generated request above one CPU; wall is mean execution wall time, excluding queue.

| Kind/profile | Target | Measured cores | Mean wall (s) | Samples |
|---|---|---:|---:|---:|
| compile/dev | `hirsel-authority-unittest-harness` | 1.61 | 61.15 | 188 |
| compile/dev | `hirsel-check` | 1.98 | 5.00 | 415 |
| compile/dev | `hirsel-check-unittest-harness` | 2.81 | 24.66 | 426 |
| compile/dev | `hirsel-cli-test-cli-harness` | 1.62 | 26.53 | 475 |
| compile/dev | `hirsel-conformance-test-it-harness` | 3.19 | 7.73 | 462 |
| compile/dev | `hirsel-conformance-unittest-harness` | 3.53 | 15.99 | 465 |
| compile/dev | `hirsel-driver` | 2.19 | 10.41 | 3079 |
| compile/dev | `hirsel-driver-example-corpus_dump` | 1.92 | 3.96 | 437 |
| compile/dev | `hirsel-driver-test-it-harness` | 2.98 | 72.70 | 449 |
| compile/dev | `hirsel-driver-test-it-stdlib-harness` | 1.67 | 44.31 | 480 |
| compile/dev | `hirsel-driver-unittest-harness` | 1.62 | 55.63 | 451 |
| compile/dev | `hirsel-engine` | 3.48 | 8.49 | 2724 |
| compile/dev | `hirsel-engine-unittest-harness` | 1.91 | 32.83 | 419 |
| compile/dev | `hirsel-heapcheck-test-it-harness` | 1.71 | 67.66 | 67 |
| compile/dev | `hirsel-interp-test-it-harness` | 1.66 | 33.20 | 166 |
| compile/dev | `hirsel-kernels-unittest-harness` | 1.63 | 3.63 | 4 |
| compile/dev | `hirsel-lir` | 3.28 | 8.09 | 367 |
| compile/dev | `hirsel-lir-test-it-harness` | 3.98 | 29.27 | 101 |
| compile/dev | `hirsel-lir-unittest-harness` | 2.24 | 10.82 | 101 |
| compile/dev | `hirsel-llvm-test-it-harness` | 1.69 | 49.14 | 166 |
| compile/dev | `hirsel-lower-unittest-harness` | 1.66 | 36.97 | 161 |
| compile/dev | `hirsel-modes` | 1.61 | 11.29 | 506 |
| compile/dev | `hirsel-modes-unittest-harness` | 2.81 | 56.59 | 279 |
| compile/dev | `hirsel-pipeline-tests-test-front-harness` | 2.85 | 47.89 | 421 |
| compile/dev | `hirsel-pipeline-tests-test-rc-harness` | 2.98 | 70.72 | 433 |
| compile/dev | `hirsel-rc` | 3.78 | 8.98 | 723 |
| compile/dev | `hirsel-rc-unittest-harness` | 2.10 | 11.56 | 137 |
| compile/dev | `hirsel-resolve-unittest-harness` | 1.62 | 18.16 | 64 |
| compile/dev | `hirsel-rt-test-it-harness` | 1.63 | 46.64 | 203 |
| compile/dev | `hirsel-rt-unittest-harness` | 1.70 | 54.78 | 226 |
| compile/dev | `hirsel-run-config-test-it-harness` | 1.60 | 15.91 | 37 |
| compile/dev | `hirsel-signature-unittest-harness` | 1.62 | 19.71 | 122 |
| compile/dev | `hirsel-types` | 2.20 | 5.62 | 357 |
| compile/dev | `hirsel-types-unittest-harness` | 2.80 | 26.66 | 115 |
| compile/dev | `hirsel-validate` | 2.67 | 14.55 | 526 |
| compile/dev | `hirsel-validate-test-it-harness` | 2.62 | 22.42 | 89 |
| compile/dev | `hirsel-validate-unittest-harness` | 4.24 | 33.77 | 91 |
| compile/release | `hirsel-check` | 2.09 | 11.41 | 22 |
| compile/release | `hirsel-driver` | 1.60 | 55.94 | 37 |
| compile/release | `hirsel-engine` | 2.53 | 26.63 | 72 |
| compile/release | `hirsel-heapcheck` | 1.74 | 17.78 | 18 |
| compile/release | `hirsel-interp` | 2.17 | 20.70 | 42 |
| compile/release | `hirsel-lir` | 2.04 | 32.23 | 14 |
| compile/release | `hirsel-lower` | 1.75 | 11.33 | 16 |
| compile/release | `hirsel-modes` | 2.18 | 28.05 | 31 |
| compile/release | `hirsel-proof-export` | 1.65 | 8.68 | 32 |
| compile/release | `hirsel-rc` | 2.14 | 27.60 | 26 |
| compile/release | `hirsel-rt` | 1.79 | 24.50 | 40 |
| compile/release | `hirsel-rt-archives-memcheck` | 1.62 | 19.35 | 267 |
| compile/release | `hirsel-rt-archives-system-allocator` | 1.67 | 19.69 | 534 |
| compile/release | `hirsel-run-config` | 1.65 | 5.92 | 141 |
| compile/release | `hirsel-signature` | 1.72 | 9.10 | 20 |
| compile/release | `hirsel-types` | 1.72 | 14.88 | 16 |
| compile/release | `hirsel-validate` | 2.29 | 36.23 | 32 |
| test | `gate-agree-full` | 2.67 | 1579.6 | 133 |
| test | `gate-native-bugs` | 1.71 | 3.7 | 2 |
| test | `gate-native-corpus` | 1.95 | 715.2 | 34 |
| test | `gate-native-cranelift-changed` | 1.61 | 21.8 | 343 |
| test | `gate-native-smoke` | 1.96 | 689.1 | 74 |
| test | `gate-native-trace-roots` | 1.89 | 331.5 | 184 |
| test | `hirsel-cli-test-cli` | 1.9 | 429.5 | 235 |
| test | `hirsel-cli-test-free` | 1.86 | 350.9 | 7 |
| test | `hirsel-conformance-unittest` | 2.05 | 5.2 | 125 |
| test | `hirsel-driver-test-it-scaling` | 1.85 | 35.1 | 147 |
| test | `hirsel-driver-test-it-stdlib` | 1.65 | 51.0 | 2554 |
| test | `hirsel-llvm-test-it` | 1.67 | 12.0 | 94 |
| test | `hirsel-pipeline-tests-test-rc` | 1.86 | 20.3 | 1077 |

Special actions:

| Action | CPUs | Request (MiB) | Peak PSS (MiB) | Samples |
|---|---:|---:|---:|---:|
| clippy | 1 | 640 | 505.0 | 23344 |
| native_observation | 1 | 1600 | 1309.9 | 321 |
| precheck | 1 | 4416 | 3640.4 | 657 |

Coverage: 213 compile/profile rows from 45,954 unique executions; 515 unmatched retained command records.
Fresh direct test evidence: 100/108 binary rows. Unobserved native stages explicitly inherit a measured sibling; obsolete or unobserved non-native rows keep their previous evidence.

Work reduction: the fast main build excludes campaign and host-only dependencies, as land does, and stays at priority 40. Existing prelude transforms already assemble dependency symlink trees in the daemon; fresh logs contain no remote deps helper actions and no rustc actions owned by Clippy twins. Clippy already shares normal dependencies, so no additional rebuild workaround is needed.

Fresh case-cost tables rebalance workspace shards; the full agreement campaign uses 384 two-CPU shards rather than 96 four-CPU shards. No lane count changes.
