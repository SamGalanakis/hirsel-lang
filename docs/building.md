# Building Hirsel

Browse the [full signature index](api.md) for every public core and std declaration.

New to the language? Start with the [runnable language tour](tour.md), then
try the [runnable tools](../examples/README.md).

Hirsel is a systems-level, general-purpose language whose primary user is a
coding agent. Its fixed goals:

- native performance;
- every definition checked only against the signatures it uses;
- one way per job;
- all authority passed as parameters (capabilities);
- reference counting with no lifetime variables;
- deterministic behaviour outside capabilities.

## Build and launch a program

```sh
hirsel build hello.hir -o hello                 # LLVM O2, production runtime
hirsel build app/manifest.json --release -o app # LLVM O3
hirsel build hello.hir --debug -o hello-debug   # O0 with DWARF source line tables
hirsel build hello.hir --target aarch64-linux -o hello-arm64
```

`build` accepts a source file (and its `.hir.json` sidecar, when present) or a
launch manifest. The default target is `x86_64-linux`; `aarch64-linux` uses the
pinned cross compiler and runtime archive. The executable contains the production
runtime and compiler support libraries statically; its only shared dependencies are
libc and the platform ELF loader. It runs independently of Hirsel and the source checkout.

Grants are supplied at launch using schema-1 RunConfig fields. A grants template
omits `schema`, `program`, `entry` and `launch`: the launcher binds those to the
selected program. For an entry receiving `Io`, `grants.json` can grant only stdout:

```json
{"io":{"stdout":"stdout.bytes@1","dirs":[]}}
```

```sh
hirsel run hello.hir --grants grants.json -- argument-one argument-two
./hello --hirsel-grants grants.json argument-one argument-two
```

`--grants` and `--run` accept inline JSON when the argument starts with `{`;
otherwise the argument is a path. Built executables spell these flags
`--hirsel-grants` and `--hirsel-run` so programs keep their own flag space:

```sh
hirsel run hello.hir --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]}}'
./hello --hirsel-grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]}}'
```

Inline templates require absolute directory hosts. Templates may supply `inputs`, `io`, `args`, `env`, `unsafe` and `admission`.
Omitted fields deny authority and use zero seeds. Binding fields, unknown fields,
duplicate fields and invalid grants refuse launch. Templates use the ordinary
RunConfig validation and admission path; selecting code grants no authority.
For `hirsel run --grants`, arguments after `--` replace the template's `args`
with the exact UTF-8 snapshot, including an explicitly empty snapshot. Without
`--`, the template's `args` remain. With `hirsel run --run`, the record's `args`
are authoritative and trailing arguments are an error. Without either option,
`hirsel run program.hir -- args...` grants only Args with exactly those values,
including an explicitly empty snapshot. No other authority is implied.

Interpreted host runs (`--run` and `--grants`) have no default step budget;
simulated runs keep a 1,000,000,000-step budget. `--max-steps <n>` selects a
nonnegative u64 bound, and `--max-steps unbounded` removes it. These are tooling
limits, independent of language semantics; `--native` rejects the option.

For a record pinned to a particular semantic program revision, export its binding,
edit its grants, and select it explicitly:

```sh
hirsel export --run hello.hir -o /absolute/path/run.json
./hello --hirsel-run /absolute/path/run.json argument-one argument-two
```

To read files, add a directory mount to `io.dirs`, for example
`{"path":"data","host":"/absolute/host/directory","access":"read"}`.
A program can acquire `io.dir(rt, path.of("data"))` and read relative paths within
that grant. A relative `host` in a grants template resolves against the template
file's directory and is canonicalized before validation and host pinning. Bound
records still require canonical absolute hosts. Use `read_write` only when writes are intended. No working directory,
standard stream, environment key, clock or network authority is implicit.

To let a program run other programs, grant each one by its exact absolute path:
`"process":{"provider":"process.spawn@1","rules":[{"program":"/usr/bin/git","env":["HOME"]}]}`
in `io`. `io.process(rt)` then projects a `Proc`, and `proc.run` runs `/usr/bin/git`
with an empty environment except `HOME`; any other program is `NotGranted()`. A granted
program can do whatever the host lets it do. A run bounded by a `Deadline` on a granted
monotonic clock is killed at expiry and returns `TimedOut()`; with `io.tasks` it parks
only its task, and cancelling that task kills the child.

When supplied as a path, the run record path must be absolute; a template path may be relative. The launcher validates the record, checks
its semantic program revision, entry and launch kind, checks required roots and
admission conditions, then pins host grants before entry. The same binary can
accept different grants and seeds on subsequent launches. Unknown or unsupported
policy refuses launch. Environment variables never supply a grant fallback.

Arguments after `--hirsel-run <path>` or `--hirsel-grants <path>` replace the record's `args` with the exact
UTF-8 process snapshot, including an empty snapshot, before admission. A record
may therefore omit Args. Without a run record, `./program -- args...` explicitly
grants only Args. `./program` grants no external roots: a root-free entry can
run, and an entry requiring Io, Args, Env or Unsafe refuses before entry. Invalid
UTF-8 arguments refuse launch. These snapshots contribute to run identity.

A directly run executable exits 0 for `Ok(())`, 1 for a returned `Error`, 2 for a
language failure and 3 for a refused launch. On an x86-64 host, test an AArch64
artifact with the pinned qemu and cross sysroot:

```sh
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" ./hello-arm64 \
  --hirsel-run /absolute/path/run.json
```

## Run a package's tests

```sh
hirsel test examples/calculator/hirsel.json
hirsel test app/manifest.json parser decode      # match either substring of the stable path
hirsel test app/manifest.json --seed 42 --json
hirsel test app/manifest.json --native
hirsel test app/manifest.json --native --backend cranelift
```

Test manifests may omit `entry`; no launch main is needed. Mark ordinary functions
with `@test`. They may be private, take no parameters or
one borrowed `Test`, and return `()` or `Result[(), E]`. Generic functions, other
parameters and other results are rejected. The command checks the whole package,
then selects only marked functions in its manifest sources, never dependencies.
Filters match substrings of versioned definition paths; multiple filters are ORed.
An empty selection succeeds with zero tests. Tests run in stable path order.

Every test gets a fresh Test context and fake providers, with no host grants.
Its default seed depends only on its stable definition path, so filtering or
moving the definition between files does not change it. `--seed` supplies the
same u64 seed to every selected test. Failures (including Error results and
runtime checks) run normal cleanup and do not prevent later tests from running.
The runner prints each test's name, outcome and seed, then totals; exit 1 means
at least one failed. Native mode builds one executable for the package and
launches it separately for each selected test, with the same output and seeds.

`--json` writes one schema-1 object with `tests` and `summary`. Each test has
`name`, `seed`, `status` (`passed` or `failed`) and `failure` (null on success).
The summary has `passed`, `failed` and `total`. Package checking errors use the
ordinary check diagnostics and prevent execution.

## Record and replay a run

A host run can stream every capability input it received (file, stdin and Env reads,
socket exchanges, clock samples and the outcome of every write) to a tape, and the tape
replays that run deterministically with no grants and no host effects
([DET-1](spec/determinism.md#det-1-contract)). Capture a failing run, then replay it as
often as needed, after the files, stdin and peers are gone:

```sh
hirsel run app.hir --grants grants.json --record failing.tape < input.txt
hirsel run app.hir --replay failing.tape              # same stdout, stderr and exit
hirsel run app.hir --replay failing.tape --native     # the same tape, compiled
```

Executables take the same inputs: `--hirsel-record <tape>` precedes their launch input,
and `--hirsel-replay <tape>` stands alone:

```sh
./app --hirsel-record failing.tape --hirsel-grants grants.json argument-one
./app --hirsel-replay failing.tape
```

`--record` combines with `--run` or `--grants`; `--replay` takes neither, because the tape
binds the canonical run record and admission time it was recorded with. The tape is one
format for both engines: a run recorded interpreted replays compiled and the other way
round. Replay refuses before entry a tape bound to a different semantic program revision,
entry or root set, so editing a function body invalidates its recordings; while it
runs, a request that differs from the next recorded one, a missing event or an unconsumed
event refuses it after the output it confirmed. A tape cut short (no `complete` trailer)
never replays. The tape path must lie outside every granted directory. Tapes hold the run
record, including granted Env values, every byte read or written and every
`random.system@1` draw: treat them as secrets.

## The std precheck

std is part of the toolchain, so it is checked once, when the toolchain is built,
rather than by every program that uses it. `hirsel precheck` checks a package
completely, as the only dependency of an empty program, and writes its *precheck*:
each definition's lowered, validated TIR, semantic-revision node and completed
native source audit, with compile-time substitutions and constant values. Native
builds reuse these products without repeating H and modes for std.

Build and install the release toolchain with Buck2 (from the repository root):

```sh
bundle="$(.buck2/bin/buck2 build --target-platforms //tools/buck2:release \
  //:hirsel-toolchain --show-full-simple-output)"
install -d "$HOME/.local/bin"
install -m 755 "$bundle/hirsel" "$HOME/.local/bin/hirsel"
install -m 644 "$bundle/hirsel-std.precheck" "$HOME/.local/bin/hirsel-std.precheck"
```

`//:hirsel-toolchain` produces a directory containing both files. Its executable
finds its sibling precheck immediately, including after installation. Plain
`cargo build` users create that sibling with one command from the repository root
(use `target/release/hirsel` after `cargo build --release`):

```sh
target/debug/hirsel precheck stdlib/std@1/hirsel.json -o target/debug/hirsel-std.precheck
```

`hirsel` loads `$HIRSEL_PRECHECK`, or else `hirsel-std.precheck` beside its own
executable. A program whose std matches the precheck skips checking std: with a
release CLI, a hello world that uses std reaches `main` in about 0.5 s instead of
4 s. A precheck is keyed by the compiler's identity (a digest of the sources of
every crate the compiler is built from, `crates/hirsel-driver/build.rs`), std's
sources and manifest, and the compile-time limits. Any difference means it is
not used and std is checked from source as before, so a stale precheck only costs
time; rebuild it after rebuilding `hirsel` or editing std. `--verbose` (any
command) says on stderr why a dependency was checked from source. `hirsel
precheck` refuses a package with any diagnostic. Diagnostics, semantic program
revisions and replay identity are the same with and without a precheck. Buck2
test targets that run std programs get `HIRSEL_PRECHECK` from
`//:hirsel-std-precheck`, which rebuilds whenever std or the compiler changes;
`HIRSEL_PACKAGE_CACHE=off` checks everything fresh.

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

## Fast loop

While iterating, run the one test that shows the change, by name, then the
same test through the heaviest crate that reads it, and gate once at the end:

```sh
T='-c hirsel.tests=focus'
.buck2/bin/buck2 test $T //:hirsel-types-unittest -- --test-arg=<case> --test-arg=--exact
.buck2/bin/buck2 test $T //:hirsel-driver-test-it -- --test-arg=<case> --test-arg=--exact
.buck2/bin/buck2 kill                           # when done: cancels what it still has queued
```

A crate's own tests are `//:hirsel-<crate>-unittest` and `-test-it`; source
compiled through the driver is tested in `//:hirsel-pipeline-tests-test-front`
and `-test-rc`. Focus books one CPU per test run, so it starts at once where a
whole-shard request waits for a worker to free its CPUs (median edit-to-result
38 s against 114 s over twelve paired edits). Leave `-c hirsel.full=true` and
`tools/gate` for the finished change.

Run one corpus program, including its expected outcome, checked execution and
canonical formatter check, on the pool:

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus //:gate-conformance-shard-1 -- \
  --test-arg=--shard --test-arg=1/1 \
  --test-arg=--path --test-arg=conformance/SEM/SEM-1-format-into-appends-run.hir
```

`--path` matches repository-relative prefixes, may repeat, and fails if no files
match. A complete filename selects that program. The explicit `1/1` overrides
the target's default shard so that a selected file always runs. The runner
checks only matched files and their ratchet entries. Every match must meet its
expected outcome, including new files absent from `passing.txt`; filtered runs
cannot update that file.

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

After those checks, land runs the built `hirsel doc --index` and commits a
changed `docs/api.md` separately as `Regenerate the API index.` before pushing.
This generated-only update needs no new gate; a push retry regenerates it
after the next rebase. The export-count and drift test stays in the gate.
Declarations picked up by a clean rebase therefore reach main with a current
index without another manual refresh or gate. Resolve an existing index
conflict by regenerating.

Changes to native code also run the filtered native matrix for the programs
they add or touch on the pool. Focus runs book one CPU at lane priority:

```sh
.buck2/bin/buck2 test -c hirsel.full=true -c hirsel.tests=focus //:gate-native-full -- --test-arg=--filter --test-arg=<program>
```

A change to LLVM emission must carry the matching Cranelift change.

Changes to `crates/hirsel-llvm`, `hirsel-cranelift`, `hirsel-lir`, `hirsel-rc`
or the runtime run the same changed-program filters through the LLVM native
smoke matrix and Cranelift parity in `tools/gate`. With no changed runnable fixtures,
both use the bounded native smoke list.

Changes to `crates/hirsel-cranelift` also run the Cranelift parity gate for the
programs they touch: each case LLVM passes (unoptimized, diagnostic archive) must
agree with the reference as a Cranelift executable, and every case that does not is
a row of `native-tests/regressions/cranelift-parity.tsv` with its reason. A failure
without a row, a changed reason and a row whose case passes all fail. `--clif`
first prints each selected case's Cranelift IR. The per-push native smoke runs the
cases of `native-tests/regressions/cranelift-smoke.txt` as Cranelift executables.

```sh
.buck2/bin/buck2 test -c hirsel.full=true -c hirsel.tests=focus //:gate-native-cranelift -- --test-arg=--filter --test-arg=<program>
```

The whole parity set runs as `//:gate-native-cranelift-shard-1` to `-8`, each under
the pool's action time limit, and as AArch64 executables under qemu as
`//:gate-native-cranelift-aarch64-shard-1` to `-8` (`--test-arg=--aarch64` on a filtered
run; its list is `cranelift-parity-aarch64.tsv`).

`native --cranelift-times [--filter TEXT]...` prints each case's emission wall time
under Cranelift, LLVM O0 and LLVM O2, and the totals.

Tests that execute the full native matrix or take about a minute or longer
carry `#[ignore = "full gate"]` and a `full_gate_` name prefix. Keep their
inputs and assertions intact; add a separate bounded smoke test when useful.
Cases that call `full_matrix` only for discovery are skipped by name outside
the full suite (`FULL_GATE_SKIPS` in `tools/buck2/sync.py`).

`tools/gate --full` is the full suite, run on demand (nothing schedules it):
every test, budgeted source/IR and native agreement, release tests,
the `full_gate_` tests in both debug and release, the derived native program and pairwise configuration covers and
every `tools/check-native.sh` runtime, sanitizer, toolchain and Lean check. It
and `tools/check-native.sh` run every stage even after one fails and fail at
the end. `tools/main-full-run.sh` runs it on a fresh detached worktree of
`origin/main` and writes `latest.json`. `tools/main-stall-alarm` appends to
`alarms.log` beside it when main gets no commit for 40 minutes while lane
worktrees are moving; run it from whatever loop watches the lanes.
