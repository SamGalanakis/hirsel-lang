# Building Hirsel

Hirsel is a systems-level, general-purpose language whose primary user is a
coding agent. Its fixed goals:

- native performance;
- every definition checked only against the signatures it uses;
- one way per job;
- all authority passed as parameters (capabilities);
- reference counting with no lifetime variables;
- deterministic behaviour outside capabilities.

## Layout

- `docs/spec/` is the specification, starting at `docs/spec/index.md`. Rule IDs
  such as `OWN-6` are stable, each is defined once as a `### ID: Title`
  heading, and every diagnostic cites one. `docs/spec/syntax.md` is the grammar.
- `crates/` is the Cargo workspace: lexer, parser, checker, typed IR, the
  interpreter, the reference-counting pass, the LLVM backend, the runtime and
  the driver. Each crate's `README.md` lists its invariants.
- `conformance/` is the rule-ID conformance corpus: one `.hir` program per
  case, named after the rule it exercises.
- `native-tests/` holds native-only fixtures (heap, layout, replay, topology)
  and the native exclusion list.
- `stdlib/` and `packages/` are Hirsel library sources and package fixtures.
- `proofs/rc/` is the Lean model of the reference-counting protocol.
- `bench/native/` is the native benchmark harness and its programs.
- `tools/` holds the Buck2 build (`tools/buck2/`), the pinned LLVM toolchain
  (`tools/native-toolchain/`) and gate scripts.

## Toolchains

Rust is pinned in `rust-toolchain.toml` (1.99.0) and installed by rustup.
The native backend needs LLVM 21.1.8 (`tools/native-toolchain/llvm-21.lock`);
`.cargo/config.toml` points `LLVM_SYS_211_PREFIX` at `/usr/lib/llvm-21`
unless the environment already sets it. `tools/native-toolchain/README.md`
installs the pinned tree. Native tests carry pinned qemu, an AArch64 cross toolchain and Valgrind as
Buck2 inputs; only the Lean checks need a host installation.

The build caps itself for a shared host: `.cargo/config.toml` limits Cargo
to 4 jobs and sets `RUST_TEST_THREADS` and `HIRSEL_JOBS` (in-process worker
threads) to 4. Export them explicitly when a wrapper might override them:

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
```

## Cargo

Cargo needs no build service:

```sh
cargo build --workspace
cargo test -p hirsel-lir
cargo run -p hirsel-conformance -- --verbose
```

`hirsel-conformance` reports counts by rule prefix; `--json` gives tool
output. It finds the repository from the current directory; `--root <repo>`
selects another checkout. `--update-passing` (run on this host by `tools/bless.sh passing`) adds newly
passing tests to `conformance/passing.txt` without removing existing entries.

## Buck2

The gate builds and tests with Buck2. Cargo manifests and `Cargo.lock` stay
the source of truth: after editing a `Cargo.toml`, run
`python3 tools/buck2/sync.py` to regenerate the BUCK files (`tools/gate`
fails while they are stale). The toolchains (Rust, the pinned LLVM, Buck2 and
its prelude) come in hermetically; nothing is installed system-wide.

```sh
python3 tools/buck2/prepare.py                  # once per checkout
.buck2/bin/buck2 build //:
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
tools/gate                                      # check the finished change
tools/land                                      # land it on main
```

Without further configuration every action runs on this host. Buck2 can
also use an optional remote-execution pool; its endpoint, credentials and
platform properties live only in the gitignored `.buckconfig.local`
(`tools/buck2/README.md`). `-c hirsel.tests=host` explicitly opts into running tests on the invoking
host; native tools remain pinned inputs in either mode. More target names and the filtered native commands are in
[tools/buck2/README.md](../tools/buck2/README.md).

## Gates

`tools/gate` checks a finished change, once. It diffs the worktree against
its merge base with `origin/main` and runs what the changed paths can affect
(the table is `tools/buck2/impact.py`):

- documentation no build reads (`*.md`, `LICENSE`, `docs/`): its added links
  resolve, links into deleted paths are gone, and no whitespace errors;
- the gate's own tooling under `tools/`: its tests and the build-file check;
- anything else: every Buck2 test with a changed input, directly or through
  any dependency, plus the corpus campaigns the change can affect. A file a
  campaign reads (the corpus, the stdlib, native fixtures) selects it, and so
  does a crate's library source: front-end, validator and interpreter crates
  select conformance and the source/IR checker agreement sample (seed 1),
  backend, runtime and LLVM crates the native smoke, the shared contract
  crates, the engine and the driver all three. Test code selects only its
  tests. A build-wide input (`Cargo.toml`, `.buckconfig`, `tools/buck2/`,
  `third-party/`) selects every test and campaign;
- whenever Buck2 runs, the workspace build and Clippy too.

All of it is one Buck2 invocation, so the pool runs the build, Clippy, the
tests and the campaigns concurrently, while formatting and the build-file
check run beside it on this host. The summary lists every target; a failure
names its failing cases and the command that reruns each alone. A passing
change is recorded by its identity, the patch-id of the lane's own diff
without context lines and the crates it touches: after main moves and the
change rebases cleanly, `tools/gate` only builds and checks formatting.
With `HIRSEL_GATE_SLOTS=dir:N` set, at most N gates on the host run their
Buck2 work at once; a gate past that waits for a slot in dir and says so.

`tools/land` lands the committed change: fetch, rebase onto `origin/main`,
then the workspace build, Clippy, formatting and the build-file check on the
rebased tree, and push, repeating when main moved. It never runs tests: it
refuses a change `tools/gate` has not passed, and stops on a rebase conflict
(resolve it, gate the result, land again).

Changes to native code also run the filtered native matrix for the programs
they add or touch, one native campaign at a time across lanes:

```sh
flock /workspace/code/hirsel-briefs/locks/native-campaign.lock \
  .buck2/bin/buck2 test -c hirsel.full=true //:gate-native-full -- --test-arg=--filter --test-arg=<program> --timeout 14400
```

Tests that execute the full native matrix or take about a minute or longer
carry `#[ignore = "full gate"]` and a `full_gate_` name prefix. Keep their
inputs and assertions intact; add a separate bounded smoke test when useful.
Cases that call `full_matrix` only for discovery are skipped by name outside
the full suite (`FULL_GATE_SKIPS` in `tools/buck2/sync.py`).

`tools/gate --full` is the full suite, run on demand (nothing schedules it):
every test, the unrestricted source/IR and native agreement, release tests,
the `full_gate_` tests in both debug and release, the full native matrix and
every `tools/check-native.sh` runtime, sanitizer, toolchain and Lean check. It
and `tools/check-native.sh` run every stage even after one fails and fail at
the end. `tools/main-full-run.sh` runs it on a fresh detached worktree of
`origin/main` and writes `latest.json`. `tools/main-stall-alarm` appends to
`alarms.log` beside it when main gets no commit for 40 minutes while lane
worktrees are moving; run it from whatever loop watches the lanes.
