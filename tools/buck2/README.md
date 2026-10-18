# Buck2 build

The workspace builds and tests with Buck2, on this host or on an optional
remote-execution pool (for example NativeLink, with a shared content-addressed
cache). Cargo manifests and `Cargo.lock` stay the source of truth: `sync.py`
derives every Buck2 build file from them, and `cargo build` keeps working.

## Setup per checkout or worktree

```sh
python3 tools/buck2/prepare.py        # bootstrap.py, then the remote-execution hook
```

`bootstrap.py` installs the official Buck2 release pinned in `pins.json` at
`.buck2/bin/buck2`, its bundled prelude with the overlay below at
`.buck2/prelude`, the Rust toolchain in `toolchain-lock.json` at
`toolchains/rust-files/`, the LLVM tree at `toolchains/llvm-files/`
(`bootstrap_llvm.py`) and Reindeer at `bin/reindeer`. Every download is
checked against its pinned SHA-256. Inputs are shared between checkouts
through the store in `bootstrap_store.py` (`$HIRSEL_BUCK2_STORE`, default
`~/.cache/hirsel-buck2`; `off` disables it).

A pool's endpoint and credentials never enter the repository. `prepare.py`
runs an optional hook, the executable named by `$HIRSEL_BUCK2_REMOTE_CONFIG`
or `~/.config/hirsel/buck2-remote-config`, with the checkout's root. The hook
writes the gitignored `.buckconfig.local` (only when its content changes):
the `[buck2_re_client]` endpoint, instance and TLS identity, and
`hirsel.remote_properties`, the `key=value,...` platform properties the
pool's workers match. Without a hook an existing `.buckconfig.local` is kept;
without one `prepare.py` writes `hirsel.execution = local`, which runs every
action on this host.

## Everyday commands

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-toolchain --show-output # CLI and sibling std precheck
.buck2/bin/buck2 build //:                        # every first-party target, dev profile, on the pool
.buck2/bin/buck2 test //...                       # every test
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir           # one crate, including doctests
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-rc-shard-2  # one shard of a sharded test binary
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-pipeline-tests-test-rc-shard-1   # run tests on this host
.buck2/bin/buck2 test //:gate-native-smoke          # one corpus campaign of tools/gate
python3 tools/buck2/conformance.py                # conformance shards and summed counts
python3 tools/buck2/agree.py                      # AGREE_LIMIT variants over AGREE_SHARDS shards, summed JSON
python3 tools/buck2/agree.py --native-full        # the native campaign's shards, merged against agree-known.txt
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-conformance-bin-native
python3 tools/buck2/sync.py                       # after metadata or action-sizes.json changes
tools/gate                                           # the per-push gate (tools/land lands)
```

`tools/land` rebuilds after its rebase, then runs the built `hirsel doc stdlib/std@1/hirsel.json`.
It commits a changed `docs/api.md` separately as `Regenerate the API index.`
before pushing; this generated-only commit needs no new gate. Push retries
regenerate after the next rebase. The gate retains the export-count and drift
test, so source changes and their published signatures stay in agreement.

Builds use Buck2's default isolation dir, `v2`: never pass
`--isolation-dir` (or pass only `--isolation-dir v2`) and leave
`BUCK_ISOLATION_DIR` unset. Each worktree already has its own `v2` daemon.
The isolation dir is part of every action's output paths, so an action run
under another one has a different key and misses the pool's shared cache. A
private isolation dir is only for inspecting the graph beside a running
command, or for escaping a stuck daemon; anything it builds starts cold.

`-c hirsel.tests=focus` is for running one test or a few by name: each test
run books one CPU, with its binary's measured memory, instead of the binary's
whole-shard request. The pool
reserves nothing for a multi-CPU action, so a four-CPU run of one case waited
for one worker to free four CPUs at once: over twelve alternated edits its
queue had a median of 43 s and a maximum of 353 s, against 0.4 s and 6 s in
focus. Leave it off for whole crates and gate stages, which use the threads
their request buys; the request is in the test's action key, so a focus run
neither reuses nor replaces their cached verdicts.

Arguments after `--` go to Buck2's test runner: `--test-arg=<a>` passes one
libtest argument, and `--timeout <seconds>` raises the ten-minute limit per
test. A sharded binary given a case filter runs it all in shard 1. Check the output
for a nonzero case count. Every test runs on the pool by default, including
native and full-gate cases. `-c hirsel.tests=host` explicitly opts into host
execution; it still uses the pinned tool inputs. There are no implicit host
partitions. Target names come from Cargo; `-unittest` and `-test-it` run unit
and integration tests, and the library target runs declared doctests. For a
sharded binary, select every `-shard-<i>` target.

Heavy Buck2 commands run from this host and execute on the pool when one is
configured. Native changes use a filtered campaign:

```sh
.buck2/bin/buck2 test -c hirsel.full=true //:gate-native-full -- --test-arg=--filter --test-arg=SEM-4-view-return-source-run --timeout 14400
```

Native tests receive LLVM 21.1.8, Arm GNU 14.3.Rel1 with its Linux sysroot,
static AArch64 qemu and Valgrind as SHA-pinned Buck2 downloads. No worker
image changes, PATH lookup or binfmt registration are needed. The launcher
resolves `LLVM_SYS_211_PREFIX` and each variable of
`native-toolchain/environment.tsv` (`HIRSEL_AARCH64_CC`, `HIRSEL_AARCH64_SYSROOT`,
`HIRSEL_QEMU_AARCH64`, `HIRSEL_VALGRIND`, `VALGRIND_LIB`,
`HIRSEL_VALGRIND_RUNTIME`: a path inside one toolchain) to absolute input paths
before changing directory; `bench/native/suite.py` sets the same table. The driver's
clang launcher selects the input lld. Native launches preserve the selected
Valgrind support directory when clearing the child's environment.

The full gate's runtime C harness, library C probes and LLVM pin check are
`//:gate-native-runtime-c`, `//:gate-native-library-c` and
`//:gate-native-toolchain-pin` under `-c hirsel.full=true`. They run on the
pool too. Lean still runs on the invoking host. The standalone tool proof is
`//tools/buck2/native-toolchain:proof`; see
[the native toolchain](../native-toolchain/README.md).

Target names follow Cargo's targets, all in the root package:

| Cargo target | Buck2 target |
|---|---|
| library of `hirsel-x` | `//:hirsel-x` |
| its unit tests | `//:hirsel-x-unittest` |
| `tests/it` (or `tests/<name>.rs`) | `//:hirsel-x-test-it` (`//:hirsel-x-test-<name>`) |
| binary `<bin>` and its unit tests | `//:hirsel-x-bin-<bin>`, `//:hirsel-x-bin-<bin>-unittest` |
| example `<ex>` | `//:hirsel-x-example-<ex>` |
| build script | `//:hirsel-x-build-script` |
| a sharded test binary (`SHARDED` in `sync.py`) | `//:hirsel-x-test-it-shard-<i>` |
| a corpus campaign of `tools/gate` | `//:gate-<stage>` (label `gate`) |
| a conformance shard (`CONFORMANCE_SHARDS`) | `//:gate-conformance-shard-<i>` |
| an agreement shard (`AGREE_SHARDS`) | `//:gate-agree-shard-<i>` |
| a budgeted full agreement shard (`AGREE_FULL_SHARDS`, `--full` only) | `//:gate-agree-full-shard-<i>` |
| a budgeted native agreement shard (`AGREE_NATIVE_FULL_SHARDS`, `--full` only; `agree.py --native-full` runs and merges them) | `//:gate-agree-native-full-shard-<i>` |
| a full native matrix shard (`NATIVE_FULL_SHARDS`, `--full` only; `gate-native-full` stays whole for `--bless` and filtered runs) | `//:gate-native-full-shard-<i>` |
| a historical native replay shard (`HISTORY_SHARDS`; `tools/native-plants.py --history-only` runs them) | `//:gate-native-history-shard-<i>` |

## Blessing expectations

`tools/bless.sh <what>...` rewrites checked-in expectations from the code that
derives them; `tools/bless.sh` with no argument lists every `<what>` and the
files it writes. Review the diff it prints before committing.

```sh
tools/bless.sh heap                # native-tests/heap goldens
tools/bless.sh layout rc-import    # several at once
tools/bless.sh all                 # every in-test bless
tools/bless.sh passing             # conformance/passing.txt (adds only)
```

A test rewrites its expectation when it runs with `HIRSEL_BLESS=1` (insta
snapshots: `INSTA_UPDATE=always`). The script runs those cases on this host
(`-c hirsel.tests=host --env HIRSEL_BLESS=1`), where a test reads and writes
the checkout through its relative `CARGO_MANIFEST_DIR`. It fails unless a
case ran, then runs the same cases again without the bless, on the pool, which
fails unless the rewritten files now pass. `passing` runs
`hirsel-conformance --update-passing` with `buck2 run --root "$PWD"`, and `exclusions` runs
`native --full --bless` through `gate-native-full` with explicit host execution.

A pool action reads a copy of the checkout that is discarded afterwards, so a
bless there writes nothing that lasts: a read-only input fails with a bare
`PermissionDenied` and a writable one passes. `test_launcher.sh` therefore
refuses, with exit 2, a pool test run under `HIRSEL_BLESS`, a writing
`INSTA_UPDATE`, `--bless` or `--update-passing`.

A new expectation file gets its rewrite under `HIRSEL_BLESS` in the test that
checks it, and a row in `tools/bless.sh`. A test whose expectation is compiled
in with `include_str!` returns after writing, since it would compare against
the old copy.

## How the graph is made

- `sync.py` writes `third-party/Cargo.toml` (every external crate a member
  names, at its locked version with the features Cargo resolved for the
  workspace), `third-party/Cargo.lock` (from the workspace lock, so no version
  moves), `third-party/BUCK` (Reindeer, configured by
  `third-party/reindeer.toml` and `third-party/fixups/`) and the root `BUCK`.
  `sync.py --check` fails when any of them is stale: after a manifest or lock
  change, and after a source adds or drops a literal `include*!` or `#[path]`
  reference, or the corpus a manifest names (rerun `sync.py` and commit the
  result).
- Native and full-gate crate cases use the same pool execution path as ordinary
  tests. The runtime ABI symbol test reads the separately built production
  archive; the layering test reads `workspace-metadata.json`, generated from
  Cargo's workspace graph by `sync.py` and checked for freshness. Neither
  invokes Cargo on a worker.
- A pool test's passing verdict is cached by its action key. Test binaries
  whose sources draw random cases (`proptest!`, `SEEDED` in `sync.py`) run
  under `PROPTEST_RNG_SEED` from `-c hirsel.test_seed` (default 1), which is in
  that key: a cached pass covers its seed only. `tools/gate --full` draws a new
  seed each run and prints it; `HIRSEL_TEST_SEED=<n>` reproduces one.
- Features are Cargo's workspace-wide resolution (`cargo build --workspace
  --all-targets`): one feature set per crate, dev-dependency features included.
- All first-party targets are in the root package, so a compile action's source
  tree keeps repository paths and `include_str!("../../../conformance/x")`
  works unchanged. A target's sources are its directory (`src/`, `tests/it/`,
  ...) plus every file its literal `include*!` and `#[path]` references reach
  (and the files an `include_str!(concat!("<prefix>", $arg))` macro names
  through its file's string literals).
- Tests run as Cargo runs them, from their package directory. Every path
  Cargo compiles in absolute is relative to that directory instead (rustc's
  `--env-set`): `env!("CARGO_MANIFEST_DIR")` is `../../crates/<crate>`, which
  names the crate from any package directory, and so are `CARGO_TARGET_TMPDIR`
  and `CARGO_BIN_EXE_<name>`. Tests read only the files they declare as data
  (`RUNTIME_DATA` in `sync.py`).
- `CARGO_BIN_EXE_<name>` names `buck-out/tmp/cargo-bin-exe/<harness>/<name>`,
  and the launcher links that to the binary the test run carries. The
  harness compile does not depend on the package's binaries, so it neither
  waits for their links nor fetches them. A harness run outside the launcher
  does not find them.
- Executables link with zlib-compressed debug sections
  (`--compress-debug-sections=zlib`): half the bytes per whole-compiler
  binary, and std's backtraces still print file and line. zstd sections lose
  the locations.
- Test harnesses are `rustc --test` binaries; `test_launcher.sh` runs them
  with an existing `CARGO_TARGET_TMPDIR`, the pinned LLVM as
  `LLVM_SYS_211_PREFIX` (its `bin/` first on `PATH`, its `lib/` on the loader
  path), insta's workspace root, and test threads and `HIRSEL_JOBS` set to the
  action's CPU share, at most 4. It also runs one shard of a sharded binary,
  by exact names: cases go longest first to the least loaded shard, weighted
  by the binary's measured cost table. It asks the compiled harness to apply
  caller filters, then applies sharding.
  The `full_gate_ --ignored` group also emits the unpartitioned compiled case
  inventory. `full_gate_counts.py --run <command...>` streams Buck2's log and
  checks every `(binary, case)` appears exactly once, rejecting missing,
  duplicate, failed or unexpected executions. Both full-gate crate stages in
  `tools/gate --full` use it independently; `full_gate_counts.py <log>` checks a
  saved log. Launcher and counter regressions run in the fast gate.
- On the pool an action's work directory is a bind mount of a subdirectory.
  The runtime refuses to record there, since it cannot establish the mount's
  source ancestry (`crates/hirsel-rt/src/host_paths.rs`), so the launcher puts
  `CARGO_TARGET_TMPDIR` and `TMPDIR` on the action's private `/dev/shm`.
- First-party binaries carry an `$ORIGIN`-relative RPATH to
  `tools/buck2/toolchains/llvm-files/lib`, so a test's child processes find
  libLLVM with a cleared environment; tests on the pool carry that directory
  (`toolchains//:llvm-libraries`).
- The driver's build script runs over the files it hashes, laid out at their
  repository paths (`hirsel_build_script` in `defs.bzl`); its output directory
  is the driver's `OUT_DIR`.
- llvm-sys's build script is replaced by its fixup: `wrappers/target.c`
  compiles against the pinned headers and binaries link the pinned libLLVM
  (`toolchains//:libllvm`), as `force-dynamic` selects under Cargo.

The runtime archive rule follows Buck2's `RustLinkInfo` native link graph and
uses LLVM ar's MRI `ADDLIB` interface to include native dependency objects in
each `libhirsel_rt.a`. This expands ring's thin C/assembly archives on both
targets, so compiled Hirsel programs still link one self-contained runtime
archive. `bundle_archive.py` preserves duplicate member names without extracting
them into a shared directory.

## Toolchains and execution

- Rust: the official 1.99.0 components (rustc, std for x86-64 and AArch64,
  clippy, rustfmt), byte-identical to the rustup ones, from
  `hermeticbuild/rust-redist`.
- C and linking: clang 21.1.8 from the pinned LLVM tree
  (`tools/native-toolchain/llvm-21.lock`). The Ubuntu clang and libLLVM load
  four libraries a minimal Ubuntu worker image lacks; `llvm-runtime-lock.json` pins
  them and `bootstrap_llvm.py` bundles them in the tree's `lib/`.
- `platforms.bzl`: one remote execution platform per measured `(CPU, memory,
  CPU class)` budget, with a 1 CPU/1 GiB compile default, 1 CPU/512 MiB Clippy
  twins and measured test requests (below), carrying the pool's platform
  properties. Only the critical-path compiles named in `CPU_CLASSES`
  (`action_sizes_from_log.py`) and the conformance shards ask for
  `cpu_class=fast`; other tests and gate shards run on any worker. Every
  remote action, compile or test run, gate, focus or full, queues at priority
  50: below the pool's lane and CI class (100), above its warming class (0),
  which gets no reserved worker. One priority, since the scheduler merges
  identical actions at the first submitter's priority. A test run carries its
  own executor (`test_executor` in `platforms.bzl`): Buck2 runs tests with it,
  not with their platform's. The priority is not part of an action key. The
  platforms also include the invoking host for an explicit
  `-c hirsel.tests=host` opt-in. Native tools are pinned action inputs in
  either mode; no test needs those tools installed on the worker image.
- Profiles (`profile.bzl`): the default target platform `//tools/buck2:dev` is
  Cargo's dev profile (third-party crates at opt-level 1, and the members a
  root `[profile.dev.package.<name>] opt-level` names at that level);
  `//tools/buck2:release` is `[profile.release]`.

## Cost-balanced test shards

`hirsel-cli-test-examples` uses one shard per declared test case (`CASE_SHARDED`
in `sync.py`), including its ignored native cases. The existing launcher deals
these equal-cost cases by name, preserving compiled test identities and exact
filters; an exact example runs through `hirsel-cli-test-examples-shard-1`.
Rerun `sync.py` after adding or removing an example test.
Each shard uses the examples binary's own measured request in `test_sizes.bzl`.
Its reservation is independent of CLI integration tests, whose concurrency and
memory needs differ.
The gate selects every shard, so independent examples can run in parallel without
one long action holding the whole examples suite's reservation.

A binary in `SHARDED` (`sync.py`) has a table of measured seconds per case,
`test-costs/<binary>.tsv`. `sync.py` gives it `ceil(total / (TEST_THREADS *
SHARD_SECONDS))` shards, and `test_launcher.sh` deals its listed cases out
longest first to the least loaded shard; a case without a row costs the
table's mean. After tests are added, slowed or sped up, re-measure every
shard of the binary with libtest's timing report and regenerate:

```sh
.buck2/bin/buck2 test //:hirsel-driver-test-it-stdlib-shard-1 ... -- --env RUSTC_BOOTSTRAP=1 \
  --test-arg=-Zunstable-options --test-arg=--report-time --timeout 3600 > /tmp/costs.log 2>&1
python3 tools/buck2/test_costs.py /tmp/costs.log   # writes the tables, prints walls and slowest cases
python3 tools/buck2/sync.py
```

Shard flags never select cases; only positional filters do, and a filtered
run executes in shard 1.

## Prelude overlay

The prelude cell is the one bundled in the pinned executable, expanded to
`.buck2/prelude` (`.buckconfig`: `[external_cells] prelude = disabled`) and
patched by `prelude_overlay.py`. Command wrappers use `/bin/bash` from the
pool image, so rustdoc can invoke its linker without a `PATH`. The LLVM tool
wrappers also locate their tree using shell builtins. It moves helper work off
the pool:

- `failure_filter` (`rust/failure_filter.bzl`): the daemon reads a compile's
  build status; a passing compile's output is a declared copy, and only a
  failed one runs the stock action, which replays rustc's diagnostics.
- `deps` (`rust/build.bzl`): each compile's `-Ldependency` tree is laid out by
  the daemon with `ctx.actions.assembled_dir`, the same tree the stock remote
  action wrote, so consumer action digests do not change. It refuses
  `crate_dynamic`, which no target uses.
- Compiler and rustdoc actions no longer carry extracted `third-party` crate
  sources (`rust/sources.bzl`, `rust/build.bzl`): rustc reads dependency text
  only at repository paths, where those archives never are.

`pins.json` (`prelude_overlay`) pins each patched file's stock and overlaid
SHA-256, and every transform replaces a stock text found exactly once, so a
prelude pin change fails `prepare.py` instead of mis-patching.
`tools/tests/prelude_overlay.py` covers the guards. After a Buck2 upgrade,
expand the new prelude, port the transforms, and re-pin with
`prelude_overlay.py --print-pins --prelude-dir <stock prelude>`. To drop the
overlay, set `[cells] prelude = prelude` and `[external_cells] prelude =
bundled`, and delete `prelude_overlay.py`, its pins, its test `tools/tests/prelude_overlay.py`, and
`prepare_prelude` in `bootstrap.py`.

## Measured test requests

A pool test binary reserves what it measured using: `test_sizes.bzl`
(`TEST_REQUESTS`, evidence in `test-sizes.json`) gives each binary, shared by
its shards, one CPU while its p95 core use is at most 1.6 (a one-CPU action
already runs up to the worker's slot share), otherwise two. No gate test asks
for more: the pool holds its one reservation for a blocked multi-CPU action, so
three or four CPUs both wait minutes and hold back the actions behind them,
where two start in seconds. The evidence keeps the parallel fraction and the
wall time two CPUs take relative to `clamp(ceil(p95), 2, 4)`; at two, every
gate binary's p95 wall, scaled from its measured runs, stays under a
quarter of its 1800 s timeout.
Only the full run's agreement shards, which already reach their timeout at
four, keep four. The launcher runs as many test threads as the request's whole
CPUs.

Memory follows the worker's kill rule: it samples an action's process-group
PSS, shared libraries and mapped binaries included, and kills it past twice its
`memory_kb`. A binary reserves its largest PSS peak / 2 * 1.2, rounded up to
256 MiB, at least 512 MiB, and at least 1.75 times the request of any run of it
killed for memory. The full native stages without measurements inherit the largest measured native
stage's request, recorded as `fallback_from` in the evidence. Other binaries
without a row reserve 1 CPU and 2 GiB.

The inputs are read-only copies: each worker's `actions.log` and
`actions.log.1` (`TEST_TARGET` and CPU per test run), the scheduler's usage
events (`trace/*.jsonl` of kiln-resource-hints on the cache host: PSS peaks by
operation) and the workers' `kiln-executor` journal lines for memory kills:

```sh
python3 tools/buck2/test_sizes_from_usage.py --usage /tmp/usage-*.log \
  --pss /tmp/trace-*.jsonl --kills /tmp/kills.txt
# Refresh only full native stages, preserving the other measured requests:
python3 tools/buck2/test_sizes_from_usage.py --native-stages \
  --usage /tmp/usage-*.log --pss /tmp/trace-*.jsonl --kills /tmp/kills.txt
# Refresh selected binaries without discarding other measurements:
python3 tools/buck2/test_sizes_from_usage.py --only hirsel-cli-test-cli \
  --usage /tmp/usage-*.log --pss /tmp/trace-*.jsonl
```

A request is part of the test's action key: resizing reruns those tests once.
The gate selects the resized binaries directly, and the bounded campaigns whose
requests changed. Evidence-only changes run tool checks. Full-only stages remain
on demand; a reservation update never starts a full native campaign.

`pool_usage.py` reports, read-only from the same trace and usage rows, the
pool's reserved CPU-hours by repository and hirsel's reserved against used
CPU-hours and GiB-hours per lane and target over the last N hours.

It also sums execution wall seconds and CPU seconds per action category,
excluding queue time. For rustc compile/link phases and first-party target
names, add exported Buck2 event logs:

```sh
.buck2/bin/buck2 log show /tmp/build.pb.zst > /tmp/build.jsonl
python3 tools/buck2/pool_usage.py --hours 6 --trace /tmp/trace-*.jsonl \
  --usage /tmp/usage-*.log --events /tmp/build.jsonl
```

This reuses the compile-sizing tool's unique time/request match; unmatched or
ambiguous rustc measurements stay in `rustc (phase unknown)`. Rustc's link
action includes compiling the executable, so its time is not linker-only.

## Measured compile requests

`action-sizes.json` keeps dev/release budgets per target, with phase timings,
CPU seconds and anonymous memory peaks. `sync.py` generates `exec_sizes.bzl`
from it; compile rules select that target's platform. A dev compile splits its
code into 16 codegen units whatever its reservation, so LLVM spreads them over
the reserved CPUs, and one reserving more than one CPU runs rustc's front end on
that many threads (`-Zthreads`; `_dev_codegen` in `defs.bzl` has the
measurements). A release compile's codegen units match its reservation;
unmeasured release targets keep Cargo's defaults.
One target's metadata and codegen share the reservation.
Cargo profiles and dependency resolution remain in Cargo.toml.

`action_sizes_from_log.py` joins Buck2 events, which identify owners,
initial requests and timing, to the pool workers' per-action usage logs,
which supply CPU and anonymous/cgroup memory. It uses only unique successful
matches, leaves ambiguous/missing usage unmeasured, and records those
omissions. Copy each worker's usage log to `/tmp/usage-<worker>.log`, then:

```sh
.buck2/bin/buck2 build //: --event-log /tmp/build.pb.zst
python3 tools/buck2/action_sizes_from_log.py --events /tmp/build.pb.zst \
  --usage /tmp/usage-*.log -o tools/buck2/action-sizes.json
python3 tools/buck2/sync.py
```

Final CPU sizing is one CPU while p95 cores is at most 1.6 (a one-CPU action
runs unthrottled up to the worker's slot share), otherwise
`clamp(ceil(p95_cores), 2, 4)`; below 20 samples the maximum replaces p95.
Release codegen units match the request, and one-CPU release targets keep two;
dev compiles use 16. A probe's old request is evidence, never a reservation floor. Schedulability on the shared pool beats peak
parallelism: smaller requests can start without a whole eight-CPU slot.
Memory keeps its measured headroom. The script documents memory headroom and sample
handling. Requests participate in action keys, so resizing deliberately
invalidates those actions.

A compile measured under one CPU reads as using one core, so the rule never
grows it. The critical-path compiles in `CPU_CLASSES` are calibrated instead:
`-c hirsel.probe_cpus=8` gives each of them eight CPUs (and, in release, codegen
units) for that invocation, `action_sizes_from_log.py --merge` sizes their rows from that
evidence, and `CALIBRATED_CPUS` keeps the reservation that measured the least
wall time including queue on the loaded pool; only such a row may exceed four
CPUs. A crate split that renames a `CPU_CLASSES` target fails `sync.py` until
the list names the new targets.

`clippy.py` builds each target's Clippy twin `<name>-clippy`: the same rule on
a 1 CPU/512 MiB platform, declared only under `-c hirsel.clippy=true`, so
Clippy never reserves a compile's request and `buck2 build //:` never
compiles the twins.

Agreement limits the global seed-1 catalogue to its lowest `AGREE_LIMIT` (5000)
ranks before partitioning by stable variant rank modulo `AGREE_SHARDS` (16), both
in `sync.py`. The sum is the unsharded sample, including
skipped variants. `agree.py` checks unique shard identities, matching catalogue
metadata, sample coverage and classification/edit totals, and fails on any
cluster or absent shard. Each pool action has a 300-second timeout.
`tools/gate --full` runs `agree.py --full`: a rotating sample of at most 96 variants per program over
`AGREE_FULL_SHARDS` (96) shards by the same stable rank, each under its own
`AGREE_FULL_TIMEOUT` (1800 s), so a slow or non-terminating variant fails one
named shard. The campaign has grown past what one 4-CPU action can finish
(286950 variants on 2026-10-16, about 33000 shard-seconds on the pool); each
shard prints its stage times, slowest variants and costliest programs to stderr,
and names any variant still running after 30 s.


Unfiltered native campaign shards consume `//:native-selection`, a cached
derived coverage artifact built from the collector binary and the complete corpus. It selects
an inclusion-minimal feature cover after historical pins. Each selected
baseline/generated program runs a rotating feasible-pair configuration cover;
topology partitions and named exclusion cells remain present. `--exhaustive`
on the campaign binary restores the previous exhaustive release check. The
campaign seed defaults to the full run's `PROPTEST_RNG_SEED`; `--seed N` makes a
run reproducible. The source/IR full agreement keeps all programs and budgets
96 mutations each; `--exhaustive` restores its complete catalogue.

The whole `gate-native-full` target has no coverage-artifact dependency, since
Buck2 fixes its build graph before it receives test-runner arguments. Explicit
`--filter` values are the program selection and skip coverage collection. An
unfiltered whole-target run derives the same inclusion-minimal cover in its
own scratch directory; full-gate shards continue to share the cached artifact.
