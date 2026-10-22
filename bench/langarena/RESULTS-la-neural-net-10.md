# NeuralNet access overhead — la-neural-net-10

The scalar access change reduces whole-process retired instructions by **0.773%**
(user instructions: **0.775%**) in the primary ten-round interleaved session.
Allocations, reference counting and runtime copied bytes are unchanged. This is
an instruction reduction, with **no demonstrated wall-clock speedup**: the
candidate's median timed work was 3.115 s versus 3.034 s before on this shared host.

The baseline is current main `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`, captured
before implementation. Historical RESULTS.md ratios are not this baseline.
Only `Etc::NeuralNet` was selected. The fixed 2–10–1 topology, seed 42, 43 PRNG
draws, 1000 epochs per iteration, training order 00/10/01/11, synapse ordering,
momentum and checksum order remain unchanged.

## Change and ordering proof

`feed` and hidden-error accumulation read the required synapse scalars directly.
`update` snapshots the neuron's error, then each synapse's weight, previous weight,
source index and source output before either write. It stores the new weight,
then the old weight into previous weight. Threshold scalars are read after the
synapse pass. The error is immutable throughout that pass; source outputs are
unchanged by weight updates. No snapshot survives a training update.

`weight += delta` becomes `weight = weight + (delta)` with the same grouping;
the two multiplications within momentum, learning-rate chain and ordered sums
retain their original expression trees. There is no reassociation, explicit FMA,
fastmath, new compiler optimization, ABI change or alternative training method.
The existing adjacency lists and flattened arena mapping are retained.

## Exact state and checksum evidence

The new auto-discovered Cargo/Buck CLI integration harness is
`crates/hirsel-cli/tests/la_neural_net_10.rs`; its explicit `#[path]` registers `support/la_neural_net_10/transitions.rs`. This
standalone harness retains ordinary Cargo/Buck discovery; no shared main.rs was
edited. It includes the frozen
before kernel and the complete pinned upstream Rust implementation, with its MIT
notice in `support/la_neural_net_10/LICENSE`.

The H control observes all 112 finite IEEE words (13 neurons × threshold,
previous threshold, output, error; 30 synapses × weight, previous weight) at
initialization and after 1, 2, 3, 4 and 8 updates. Every before/after bit agrees.
Rust agrees exactly except three already-existing propagated libm differences:
update 2, fields 38/39 (global neuron 9 output/error), and update 8, field 14
(neuron 3 output). Their bit-pattern distances are 2, 2 and 1. The test permits
only those fields and at most two bit-pattern units, with exact comparisons
elsewhere. It does not apply a blanket numerical tolerance. Hirsel's existing
`stdlib/std@1/f64_exp.hir:32` documents the fdlibm exp error below one ULP;
Rust's system exp can round differently. Initialization and the first complete
update match Rust exactly. No Hirsel semantic tolerance was changed.

The native control compares before/after full state after 1000, 2000 and 3000
epochs on both LLVM release and Cranelift debug. It also requires cross-backend
state equality and the independent Rust checksums:

| Epochs | Checksum |
| --- | ---: |
| 1000 | 1584301501 |
| 2000 | 1585701034 |
| 3000 | 1586674334 |

The last row is test.js's two measured iterations after one retained warmup.
Separate retained native diagnostics also observe initialization, every first
four updates, update 8 and those three epoch boundaries on both emitters:
all 1008 before/after state words agree exactly per backend, and both backends
agree. Against system Rust exp, the maximum absolute difference after 3000
epochs is 1.244e-14; this describes the pre-existing libm difference and is not an
acceptance tolerance. The unchanged full-size run.js oracle is **144809112**.
All 30 primary cells pass it; no zero-selection result was accepted.

Final scoped verification:

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test root//:hirsel-cli-test-la_neural_net_10 -- --test-arg=transitions::
```

The final harness executes **2 tests, 2 passed, 0 filtered**. It executes three
before programs and three after programs: H, LLVM and Cranelift. Before this
change the lane-private harness had 0 tests. A prior one-control green and the
two-control green before the test-only PRNG mutex are retained separately. The
mutex protects the shared upstream PRNG when the Rust test runner overlaps tests.

## Primary instruction experiment

`counts.json` is the first accepted corrected experiment, not replaced by later
runner validation. Ten rounds, seed **20261022**, shuffled before/after/Rust order,
CPU **10** (AMD Ryzen 9 5950X, SMT sibling 26). The ordinary CPU10 convention lock
was acquired immediately and held throughout; CPU31 was never used. No other CPU
lock was held. Each process uses nice 10, taskset and unchanged tools/capped limits.

| Median of 10 | Before | After | Pinned Rust |
| --- | ---: | ---: | ---: |
| Instructions, user + kernel | 40,087,394,661 | 39,777,498,800 | 22,155,360,776.5 |
| Instructions, user only | 40,017,879,158 | 39,707,798,656 | 22,135,432,696.5 |
| Cycles, user + kernel | 13,963,456,870 | 14,326,951,113 | 6,241,994,191.5 |
| Harness timed seconds | 3.034157 | 3.114845 | 1.353000 |
| Whole-process peak RSS, KiB | 6272 | 6272 | 3584 |

All four hardware events have **100% PMU running coverage in all 30 cells**.
Counters cover the inherited benchmark process tree, startup, parsing, preparation,
340 warmup iterations, 1700 timed iterations and checksum; compiler/build processes
are excluded. The harness's time covers only timed work. Full /proc/stat snapshots
retain SMT activity. Host scheduling and sibling work remain uncontrolled: cycles
and times are supporting observations from this session, without cross-host
projection, published-language placement or speedup claim.

A second campaign, `repro-session/measurements.json`, exists solely to validate the
new checked-in three-participant runner. It retains separate identities and raw
output: 3 test.js correctness cells and 30 full-size cells pass, with frozen-input
verification and 100% PMU coverage. Its user-instruction medians are
40,017,861,217 / 39,707,781,450.5 / 22,135,416,886. It does not replace or pool with
the primary evidence. The standard compare.py candidate/reference command, already running before the
coordination stop on additional campaigns, also completed: 20/20 full-size cells
pass, with input verification and PMU coverage retained. Its separate report and
full command log are `canonical-report.md` and `/tmp/la-neural-net-10-compare.log`;
`canonical-raw/` preserves the worktree outputs. It used the assigned filter and
10 rounds with `--cpu 10`, the frozen compiler via HIRSEL, a private --tools
directory and a private --output report. It provides no additional before/after
change claim. No campaign or control was repeated because main moved.

## Cost attribution and allocations

A separate baseline cycles:u diagnostic profile, outside timing samples, attributes
33.75% of sampled cycles to exp, 26.54% to update, 19.49% to feed and 17.51% to train.
These include their emitted access instructions; they are not isolated causal cost
estimates. Checksum/preparation/runtime allocation are below the useful sampling
resolution. No singleton profile is used as speedup evidence.

Separate instrumented LLVM binaries use the same frozen packages/compiler/runtime
and regions-on-whole configuration, with the measurement runtime archive. Their
full-size oracles pass. Both report exactly:

| Managed runtime counter | Before | After |
| --- | ---: | ---: |
| Physical allocations / frees | 410 / 410 | 410 / 410 |
| Reference-count increments | 261123359 | 261123359 |
| Reference-count decrements | 261123696 | 261123696 |
| COW detaches / deep copies | 1 / 1 | 1 / 1 |
| Copied blocks / bytes | 29 / 3632 | 29 / 3632 |
| Peak live managed bytes | 37472 | 37472 |
| Live managed bytes at exit | 0 | 0 |

These counters cover Hirsel managed objects, including configuration, initialization
and checksum, excluding compiler memory, host allocators and native dependency
arenas. They do not count scalar register/record copies or field stores. The
checksum's explicit `unique s` clone is retained; no training allocation or cloning
reduction is claimed. The Rust anchor's separate glibc interposer observes 1127
successful allocation calls and 108020 requested bytes, including process startup
and checksum. Its coverage boundary differs from Hirsel's and is not an allocation
ratio. Instrumented binaries are excluded from performance samples.

The next concrete route is to measure scalar adjacency metadata for this fixed
fully-connected topology against the 261 million unchanged reference-count
operations, preserving source-major synapse numbering and destination sums. Exp
alone cannot explain the remaining access cost; changing libm is out of scope.

## Reproduction, provenance and retained attempts

The checked-in `la-neural-net-10.py` interleaves three frozen binaries, validates
both original configurations, records exact argv/cwd, raw stdout, perf coverage,
RSS, CPU/SMT snapshots and all supplied input identities, then verifies those
identities after each round. `--help` documents its binary/grants/root arguments.
Use `--identity` for frozen package sources, std, compiler, native helper, LLVM,
link kit, glibc floor and runtime archives; use a fresh `--output` directory. A
busy timing lock fails immediately; `--instructions-only` supports an ordinary
free CPU without wall-ratio evidence. The CPU31 guard is intentional.

The before package is built from `crates/hirsel-cli/tests/support/la_neural_net_10/before.hir`, the after
package from current `bench/langarena/neural_net.hir`. Both use verbatim
main.hir/harness.hir/run.js/test.js and a private NeuralNet-only registry, a copied
stdlib manifest tree, and identical native inputs. Their prepare, warmup, timed
step and checksum boundaries are unchanged. Use `hirsel build --release` for both;
the test harness independently generates reduced observation packages.

All local evidence is retained in `/workspace/code/hirsel-briefs/la-neural-net-10/`:
source packages, manifests/grants, compiler/helper/native snapshots, binaries,
full-state vectors, primary counts/raw output, separate runner validation,
allocation reports and scripts. Input trees include consumed tracked/untracked
files and are SHA-256 sealed before consumers and verified afterwards. Buck
check source manifests and post-consumer verifications are retained separately.
The final landed source may rebase; measured binaries stay pinned to the captured
baseline and candidate inputs.

| Frozen input | SHA-256 |
| --- | --- |
| Before neural_net.hir | `902b4b9ab89f3134f83943975b4ffcb6d170104f6254907e8e41e9f58695f791` |
| After neural_net.hir | `beded7a460b94d4172d3bc025a78b4d90f777010c421c11b95885ff27f6131fc` |
| Before executable | `d770b5fb7c93487891c2caa2ade6f999e8ed59960f07e55ff4317e150852006c` |
| After executable | `3072c98a2eb5bc0188455eed17fc64e9e66b6b56da54b941add0858596ca225f` |
| Rust executable | `468817173ea176c7c80ee1ddd3e4b0233eecf53a209419de392ec080e0f39f4f` |
| Compiler | `90dfbd42bf3380d27041c1634e769a128e4bf0f579a9b98adc937071f08715e9` |
| Upstream neural_net.rs | `1d61f96f0b0b9f87d59123d01e47db973fd1a4c5504f9fa32d97dab608ce0e56` |

Upstream is LangArena **3c9f84ddaeec834c0631747cb789e8cfd602f993**,
`rust/src/benchmarks/neural_net.rs:10–164,184–222`, `test.js:103–107`.
Its source identity was checked at launch, not followed from HEAD. test.js SHA-256
is `424371dd25f3fa3f779789c823a05dbe90d2eddbc2a3ca3c5e4319252c572797`.
Copyright (c) 2026 Konstantin Makarchev, MIT; the complete license is retained in
bench/langarena/LICENSE and beside the copied Rust fixture. The glibc diagnostic
interposer follows the prior la-astar-9 local allocation diagnostic.

Hirsel adaptations are the existing flattened global neuron indices/u64 List
indices, mut parameters, source std fdlibm exp, and the existing checksum clone
and float-text hash. This change adds scalar snapshots/direct stores only. The
Rust vector wrapper adds observation, flattens layer state for comparison,
serializes its shared PRNG in tests, and retains the upstream scalar PRNG/hash;
the upstream neural-net file itself is unmodified. No training-vector deviation
or checksum tolerance was introduced.

Preserved failures/setup attempts: fixture type `Out` (0/1 test), missing grants
`dirs` (0/1), text-formatted snapshots exceeding the unchanged one-million-step H
budget (0/1; replaced with binary IEEE output), baseline absolute config path
outside granted virtual root (one failed setup invocation), and the first perf
parser attempt missing task-clock (one checksum-green sample, no accepted
performance result). Each has its own log/artifact; none was relabeled green,
erased, pooled into accepted samples or hidden by a budget increase.

The first landing setup rejected a private modules.rs matching the shared
registry check. An attempted main.rs wrapper then caused duplicate Cargo test
discovery. Both setup reds are retained as land-attempt-1.log and
land-attempt-2.log. A third attempt compiled the workspace but was explicitly
cancelled (EXIT143) before lint completed to adopt the coordinator-specified
standalone layout: private helpers under tests/support/la_neural_net_10 and an
explicit #[path] in the auto-discovered root test. Every moved helper is byte
identical to the tested source except the required relative benchmark include
paths; helper-move-verification.json proves the normalized identities. Ordinary
landing checks compile the final layout. The completed semantic controls and
accepted campaigns are retained without reruns for this helper movement.
