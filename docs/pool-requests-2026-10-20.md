# Pool requests, 2026-10-20

Evidence window: 2026-10-19T21:42:39+00:00 to 2026-10-20T21:42:39+00:00.

Policy: maximum measured PSS × 1.2; 64 MiB rounding; 128 MiB measured floor.
Tests use peaks at the new CPU count when observed, otherwise the largest available peak.
CPU is total CPU seconds / total wall seconds, with one CPU through 1.6 cores and two above.
There is no fresh comparative critical-path calibration establishing a four-CPU win, so no generated request keeps four CPUs.
An unobserved profile inherits measured sibling memory, with one CPU until parallelism is measured in that profile. Entirely unmeasured targets use 256 MiB; no old calibration pins a request.

Guard limits include virtual stacks and are independent of resident reservations. Completed retries do not preserve geometric escalation overshoot.

All requests below use two CPUs. The table lists every generated request above one CPU; wall is mean execution wall time, excluding queue.

| Kind/profile | Target | Measured cores | Mean wall (s) | Samples |
|---|---|---:|---:|---:|
| compile/dev | `hirsel-authority-unittest-harness` | 1.61 | 61.07 | 197 |
| compile/dev | `hirsel-check` | 1.98 | 5.02 | 412 |
| compile/dev | `hirsel-check-unittest-harness` | 2.81 | 24.80 | 432 |
| compile/dev | `hirsel-cli-test-cli-harness` | 1.62 | 26.59 | 481 |
| compile/dev | `hirsel-conformance-test-it-harness` | 3.20 | 7.73 | 468 |
| compile/dev | `hirsel-conformance-unittest-harness` | 3.52 | 16.02 | 471 |
| compile/dev | `hirsel-driver` | 2.19 | 10.48 | 3121 |
| compile/dev | `hirsel-driver-example-corpus_dump` | 1.92 | 3.96 | 444 |
| compile/dev | `hirsel-driver-test-it-harness` | 2.98 | 72.86 | 455 |
| compile/dev | `hirsel-driver-test-it-stdlib-harness` | 1.67 | 44.52 | 486 |
| compile/dev | `hirsel-driver-unittest-harness` | 1.62 | 55.99 | 456 |
| compile/dev | `hirsel-engine` | 3.48 | 8.53 | 2773 |
| compile/dev | `hirsel-engine-unittest-harness` | 1.91 | 32.79 | 425 |
| compile/dev | `hirsel-heapcheck-test-it-harness` | 1.71 | 67.84 | 69 |
| compile/dev | `hirsel-interp-test-it-harness` | 1.66 | 33.05 | 169 |
| compile/dev | `hirsel-kernels-unittest-harness` | 1.63 | 3.63 | 4 |
| compile/dev | `hirsel-lir` | 3.26 | 8.18 | 367 |
| compile/dev | `hirsel-lir-test-it-harness` | 3.98 | 29.39 | 99 |
| compile/dev | `hirsel-lir-unittest-harness` | 2.24 | 10.80 | 100 |
| compile/dev | `hirsel-llvm-test-it-harness` | 1.69 | 49.13 | 167 |
| compile/dev | `hirsel-lower-unittest-harness` | 1.66 | 36.78 | 159 |
| compile/dev | `hirsel-modes` | 1.61 | 11.25 | 508 |
| compile/dev | `hirsel-modes-unittest-harness` | 2.81 | 56.66 | 275 |
| compile/dev | `hirsel-ownership-unittest-harness` | 1.60 | 59.65 | 204 |
| compile/dev | `hirsel-pipeline-tests-test-front-harness` | 2.86 | 48.37 | 428 |
| compile/dev | `hirsel-pipeline-tests-test-rc-harness` | 2.98 | 70.84 | 439 |
| compile/dev | `hirsel-rc` | 3.78 | 9.07 | 730 |
| compile/dev | `hirsel-rc-unittest-harness` | 2.11 | 11.53 | 139 |
| compile/dev | `hirsel-resolve-unittest-harness` | 1.62 | 18.16 | 64 |
| compile/dev | `hirsel-rt-test-it-harness` | 1.63 | 46.46 | 205 |
| compile/dev | `hirsel-rt-unittest-harness` | 1.70 | 54.74 | 228 |
| compile/dev | `hirsel-run-config-test-it-harness` | 1.60 | 16.45 | 36 |
| compile/dev | `hirsel-signature-unittest-harness` | 1.62 | 19.65 | 122 |
| compile/dev | `hirsel-types` | 2.20 | 5.63 | 359 |
| compile/dev | `hirsel-types-unittest-harness` | 2.80 | 26.55 | 115 |
| compile/dev | `hirsel-validate` | 2.67 | 14.67 | 525 |
| compile/dev | `hirsel-validate-test-it-harness` | 2.62 | 22.27 | 88 |
| compile/dev | `hirsel-validate-unittest-harness` | 4.24 | 33.74 | 90 |
| compile/dev | `third-party:cranelift-codegen-meta-0.136` | 1.64 | 15.02 | 1 |
| compile/dev | `third-party:cranelift-isle-0.136` | 1.63 | 8.74 | 1 |
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
| compile/release | `hirsel-rt-archives-memcheck` | 1.65 | 19.20 | 274 |
| compile/release | `hirsel-rt-archives-system-allocator` | 1.69 | 19.67 | 548 |
| compile/release | `hirsel-run-config` | 1.64 | 6.05 | 137 |
| compile/release | `hirsel-signature` | 1.72 | 9.10 | 20 |
| compile/release | `hirsel-types` | 1.72 | 14.88 | 16 |
| compile/release | `hirsel-validate` | 2.29 | 36.23 | 32 |
| test | `gate-agree-full` | 2.68 | 1595.5 | 156 |
| test | `gate-native-bugs` | 1.71 | 3.7 | 2 |
| test | `gate-native-corpus` | 1.96 | 717.2 | 35 |
| test | `gate-native-smoke` | 1.95 | 695.4 | 73 |
| test | `gate-native-trace-roots` | 1.89 | 331.5 | 184 |
| test | `hirsel-cli-test-cli` | 1.91 | 430.7 | 230 |
| test | `hirsel-cli-test-free` | 1.86 | 350.9 | 7 |
| test | `hirsel-conformance-unittest` | 2.06 | 5.2 | 123 |
| test | `hirsel-driver-test-it-scaling` | 1.85 | 36.3 | 140 |
| test | `hirsel-driver-test-it-stdlib` | 1.64 | 51.0 | 2540 |
| test | `hirsel-llvm-test-it` | 1.65 | 11.7 | 96 |
| test | `hirsel-pipeline-tests-test-rc` | 1.86 | 20.3 | 1071 |

Special actions:

| Action | CPUs | Request (MiB) | Peak PSS (MiB) | Samples |
|---|---:|---:|---:|---:|
| clippy | 1 | 640 | 506.4 | 23644 |
| native_observation | 1 | 1600 | 1309.9 | 321 |
| precheck | 1 | 4416 | 3640.4 | 656 |

Coverage: 345 compile/profile rows from 47,027 unique executions; 363 unmatched retained command records.
Fresh direct test evidence: 100/108 binary rows. Unobserved native stages explicitly inherit a measured sibling; obsolete or unobserved non-native rows keep their previous evidence.

Work reduction: the fast main build excludes campaign and host-only dependencies, as land does, and stays at priority 40. Existing prelude transforms already assemble dependency symlink trees in the daemon; fresh logs contain no remote deps helper actions and no rustc actions owned by Clippy twins. Clippy already shares normal dependencies, so no additional rebuild workaround is needed.

Fresh case-cost tables rebalance workspace shards; the full agreement campaign uses 384 two-CPU shards rather than 96 four-CPU shards. No lane count changes.

The CLI binary now has 16 cost-balanced shards from 141 measured passing cases (1212.8 test-seconds). Its 145-second longest case remains a single case; shard count does not split test bodies. The two unchanged timing-sidecar failures are registered in known-failures.tsv.
