# Buck2 build

The workspace builds and tests with Buck2, on this host or on an optional
remote-execution pool (for example NativeLink, with a shared content-addressed
cache). Cargo manifests and `Cargo.lock` stay the source of truth: `sync.py`
derives every Buck2 build file from them, and `cargo build` keeps working.

## Generated files

No lane commits a generated file, so no rebase can conflict on one
(`generated.py`):

- **Build inputs are gitignored and generated on demand.** `sync.py` writes the
  root and `third-party/` `BUCK`, `third-party/Cargo.{toml,lock}` and `lib.rs`,
  `exec_sizes.bzl`, `test_sizes.bzl`, `workspace-metadata.json` and
  `tools/native-toolchain/pins.bzl`. `bootstrap.py` (so `prepare.py`),
  `tools/land`, `tools/main-runs`, `tools/bless.sh` and
  the runtime benchmark harness run
  `sync.py --if-stale`. Generation has two parts, third-party dependencies
  (Cargo metadata, then Reindeer) and the first-party inventory (Cargo
  metadata, then source inspection), and a stamp in `.buck2/` records each
  part's outputs and every file, existence test and glob it read. `--if-stale`
  regenerates only a part whose outputs are missing or edited or whose inputs
  read differently, so a documentation edit regenerates nothing and a Rust
  source edit reruns only the inventory; either part replaces an output only
  when its bytes change. `sync.py` without flags regenerates both parts. After
  you edit sources or rebase, run `prepare.py` or
  `sync.py --if-stale` before a bare `.buck2/bin/buck2`. Where the root BUCK is
  missing, Buck2 reads the committed `BUCK.unsynced` (`[buildfile]` in
  `.buckconfig`), which fails saying to run `sync.py`. The files are
  byte-identical to the committed ones they replace, so every action key, and
  so remote execution and its cache, is unchanged.
- **`docs/api.md` is committed, and only main's hourly run writes it**:
  `tools/main-runs` regenerates the page with the built CLI after each run and
  pushes a change straight to main, so main's page trails its sources by at
  most one run and no test compares the two. `tools/land` refuses a change
  that touches it (or force-adds a build input) and prints the commands that
  drop it.
- **Files many lanes add to merge themselves.** `.gitattributes` gives
  `third-party/NOTICES.md`, every `hirsel.json` and the sorted ratchet and
  expectation lists (`SORTED_LISTS` in `merging.py`) the `hirsel` merge
  driver, which `bootstrap.py` installs in the clone's config: appended rows
  and entries either side adds stay, entries either side removes go, and only
  two sides changing one entry stays a conflict. Lanes' own rebases and land's
  batch rebase merge them alike.
- `tools/known-failures.tsv` stays hand-edited, direct to main.

## Setup per checkout or worktree

```sh
python3 tools/buck2/prepare.py        # bootstrap.py, then the remote-execution hook
python3 tools/buck2/prepare.py --local  # or: no pool, every action on this host
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
pool's workers match. When the hook is missing or fails, an existing remote
`.buckconfig.local` is kept; without one `prepare.py` fails and names the
missing hook. Only `prepare.py --local` writes `hirsel.execution = local`,
which runs every action on this host, so an unattended `tools/land` or
`tools/main-runs` never silently redoes the pool's work here.

Toolchain preparation runs on the pool like any compile: the third-party crate
`http_archive`s, the `native_toolchain_unpack`s and the `link_kit`
(`native-toolchain/defs.bzl`) resolve to the default pool platform, so every
checkout after the first takes their outputs from the remote action cache and
never downloads or unpacks them here. Only targets that name
`//tools/buck2:host` (tests that run the host's own tools) stay local, uncached:
the pool's client listener serves the action cache read-only, so a local
result could not be uploaded (`allow_cache_uploads`) anyway.

## Stale outputs

Each checkout's daemon prunes its own inactive outputs: a six-hour TTL, an
hourly scan, and a six-minute initial delay. At 10% free disk or less, adaptive
cleaning promotes the oldest inactive outputs until projected free space
recovers, keeping anything accessed within one hour. Active artifacts stay
protected, including under disk pressure. Lanes never delete another lane's
`buck-out`. The policy is initialized at daemon startup; an existing daemon
needs a restart. Gate and land stop their own daemon when finished.

The settings were checked against the exact `pins.json` source commit
[`6507dd157a6f81a810c48583edf1758dd0c337c5`](https://github.com/facebook/buck2/tree/6507dd157a6f81a810c48583edf1758dd0c337c5),
and the installed executable's SHA-256 matched the pin. Source references below
are relative to that commit:

- `app/buck2_execute_impl/src/materializers/deferred/clean_stale.rs`,
  `CleanStaleConfig::from_buck_config`: `clean_stale_enabled` and both adaptive
  protection switches are booleans; `clean_stale_artifact_ttl_hours`,
  `clean_stale_period_hours`, `clean_stale_start_offset_hours`, and
  `clean_stale_low_disk_adaptive_min_ttl_hours` are nonnegative floating-point
  hours. `clean_stale_low_disk_adaptive_enabled` is a boolean.
  `clean_stale_low_disk_threshold` is a floating-point **free-space percentage**,
  so 10 engages at 90% full. `clean_stale_low_disk_artifact_ttl_hours` is an
  optional floating-point hour value for fixed mode, ignored in adaptive mode;
  we leave it unset. The two protection switches are
  `clean_stale_low_disk_adaptive_delete_intermediate_within_min_ttl = false`
  and `clean_stale_low_disk_adaptive_unmaterialize_active = false`.
- That file's `create_pending_clean_result` requires SQLite state and deferred
  writes. `app/buck2_server/src/daemon/state.rs` creates the deferred
  materializer, but **`defer_write_actions` defaults to false**. We explicitly
  set it and `sqlite_materializer_state` to `true`; both accept booleans via
  `RolloutPercentage` (`app/buck2_core/src/rollout_percentage.rs`). SQLite state
  defaults to true (`app/buck2_server/src/daemon/disk_state.rs`). Changing the
  deferred-write setting invalidates old persisted materializer state, so the
  first scan also removes untracked outputs after current outputs are declared.
- `StaleFinder::visit` and `find_stale_tracked_only` require `active: false`
  before marking tracked artifacts stale. `apply_adaptive_low_disk` promotes
  only `TrackedState::Retained`; it returns before considering active artifacts
  when `unmaterialize_active` is false. Declarations and materialization mark
  current artifacts active (`deferred/command_processor.rs`); state loaded
  after a daemon restart begins inactive (`deferred/artifact_tree.rs`).
  `MaterializerSender::send` cancels a cleaning
  scan before processing new materializer work (`deferred.rs`).
- `tools/land`, `tools/main-runs` and `prepare.py` do not override these
  keys. `--materializations none` controls requested final outputs, not the
  deferred-write prerequisite. The host remote-config hook writes only
  `[buck2_re_client]` and `[hirsel]`, leaving this `[buck2]` policy intact.

Six hours preserves recent rebase outputs without retaining days of hashes;
hourly scans bound the delay, and six minutes gives an initial build time to
declare its outputs. One hour under disk pressure retains recent reuse while
reclaiming older inactive outputs. The policy cannot reclaim an active set
kept alive in one daemon indefinitely.

Validation on 2026-10-20 used a disposable copy of `fs-watch-mmap/buck-out`,
with a consistent read-only SQLite backup, in `buck-clean-stale`. After building
`//:hirsel-toolchain` and `//:hirsel-span` and testing `//:hirsel-span-unittest`,
allocated `buck-out` size (`du -s -B1`) fell from **22,952,734,720 bytes
(21.376 GiB)** to **3,068,723,200 bytes (2.858 GiB)**, an **86.63%** reduction:

```sh
.buck2/bin/buck2 clean -c buck2.clean_stale_enabled=false --stale 6h \
  --adaptive-low-disk-threshold 10 --adaptive-min-ttl 1h
```

Only the periodic schedule was disabled during measurement to isolate the
explicit scan. Buck reported 5,292 untracked paths removed (19.6 GiB logical
size); switching deferred writes had invalidated the copied old database.
Thus this measures the initial migration cleanup, not steady-state tracked
TTL eviction. The policy also passed a configured-policy dry run. A zero-TTL
adaptive dry run found zero stale paths and retained all 544 active/recent
artifacts. All three current CLI, std image and library output checksums stayed
identical. The incremental build and test then used zero build actions; the
unit test passed before and after (one test each). After restarting the daemon
with the committed policy, the same build hit 604 cached actions, with zero
remote executions (172 local preparation actions); the test passed again.
The copied runtime archives accounted for about 8.16 GiB and toolchain bundles
3.36 GiB before cleanup; sharing these across worktrees is separate work.

## Distribution compiler

Native runtime archives use a separate `profile_runtime` on both target
architectures: fat LTO, `opt-level=3` for the runtime and its live-operation
dependencies, and `opt-level=s` for run-config, run-schema and the Serde codecs.
The allowlist and flags live in `profile.bzl`; generated programs keep the
native compiler's speed optimization. Compiler dev/release/dist profiles and
host build scripts are independent of this archive profile.

`tools/toolchain-dist/build --pgo --output /tmp/hirsel-dist` builds the shipped
compiler with `//tools/buck2:dist`: fat LTO, one codegen unit and PGO trained on
compiler-bench's examples and syn3k. The existing mimalloc dependency already
builds v3. `tools/compiler-bench/run-main` measures this distribution pipeline.
Developer `dev` and `release`, checked tools and runtime archives keep their
existing profiles. See [the pipeline](../toolchain-dist/README.md) for exact
commands, profile provenance, interleaved measurements and the declined BOLT trial.

## Everyday commands

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-toolchain --show-output # CLI and sibling std image
.buck2/bin/buck2 build //:                        # every first-party target, dev profile, on the pool
.buck2/bin/buck2 test //...                       # every test
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir           # one crate, including doctests
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-rc-shard-2  # one shard of a sharded test binary
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-pipeline-tests-test-rc-shard-1   # run tests on this host
.buck2/bin/buck2 test //:gate-native-smoke          # the native smoke
.buck2/bin/buck2 test //:gate-link-hosts            # native-tests/link built here and on a pool worker, byte-compared
python3 tools/buck2/conformance.py                # the whole conformance campaign's shards (full suite only)
python3 tools/buck2/agree.py --native-full        # the native campaign's shards, merged against agree-known.txt (full suite only)
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-conformance-bin-native
python3 tools/buck2/sync.py --if-stale            # the generated build files, after any edit or rebase
tools/land                                           # rebase, build, Clippy, fmt, push
```

[Landing and race retries](../../docs/landing.md) describes the exact-tree checks,
conservative incoming-crate selection and unbounded retries with jitter.
Every attempted tree passes build, Clippy and fmt; no retry skips a check.

After each run, `tools/main-runs` runs the built `hirsel doc stdlib/std@1/hirsel.json`
and pushes a changed `docs/api.md` straight to main. The index's export-count
test checks the renderer; nothing compares the committed page, which only
main-runs writes.

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

Pool test actions reserve 5% of the unchanged 600-second RE limit for reporting.
At the 570-second inner deadline, `test_report.py` prints
`HIRSEL_TEST_INNER_DEADLINE`, every completed body's result, and each
`HIRSEL_TEST_RUNNING` name with its elapsed time, then terminates its own test
descendants and exits 124. Descendants keep the executor's process group for
memory sampling and enforcement. This returns an ordinary failed action with its
output before RE's `DeadlineExceeded` discards that output. Rust bodies are
tracked through libtest's JSON start/result events; ordinary result lines,
summaries and per-case timings remain available to count and cost tools.
Python unittest scripts retain their verbose output, including the active
method. A deadline during setup, discovery or teardown is labeled separately.
The action's CPU and memory grants and outer RE timeout remain unchanged.

Heavy Buck2 commands run from this host and execute on the pool when one is
configured. Native changes use a filtered campaign:

```sh
.buck2/bin/buck2 test -c hirsel.full=true //:gate-native-full -- --test-arg=--filter --test-arg=SEM-4-view-return-source-run --timeout 14400
```

Native tests receive LLVM 23.1.4, the link kit and glibc 2.28 floor runtime
(`native-toolchain:kit`, `:glibc_floor`), Arm GNU 14.3.Rel1 with its Linux
sysroot, static AArch64 qemu and Valgrind as SHA-pinned Buck2 downloads or
builds from them. No worker
image changes, PATH lookup or binfmt registration are needed. The launcher
resolves `LLVM_SYS_231_PREFIX` and each variable of
`native-toolchain/environment.tsv` (`HIRSEL_AARCH64_CC`, `HIRSEL_AARCH64_SYSROOT`,
`HIRSEL_QEMU_AARCH64`, `HIRSEL_VALGRIND`, `VALGRIND_LIB`,
`HIRSEL_VALGRIND_RUNTIME`, `HIRSEL_LINK_KIT`, `HIRSEL_GLIBC_FLOOR`: a path inside
one toolchain) to absolute input paths before changing directory;
`bench/native/suite.py` sets the same table. The driver links with the input
`ld.lld` and the link kit's line (`crates/hirsel-driver/src/native/kit.rs`),
never a compiler driver. Native launches preserve the selected
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
| a corpus campaign | `//:gate-<stage>` (label `gate`) |
| a conformance shard (`CONFORMANCE_SHARDS`) | `//:gate-conformance-shard-<i>` |
| a slice of the corpus cases a lane chose (`-c hirsel.conformance_paths`, up to `SELECTED_SLICES`) | `//:gate-conformance-selected-shard-<i>` |
| a slice of the chosen cases' variants in the seed-1 sample (`-c hirsel.agree_paths`) | `//:gate-agree-selected-shard-<i>` |
| a budgeted full agreement shard (`AGREE_FULL_SHARDS`, full suite only) | `//:gate-agree-full-shard-<i>` |
| a budgeted native agreement shard (`AGREE_NATIVE_FULL_SHARDS`, full suite only; `agree.py --native-full` runs and merges them) | `//:gate-agree-native-full-shard-<i>` |
| a full native matrix shard (`NATIVE_FULL_SHARDS`, full suite only; `gate-native-full` stays whole for `--bless` and filtered runs) | `//:gate-native-full-shard-<i>` |
| a historical native replay shard (`HISTORY_SHARDS`, on demand) | `//:gate-native-history-shard-<i>` |
| a root-anchor trace shard (`TRACE_ROOTS_SHARDS`, on demand; `gate-native-trace-roots` stays whole for filtered runs) | `//:gate-native-trace-roots-shard-<i>` |

## Instruction budgets

`.buck2/bin/buck2 test //:gate-perf-counts` checks the
17 small native benchmarks and all eight `bench/micro/programs/*.hir` at
N=1000 (one epoch) on the pool. The complete 25-case measurement, including
builds and annotation, reserved 125.37 seconds on one pool CPU (2.09
pool-minutes) and used 119.46 CPU-seconds. Run it for changes
to LLVM, RC, LIR, the runtime,
stdlib and benchmark programs. Valgrind, its loader/libc, LLVM and the runtime
archives are pinned inputs. Native builds use LLVM O3, one epoch of each
small input read through an explicit `fixtures` directory grant, and must
print the committed checksum. Runtime input prevents compile-time folding of
the workload. Native counts cover fixture loading, the original workload,
checksum, complete teardown and stdout. Both builds and diagnostic compiler
checks use the production release compiler and the pinned std image,
with one compiler worker. Cache simulation is disabled: only `Ir` is counted,
so host cache geometry and wall time cannot change the verdict.

`tools/perf-counts --bless` measures on the pool and regenerates
[`tools/perf-counts.tsv`](../perf-counts.tsv) on the host. Review and commit
that diff. An increase beyond 1% fails; exactly 1% passes. A decrease passes
and prints `perf-counts: improvement`; main's full run (`tools/main-runs`)
writes the lower counts it measures into the table with a direct push.
An automatic update never raises a budget; upward moves require an explicit
reviewed bless with a reason. Drops over 15% still update the baseline and
print `main-runs: PERF-CHECK <count> <old> -> <new> (run: <sha>)` after a
successful push. The report and the run's `runs.jsonl` row retain these
`perf_checks`, including the measured and pushed SHAs, for review of skipped
work. Exactly 15% does not alert. A rejected push regenerates from the newer
baseline and reports only the changes that actually land.
The table retains the top 50 `cg_annotate` function counts per case, without
worker source paths. Failures print the largest 20 function deltas against
those saved counts. Missing and removed cases fail too.

The two compiler cases remain explicitly open in the runner and table:
`check/hello-web` took 113 seconds with the checked compiler, and
`check/todo-api` exceeded the unchanged 240-second process limit with checked
code and took 232.56 seconds (27,528,240,687 instructions) in release. The owner
approved landing the 25 native/micro cells within five reserved pool-minutes
while compiler performance is addressed. A diagnostic case remains runnable:

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus //:gate-perf-counts -- --test-arg=--record --test-arg=--filter --test-arg=check/todo-api
```

## Blessing expectations

`tools/bless.sh <what>...` rewrites checked-in expectations from the code that
derives them; `tools/bless.sh` with no argument lists every `<what>` and the
files it writes. Review the diff it prints before committing.

```sh
tools/bless.sh heap                # native-tests/heap goldens
tools/bless.sh layout rc-import    # several at once
tools/bless.sh all                 # every in-test bless
```

A test rewrites its expectation when it runs with `HIRSEL_BLESS=1` (insta
snapshots: `INSTA_UPDATE=always`). The script runs those cases on this host
(`-c hirsel.tests=host --env HIRSEL_BLESS=1`), where a test reads and writes
the checkout through its relative `CARGO_MANIFEST_DIR`. It fails unless a
case ran, then runs the same cases again without the bless, on the pool, which
fails unless the rewritten files now pass. `exclusions` runs
`native --full --bless` through `gate-native-full` with explicit host execution.

A pool action reads a copy of the checkout that is discarded afterwards, so a
bless there writes nothing that lasts: a read-only input fails with a bare
`PermissionDenied` and a writable one passes. `test_launcher.sh` therefore
refuses, with exit 2, a pool test run under `HIRSEL_BLESS`, a writing
`INSTA_UPDATE` or `--bless`.

A new expectation file gets its rewrite under `HIRSEL_BLESS` in the test that
checks it, and a row in `tools/bless.sh`. A test whose expectation is compiled
in with `include_str!` returns after writing, since it would compare against
the old copy.

## How the graph is made

- `sync.py` writes `third-party/Cargo.toml` (every external crate a member
  names, at its locked version with the features Cargo resolved for the
  workspace), `third-party/Cargo.lock` (from the workspace lock, so no version
  moves), `third-party/BUCK` (Reindeer, configured by
  `third-party/reindeer.toml` and `third-party/fixups/`) and the root `BUCK`,
  all gitignored (Generated files above).
- Native and full-gate crate cases use the same pool execution path as ordinary
  tests. The runtime ABI symbol test reads the separately built production
  archive; the layering test reads `workspace-metadata.json`, generated from
  Cargo's workspace graph by `sync.py`. Neither
  invokes Cargo on a worker.
- A pool test's passing verdict is cached by its action key. Test binaries
  whose sources draw random cases (`proptest!`, `SEEDED` in `sync.py`) run
  under `PROPTEST_RNG_SEED` from `-c hirsel.test_seed` (default 1), which is in
  that key: a cached pass covers its seed only. `tools/main-runs` draws a new
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
  `LLVM_SYS_231_PREFIX` (its `bin/` first on `PATH`, its `lib/` on the loader
  path), insta's workspace root, and test threads and `HIRSEL_JOBS` set to the
  action's effective CPU reservation (`HIRSEL_TEST_CPUS`, passed from the same
  request as the executor, including focus mode), at most 4. A visible cgroup
  quota or a smaller caller setting can reduce it; command-line
  `--test-threads` cannot exceed it. Pool actions require the explicit
  reservation, since their sandbox need not expose `cpu.max`.
  It also runs one shard of a sharded binary,
  by exact names: cases go longest first to the least loaded shard, weighted
  by the binary's measured cost table. It asks the compiled harness to apply
  caller filters, then applies sharding.
  The `full_gate_ --ignored` group also emits the unpartitioned compiled case
  inventory. `full_gate_counts.py --run <command...>` streams Buck2's log and
  checks every `(binary, case)` appears exactly once, rejecting missing,
  duplicate, failed or unexpected executions. Both full-gate crate stages in
  `tools/main-runs` use it independently; `full_gate_counts.py <log>` checks a
  saved log.
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

Workspace build scripts publish only their generated files in `OUT_DIR`.
Their unused Cargo rerun directives are discarded: those directives contain
absolute action-sandbox paths and filesystem traversal order, so keeping them
would change consumer input digests under an identical build-script action key.

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
- C and linking: clang 23.1.4 from the pinned LLVM tree
  (`tools/native-toolchain/llvm-23.lock`). The Ubuntu clang and libLLVM load
  five libraries a minimal Ubuntu worker image lacks; `llvm-runtime-lock.json` pins
  them and `bootstrap_llvm.py` bundles them in the tree's `lib/`.
- `platforms.bzl`: one remote execution platform per measured `(CPU, memory,
  CPU class)` budget, with a 1 CPU/1 GiB compile default, 1 CPU/512 MiB Clippy
  twins and measured test requests (below), carrying the pool's platform
  properties. Only the critical-path compiles named in `CPU_CLASSES`
  (`action_sizes_from_log.py`), the std precheck (`PRECHECK_REQUEST`, 1 CPU
  and 2 GiB) and the conformance shards ask for `cpu_class=fast`; other tests
  and gate shards run on any worker. Every
  remote action, compile or test run, gate, focus or full, queues at priority
  50: below the pool's lane and CI class (100), above its warming class (0),
  which gets no reserved worker. One priority, since the scheduler merges
  identical actions at the first submitter's priority; `-c hirsel.priority=N`
  overrides it per invocation: tools/land at 75, so the batch every lane
  waits on does not queue behind lanes' test runs, and main's hourly full
  suite (tools/main-runs) at 40, below the lanes. A test run carries its
  own executor (`test_executor` in `platforms.bzl`): Buck2 runs tests with it,
  not with their platform's. The priority is not part of an action key. The
  platforms also include the invoking host for an explicit
  `-c hirsel.tests=host` opt-in. Native tools are pinned action inputs in
  either mode; no test needs those tools installed on the worker image.
- Profiles (`profile.bzl`): the default target platform `//tools/buck2:dev` is
  Cargo's dev profile (third-party crates at opt-level 1, and the members a
  root `[profile.dev.package.<name>] opt-level` names at that level);
  `//tools/buck2:release` is `[profile.release]`;
  `//tools/buck2:checked` is `[profile.checked]` (release with assertions and overflow checks).
  Test harnesses and the executables they launch use `//tools/buck2:checked`,
  an optimized profile with debug assertions and overflow checks enabled. This
  includes `CARGO_BIN_EXE` subprocesses, conformance, agreement and native
  compiler runners. Their runtime archives retain the production release
  profile. Proc macros and build scripts retain their canonical dev profile.
  The std image pins its exec dependency to this same checked configuration:
  an ordinary `exec_dep` inherits the execution platform's configuration, which
  has no Rust profile, and would fall back to dev even under a release caller.
  Checked compiles use the dev compile requests and codegen policy, preserving
  their memory headroom and parallelism while enabling O3.
  Canonicalizing test harnesses on entry also keeps the workspace build and
  tests from compiling the same harness under different reservations.

## Cost-balanced test shards

`hirsel-cli-test-examples` and `hirsel-cli-test-free` use one shard per
declared test case (`CASE_SHARDED` in `sync.py`), including ignored native
cases. A Free replay matrix case needs most of one action's 600 s limit, and
its one-permit cells must not share a CPU with a co-scheduled case. The existing launcher deals
these equal-cost cases by name, preserving compiled test identities and exact
filters; an exact example runs through `hirsel-cli-test-examples-shard-1`.
Rerun `sync.py` after adding or removing an example test.
Each shard uses the examples binary's own measured request in `test_sizes.bzl`.
Its reservation is independent of CLI integration tests, whose concurrency and
memory needs differ.
The gate selects every shard, so independent examples can run in parallel without
one long action holding the whole examples suite's reservation.

A binary in `SHARDED` (`sync.py`) has a table of measured seconds per case,
`test-costs/<binary>.tsv`. `sync.py` gives it `ceil(total / (cpu_count *
SHARD_SECONDS))` shards using its pool grant from `test-sizes.json`, and `test_launcher.sh` deals its listed cases out
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

`ISOLATED_TEST_CASES` in `test_requirements.bzl` gives an indivisible slow body
its own `<binary>-isolated-<n>` target with the binary's existing resource request
and deadline. The launcher removes it after ordinary cost assignment, preserving
every other partition. Explicit body filters still work through shard 1. Main's
test query includes the dedicated targets, and full-gate selection cannot make
one of them execute other bodies. The URI file vectors use this partition to
avoid spending the stdlib aggregate's deadline on repeated checked executions.

After regenerating stale build files, `sync.py --check` lists each cost-balanced
binary's cases on the pool (including ignored and macro-generated cases, without
running test bodies). It fails if more than 5% of the driver's cases have no
cost row, or if listing fails. The stdlib, front, rc and rt binaries warn at
the same threshold until real gate results refresh their stale tables; no extra
measurement campaigns run for them. Stale rows do not contribute to coverage. Prepare the worktree first;
the listing can build changed harnesses. Measure costs before regenerating the
graph so `test_costs.py` can recognize a complete set of the old shards and
remove deleted cases.

Each gate appends every Buck2 TestResult to
`~/.cache/hirsel-buck2/test-results.tsv`: `gate`, `target`, `shard`,
`duration_seconds`, `date` (UTC), and `status` (Buck2's numeric status).
The gate identifier is its unique event-log filename without `.pb.zst`.
Unsharded targets have an empty shard. This compact file survives the compressed
event logs' three-day rotation. The host selects the sink with
`HIRSEL_TEST_HISTORY=/path/to/results.tsv`, or disables it with
`HIRSEL_TEST_HISTORY=none`. The default file implementation locks concurrent
appends and flushes on close; it does not fsync each row. Tests can inject a
simulated sink into `event_logs.test_results`.

Cost-balanced binaries emit libtest's per-case `--report-time` output in normal
gates, including when their current graph needs only one shard. The launcher
enables the pinned harness's unstable timing option through `RUSTC_BOOTSTRAP=1`.
The TSV's additional `case_costs` column holds a JSON object of successful case
names to seconds (empty for results without timings). Refresh a table from real
gates without another measurement run:

```sh
python3 tools/buck2/test_costs.py --from-history --binary hirsel-rt-test-it
python3 tools/buck2/sync.py
```

`--binary all` merges every cost-balanced binary; `--since` restricts the
history and `--report-only` leaves tables untouched. The newest observation of
each successful case wins; unobserved cases retain their existing cost. Historical
rows without per-case timings cannot supply missing costs.

For a cheap report-time overhead probe, compile the existing profile harness
and run its single assertion case, interleaving the command below with the same
command plus `-Zunstable-options --report-time`. Use ten seeded pairs, hold
`cpu31-timing.lock` for the session, and record CPU 15's busy share from
`/proc/stat` around every sample:

```sh
tools/capped tools/buck2/toolchains/rust-files/bin/rustc --test tools/tests/test_tool_profile.rs -o /tmp/report-time-probe
tools/capped env RUSTC_BOOTSTRAP=1 taskset -c 31 perf stat -e cycles,instructions,task-clock \
  /tmp/report-time-probe debug_assertions_are_enabled --exact
```

Report shard medians and maxima over at least five distinct gates per shard:

```sh
python3 tools/buck2/test_costs.py --history --since 2026-10-19T20:00:00+00:00
```

Use `--history /path/to/results.tsv` for another file, `--binary <target>` for
another binary, or `--min-gates 1` to inspect early data. The report prints
partial data but exits unsuccessfully until every observed shard has enough
gates. Gate durations describe pool experience; they are not a cross-session
performance verdict. Keep before/after histories separate when shard counts change.

## Prelude overlay

The prelude cell is the one bundled in the pinned executable, expanded to
`.buck2/prelude` (`.buckconfig`: `[external_cells] prelude = disabled`) and
patched by `prelude_overlay.py`. Command wrappers use `/bin/bash` from the
pool image, so rustdoc can invoke its linker without a `PATH`. The LLVM tool
wrappers also locate their tree using shell builtins. It moves helper work off
the pool:

- `rust_library.bzl`: successful doctest verdicts are cached like libtests.
  Their executor requests one CPU and 512 MiB at the same priority as other
  tests, and a stable `TEST_TARGET` lets pool usage name them.
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
(`TEST_REQUESTS`, which `sync.py` derives from the evidence in
`test-sizes.json`) gives each binary, shared by
its shards, one CPU while total CPU seconds / total wall seconds is at most
1.6, otherwise two. A one-CPU action may burst to two (cpu.max = 2 × request).
A short CPU burst does not establish sustained parallelism. Four-CPU full-run
shards no longer have an exemption: agreement uses 384 smaller shards instead
of 96 long-lived four-CPU reservations. The launcher runs as many test threads
as the reservation's whole CPUs.

Memory reserves the largest measured process-group PSS peak times 1.2, rounded
up to 64 MiB, at least 128 MiB. The worker's twice-reservation kill threshold
is a safety mechanism, not a discount on the reservation. The 20% margin
covers PSS sampling jitter and moderate code growth; fine rounding avoids a
256 MiB rounding penalty on small binaries. Unmeasured native stages inherit
the largest freshly measured native sibling, explicitly recorded as a fallback.
Other binaries without a row retain the 1 CPU / 2 GiB conservative default.

The process data/address-space guard (HIRSEL_MEMORY_LIMIT) normally uses
that kill line, with a 1 GiB guard floor retained independently of PSS.
guard-stops.json records allocation limits including unused
thread stacks, which can far exceed resident PSS. Those now raise only the
guard limit (TEST_GUARD_LIMITS), independently from the reservation. Completed
memory-kill retries are sized from measured peaks, without preserving the
scheduler's geometric escalation overshoot forever. Unfinished memory kills
keep a conservative floor of twice their failed request times 1.2.

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
requests changed. Evidence-only changes run tool checks. Full-only stages run
in main's hourly run; a reservation update never starts a full native campaign.

`pool_usage.py` reports, read-only from the same trace and usage rows, the
pool's reserved CPU-hours by repository and hirsel's reserved against used
CPU-hours and GiB-hours per lane and target over the last N hours.

It also sums execution wall seconds and CPU seconds per action category,
excluding queue time. For rustc compile/link phases and target names, it joins
the shared retained Buck2 event logs. `tools/land` and `tools/main-runs` retain their
compressed `.pb.zst` logs, including failed runs, under
`$HIRSEL_BUCK2_LOG_DIR` (default `~/.cache/hirsel-buck2/event-logs`, shared across
worktrees). The full suite's helpers use the same recorder. Rotation keeps at
most three days and 4 GiB, oldest first; running writers and audit readers keep
their files locked until finished, so active logs may temporarily exceed the
size bound. A new run rotates completed logs after it finishes.

Select another archive with `--event-dir`, or add individual compressed or
JSONL logs with `--events`:

```sh
python3 tools/buck2/pool_usage.py --hours 6 --trace /tmp/trace-*.jsonl \
  --usage /tmp/usage-*.log --events /tmp/build.pb.zst
```

This reuses the compile-sizing tool's unique time/request match, separately for
each action category, so native observations and other helpers also get their
target names. Unmatched or ambiguous rustc measurements stay in `rustc (phase unknown)`. It also reports
repeated target/action-digest executions from retained compile and test events;
cache hits and copies of the same merged execution do not count as reruns.
The report states its log coverage; rotated history cannot be reconstructed.
Rustc's link
action includes compiling the executable, so its time is not linker-only.

## Measured compile requests

`action-sizes.json` keeps dev/release budgets per target, with phase timings,
CPU seconds and PSS memory peaks. `sync.py` generates `exec_sizes.bzl`
from it; compile rules select that target's platform. A dev compile splits its
code into 16 codegen units whatever its reservation, so LLVM spreads them over
the reserved CPUs, and one reserving more than one CPU runs rustc's front end on
that many threads (`-Zthreads`; `_dev_codegen` in `defs.bzl` has the
measurements). A release compile's codegen units match its reservation;
unmeasured release targets keep Cargo's defaults.
One target's metadata and codegen share the reservation.
Cargo profiles and dependency resolution remain in Cargo.toml.

The compile generator joins exported Buck2 commands to dispatches by action
digest and start time, then to worker CPU and scheduler PSS measurements by
operation. Only uniquely joined successful executions inside the explicit
measurement window count. Both first-party and third-party compiles get
per-target dev/release requests. A profile with no fresh executions inherits
the measured sibling profile's memory before using the generic default;
its CPU request remains one until measured in that profile. Missing or
ambiguous matches stay visible. Retries of one operation are one join candidate;
their largest PSS peak includes killed attempts. Refreshing an existing evidence
file retains unobserved targets and special classes, including on-demand native
observations.
Decode retained event logs with event_logs.commands and copy existing worker
logs and scheduler traces; do not launch a workspace build to collect data.
Run action_sizes_from_log.py with --commands, --pss, --usage, --since and
--until, writing action-sizes.json, then regenerate with sync.py.

CPU follows sustained use across the complete run: one CPU through 1.6 cores,
otherwise two. Old October 16 calibrations no longer pin four CPUs; a new
four-CPU request needs fresh evidence that it shortens the critical path.
Memory is maximum measured PSS times 1.2, rounded to 64 MiB, at least 128 MiB.
The unmeasured compile default is 1 GiB: new harnesses and generated targets
exceeded the former 256 MiB reservation repeatedly. There is no fixed
512 MiB anonymous-memory surcharge. special_requests in the same evidence
sizes precheck, Clippy and native observation from their own measured peaks.

Priority and lane attribution do not enter the action digest. Dependency
symlink trees are assembled in the daemon by the pinned prelude overlay,
without remote compile-sized helper actions. Clippy twins share ordinary
dependencies. The fast main run builds the same bounded workspace build set
as land, then builds test harnesses in its test stage; it does not build full
campaign dependencies through the root wildcard. Its pool priority remains 40.

`clippy.py` builds each target's Clippy twin `<name>-clippy`: the same rule on
its measured single-CPU platform, declared only under `-c hirsel.clippy=true`, so
Clippy never reserves a compile's request and `buck2 build //:` never
compiles the twins.

A lane's agreement run is the seed-1 sample's variants of its chosen cases:
the global seed-1 catalogue limited to its lowest `AGREE_LIMIT` (5000) ranks
(`sync.py`), in `gate-agree-selected-shard-<i>`. For the full campaign's
shards, `agree.py` checks unique shard identities, matching catalogue
metadata, sample coverage and classification/edit totals, and fails on any
cluster or absent shard.
`tools/main-runs` runs `agree.py --full`: a rotating sample of at most 96 variants per program over
`AGREE_FULL_SHARDS` (96) shards by the same stable rank, each under its own
`AGREE_FULL_TIMEOUT` (1800 s), so a slow or non-terminating variant fails one
named shard. The campaign has grown past what one 4-CPU action can finish
(286950 variants on 2026-10-16, about 33000 shard-seconds on the pool); each
shard prints its stage times, slowest variants and costliest programs to stderr,
and names any variant still running after 30 s.


Whole-campaign wrappers opt into the shared `//:native-selection` dependency
with `-c hirsel.native_selection=true`. It exports the frozen program cover
checked in as `native-tests/regressions/native-selection.json`, an
inclusion-minimal feature cover after historical pins. Each selected
baseline/generated program runs a rotating feasible-pair configuration cover;
topology partitions and named exclusion cells remain present. `--exhaustive`
on the campaign binary restores the previous exhaustive release check. The
campaign seed defaults to the full run's `PROPTEST_RNG_SEED`; `--seed N` makes a
run reproducible. The source/IR full agreement keeps all programs and budgets
96 mutations each; `--exhaustive` restores its complete catalogue.

The cover is frozen because deriving it observes every corpus program through
14 native builds (two targets by seven lowering options) with LLVM emission:
substantial work per program. The former 32 observation shards did not
finish within their time budget. Each observation now runs in a fresh process;
only feature strings and errors survive between cases. Compiler panics become
program errors; a worker crash fails collection. The `inventory` and
`observations-N` subtargets expose individual derivation stages. Loading the frozen cover adds, from the current tree, every
historical pin and every program the cover never observed, so new cases run
until the next derivation. `tools/bless.sh native-selection` derives it again
on the pool (`-c hirsel.native_selection=derive`: 256 observation shards of
about 10 programs within 1 CPU and 4 GiB each, then one merge) and writes the
file for review.

Direct `gate-native-full` and `gate-native-full-shard-N` targets have no
coverage-artifact dependency unless that setting is enabled, since
Buck2 fixes its build graph before it receives test-runner arguments. Explicit
`--filter` values are the program selection; an unfiltered whole-target run
reads the checked-in cover.

Native program selections use measured per-case costs from
`tools/buck2/test-costs/gate-native-{changed,cranelift-changed}.tsv`.
`python3 tools/buck2/native.py <ID>...` packs the slower backend's cost into 600-second slices; programs
without a measurement, or above that budget, run alone. Both backends run
every slice in separate pool actions. The helper uses exact `--case` IDs; the
CLI's `--filter` continues to match substrings. Each selected smoke or parity case runs
in a fresh process within its action, releasing compiler and interpreter
state before that worker takes another case. The existing memory cap and
1800-second action timeout still apply.

Workers print `native case program seconds peak_RSS_KiB`.
Import a Buck2 log with `python3 tools/buck2/test_costs.py <log>`, or refresh
from test history with `--from-history <history> --binary gate-native-changed`
(and `gate-native-cranelift-changed`). Partial selections retain timings for
unobserved programs. The times guide scheduling; they are not cross-session
performance comparisons.

## Real I/O ThreadSanitizer control

The opt-in `thread` platform instruments the runtime and its Rust dependencies,
using an instrumented std built by the exact compiler in `toolchain-lock.json`.
Prepare std in this checkout, then run the eight-worker control on the pool:

```sh
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=2 tools/capped python3 tools/buck2/prepare_tsan.py
.buck2/bin/buck2 test -c hirsel.sanitizer=thread --target-platforms //tools/buck2:thread //:gate-io-real-tsan
.buck2/bin/buck2 test -c hirsel.sanitizer=thread --target-platforms //tools/buck2:thread //:gate-cpu-pool-tsan
```

Proc macros and build scripts keep the ordinary host toolchain. The generated
`tools/buck2/toolchains/tsan-files/` is private to this checkout and never committed.
The sanitizer child's address-space guard is lifted for shadow mappings; its
normal pool RSS reservation still applies. The control refuses an uninstrumented
binary. `//:io-real-workers` is also a small file-I/O probe: arguments are worker
count (1/2/4/8) and writes per file (default 16, 64 KiB each, eight files total).
The CPU control runs the existing scoped-pool unit and race cases with fiber
annotations. It requires a conflicting-access positive control and executed
fiber switches under Free and Ordered. This does not instrument emitted mapper
instructions or certify either native emitter.

`test_requirements.bzl` declares exact CPU counts for harnesses whose assertions
need a physical width. These override measured CPUs and the focus clamp while
keeping measured memory and CPU class. The run's executor sends `cpu_count=N`,
`exact_cpus=N` and `cpu_affinity=exact-v1`. The scheduler must declare the last
property exact, and only upgraded Kiln workers advertise it. The launcher checks
that inherited affinity has N CPUs and the visible quota permits N; it refuses
a narrower grant rather than silently running fewer workers. Ordinary targets
continue using measured sizing. No runtime worker/topology override is involved.

Native CPU pool TSan qualification uses the same W4/W8 physical pool fixtures,
with instrumented emitted instructions on both backends and a separately
instrumented runtime, std and fiber annotations. The compiler keeps its normal
allocator. Prepare the private TSan std as above, then run:

```sh
.buck2/bin/buck2 test -c hirsel.native_tsan=true //:hirsel-llvm-unittest //:hirsel-cranelift-unittest -- --test-arg=native_tsan
.buck2/bin/buck2 test -c hirsel.native_tsan=true //:gate-cpu-pool-native-tsan-4 //:gate-cpu-pool-native-tsan-8
```

LLVM runs its ThreadSanitizer pass after optimization and runtime fast-path
expansion. Cranelift annotates each lowered memory access with its actual byte
range, including narrowed loads/stores and negative offsets, and refuses
uncovered memory instructions. The emitter controls call the generated code
from an unsanitized C harness: disjoint cells pass; shared cells must exit 66
with a race report. These controls do not substitute for a production job-claim
mutant. Instrumented pool fixtures use 3,000,000 loop iterations and retain the
same switch-free overlap assertion and probe cap. The x86 fiber switch annotates both ordinary fibers and naked CPU ahead
handoffs; only CPU ahead certificates are enabled in this sanitizer mode.
Each policy/emitter pool cell must independently establish overlap.

### Private CPU overlap audits

`tools/cpu-overlap --prepare /tmp/cpu-overlap-commands` builds the W4/W8/W16
Ordered audits, P16, and both native TSan gates, and materializes their existing
launcher commands and environments. It copies each mode's executables and std
products into that directory because Buck's sanitizer config reuses output
paths. Remove the prepared directory after the window. Prepare instrumented
std first with the
existing `tools/capped python3 tools/buck2/prepare_tsan.py` recipe. Builds happen
before the fenced window, outside the test scopes.

`tools/cpu-overlap --run /tmp/cpu-overlap-commands --cpus 0-7,16-23` runs those
commands in private delegated user scopes with exact effective cpusets and the
existing memory grants. It does not reserve CPUs. Every body checks ownership,
worker placement, samples and full overlap, then refuses P qualification with
`host-blocked: not exclusive` when another cgroup can run on those CPUs.
Failures remain failures. The three ignored Ordered bodies stay ignored because
libtest cannot report a dynamic host skip without counting the body as passed.

The explicit gate homes are
`tools/ignored-tests cpu_pool_4::ordered_allocating_mappers_overlap_and_replay`,
with `cpu_pool_8` and `cpu_pool_16` selecting the other widths. These run
`tools/cpu-overlap --only ordered-4|ordered-8|ordered-16` against prepared
commands, require exactly one passing body, and propagate host-blocked failures.
All three remain ignored in ordinary test runs. After preparation and fencing,
run each home as root with the original user's UID, for example:

```sh
sudo -n env HIRSEL_CPU_OVERLAP_COMMANDS=/tmp/cpu-overlap-commands \
  HIRSEL_CPU_OVERLAP_CPUS=0-7,16-23 HIRSEL_CPU_OVERLAP_SLICE=timing.slice \
  HIRSEL_CPU_OVERLAP_UID=1000 \
  tools/ignored-tests cpu_pool_4::ordered_allocating_mappers_overlap_and_replay
```

After fencing other top-level cgroups, the orchestrator can run
`sudo -n tools/cpu-overlap --run /tmp/cpu-overlap-commands --cpus 0-7,16-23 --slice timing.slice --uid 1000`.
Each command gets its own system scope in the named top-level slice and drops
to that user. No daemon or build shares the scope. The sixteen logical CPUs
may include SMT siblings; no sibling-idleness or speedup claim is made. The
runner has a fifteen-minute total deadline and never changes other slices.
Ownership comes from `cpuset.cpus.effective`; disjoint effective sets outside
the job's ancestry, plus no resident userspace processes in its ancestors,
prove exclusivity independently of
kernel partition-root status. Root kernel threads remain subject to the
switch-free interval check. Unreadable branches refuse qualification.

The fence must also exclude the owned CPUs from empty top-level cgroups such
as `dev-hugepages.mount`, whose inherited effective sets otherwise intersect.
TSan controls remain mandatory. A failed or fifteen-second hung mutant is
recorded before the production placement audit continues; the body still
fails. The existing 180-second native TSan gate deadline remains unchanged.
