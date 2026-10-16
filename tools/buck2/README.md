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
.buck2/bin/buck2 build //:                        # every first-party target, dev profile, on the pool
.buck2/bin/buck2 test //...                       # every test
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir-test-it-host //:hirsel-lir           # one crate, including host cases and doctests
.buck2/bin/buck2 test //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-rc-shard-2  # one shard of a sharded test binary
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-pipeline-tests-test-rc-shard-1   # run tests on this host
.buck2/bin/buck2 test //:gate-native-smoke          # one tools/check.sh corpus stage
python3 tools/buck2/conformance.py                # conformance shards and summed counts
python3 tools/buck2/agree.py                      # AGREE_LIMIT variants over AGREE_SHARDS shards, summed JSON
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-conformance-bin-native
python3 tools/buck2/changed_tests.py              # the tests these changes reach
python3 tools/buck2/sync.py                       # after metadata or action-sizes.json changes
tools/check.sh                                       # the per-push gate
```

Builds use Buck2's default isolation dir, `v2`: never pass
`--isolation-dir` (or pass only `--isolation-dir v2`) and leave
`BUCK_ISOLATION_DIR` unset. Each worktree already has its own `v2` daemon.
The isolation dir is part of every action's output paths, so an action run
under another one has a different key and misses the pool's shared cache. A
private isolation dir is only for inspecting the graph beside a running
command, or for escaping a stuck daemon; anything it builds starts cold.

Arguments after `--` go to Buck2's test runner: `--test-arg=<a>` passes one
libtest argument, and `--timeout <seconds>` raises the ten-minute limit per
test. A sharded binary given a case filter runs it all in shard 1. Check the output
for a nonzero case count. Host and pool targets intersect the caller's selection
with their declared partition before sharding; even `--exact` cannot put a
host case on the pool. Host filters are metadata, never extra OR-ed libtest
arguments. Target names come from Cargo; `-unittest` and
`-test-it` run unit and integration tests. Add `-test-it-host` for a crate's
declared host-only cases and the library target for declared doctests, as in
the LIR example. For a sharded binary, select every `-shard-<i>` target.

Heavy Buck2 commands run from this host and execute on the pool when one is
configured. Native changes use a filtered campaign:

```sh
.buck2/bin/buck2 test -c hirsel.full=true -c hirsel.tests=host //:gate-native-full -- --test-arg=--filter --test-arg=SEM-4-view-return-source-run --timeout 14400
```

`-c hirsel.tests=host` changes test execution only; rustc stays on the pool.
Use it for native tests needing qemu, the AArch64 linker or Valgrind, for example:

```sh
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-pipeline-tests-test-rc-shard-1 -- --test-arg=rc_library_ --test-arg=--include-ignored
```

The scheduled full gate selects host execution for the full native matrix,
native agreement, sanitizer and hash sweep. Lean and the runtime C harness
also run on the invoking host. Keep those tools installed there; the pinned
Rust/LLVM inputs are bootstrapped by `prepare.py`.

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
| its cases that need the host (`host_cases` in `sync.py`) | `//:hirsel-x-test-it-host` |
| a `tools/check.sh` corpus or campaign stage | `//:gate-<stage>` (label `gate`) |
| a conformance shard (`CONFORMANCE_SHARDS`) | `//:gate-conformance-shard-<i>` |
| an agreement shard (`AGREE_SHARDS`) | `//:gate-agree-shard-<i>` |
| an unrestricted agreement shard (`AGREE_FULL_SHARDS`, `--full` only) | `//:gate-agree-full-shard-<i>` |

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
`native --full --bless` through the host-only `gate-native-full`.

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
- Full-gate crate cases (`full_gate_` functions) are automatically added to
  `host_cases` when their source module uses `full_matrix`, an AArch64 target,
  the cross linker, qemu, Valgrind or Lean. The complete module is scanned so
  module-local campaign helpers count; full-gate siblings (including x86-only
  controls) conservatively share host routing. An opaque external helper can
  declare this with `#[ignore = "full gate: host tools"]`. Existing ordinary
  host cases stay in `HOST_CASES`. Adding a new full-gate case or host-tool use
  changes the generated `BUCK`, so `sync.py --check` rejects stale routing.
  `tools/tests/buck2_sync.py` exercises discovery in the fast gate.
- A pool test's passing verdict is cached by its action key. Test binaries
  whose sources draw random cases (`proptest!`, `SEEDED` in `sync.py`) run
  under `PROPTEST_RNG_SEED` from `-c hirsel.test_seed` (default 1), which is in
  that key: a cached pass covers its seed only. `tools/check.sh --full` draws a new
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
  caller filters, then partitions by host declarations and applies sharding.
  The `full_gate_ --ignored` group also emits the unpartitioned compiled case
  inventory. `full_gate_counts.py --run <command...>` streams Buck2's log and
  checks every `(binary, case)` appears exactly once, rejecting missing,
  duplicate, failed or unexpected executions. Both full-gate crate stages in
  `tools/check.sh --full` use it independently; `full_gate_counts.py <log>` checks a
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
  `cpu_class=fast`; other tests and gate shards run on any worker. Compile platforms queue at priority 100 and test
  platforms at 0, so an edit's compiles
  overtake campaign shards; the priority is not part of an action key. The
  platforms also include plus the invoking host for tests labelled `host`: cases that run
  Cargo, and the stages that need qemu-user, the AArch64 cross toolchain or
  Valgrind, which the pool image lacks (`-c hirsel.tests=host` runs every test
  there).
- Profiles (`profile.bzl`): the default target platform `//tools/buck2:dev` is
  Cargo's dev profile (third-party crates at opt-level 1, and the members a
  root `[profile.dev.package.<name>] opt-level` names at that level);
  `//tools/buck2:release` is `[profile.release]`.

## Cost-balanced test shards

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
patched by `prelude_overlay.py`. It moves helper work off
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
bundled`, and delete `prelude_overlay.py`, its pins, its test with its `tools/check.sh` stage, and
`prepare_prelude` in `bootstrap.py`.

## Measured test requests

A pool test binary reserves what it measured using, not a fixed 4 CPUs and
4 GiB: `test_sizes.bzl` (`TEST_REQUESTS`, evidence in `test-sizes.json`) gives
each binary, shared by its shards, one CPU while its p95 core use is at most
1.6 (a one-CPU action already runs up to the worker's slot share), otherwise
`clamp(ceil(p95), 2, 4)`, and its anonymous memory peak (p99) times 1.5,
rounded up to 256 MiB, at least 512 MiB. The launcher runs as many test
threads as the request's whole CPUs. A binary without a row reserves 1 CPU and
2 GiB. The pool workers' usage logs tag each test run with its `TEST_TARGET`;
copy each worker's `actions.log` and `actions.log.1`, then:

```sh
python3 tools/buck2/test_sizes_from_usage.py /tmp/usage-*.log
```

A request is part of the test's action key: resizing reruns those tests once.

## Measured compile requests

`action-sizes.json` keeps dev/release budgets per target, with phase timings,
CPU seconds and anonymous memory peaks. `sync.py` generates `exec_sizes.bzl`
from it; compile rules select that target's platform and set `-Ccodegen-units`
to its CPU reservation. Unmeasured targets keep Cargo's codegen defaults.
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
Codegen units match the request, and one-CPU targets keep two. A probe's old request is evidence,
never a reservation floor. Schedulability on the shared pool beats peak
parallelism: smaller requests can start without a whole eight-CPU slot.
Memory keeps its measured headroom. The script documents memory headroom and sample
handling. Requests participate in action keys, so resizing deliberately
invalidates those actions.

A compile measured under one CPU reads as using one core, so the rule never
grows it. The critical-path compiles in `CPU_CLASSES` are calibrated instead:
`-c hirsel.probe_cpus=8` gives each of them eight CPUs and codegen units for
that invocation, `action_sizes_from_log.py --merge` sizes their rows from that
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
`tools/check.sh --full` runs `agree.py --full`: the whole seed-1 catalogue over
`AGREE_FULL_SHARDS` (96) shards by the same stable rank, each under its own
`AGREE_FULL_TIMEOUT` (1800 s), so a slow or non-terminating variant fails one
named shard. The campaign has grown past what one 4-CPU action can finish
(286950 variants on 2026-10-16, about 33000 shard-seconds on the pool); each
shard prints its stage times, slowest variants and costliest programs to stderr,
and names any variant still running after 30 s.
