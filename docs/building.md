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

## Document a package

```sh
hirsel doc app/manifest.json                 # Markdown on stdout
hirsel doc app/manifest.json -o app/api.md    # write a page
hirsel doc app/manifest.json --json          # schema-1 JSON
hirsel doc stdlib/std@1/hirsel.json -o docs/api.md
```

`doc` accepts an explicit package manifest, including a library without an entry.
It lists every definition visible to other packages under PROG-2, including
transparent constructors, and keeps opaque representations and `internal` modules
hidden. Signatures retain modes, brands and sibling origins, relations, `uses`,
contracts and attributes; attached `///` comments keep their full text and
paragraphs. Bodies and constant initializers are not checked or evaluated.
Documentation examples are not executed.

Definition anchors encode exact package/version/module/item paths, so moving a
source file or reordering declarations preserves links. JSON contains `schema: 1`
and an `entries` array with `identity`, `anchor`, `name`, `declaration`,
`description` and resolved `links` (`identity`, `name`, `url`). Markdown uses the
same anchors and references. Core and std references link to the published
[API page](api.md); other dependency references use
`../<package>@<version>/api.md#<anchor>`, so publishing package pages in sibling
versioned directories keeps those links intact. `-o` also accepts JSON output.
Std's manifest publishes core registry entries and std declarations together
through this same renderer.

## Build and launch a program

```sh
hirsel build hello.hir -o hello                 # LLVM O2, production runtime
hirsel build app/manifest.json --release -o app # LLVM O3
hirsel build hello.hir --debug -o hello-debug   # O0 with DWARF source line tables
hirsel build hello.hir --debug --backend cranelift -o hello-clif
hirsel build hello.hir --target aarch64-linux -o hello-arm64
```

`build` accepts a source file (and its `.hir.json` sidecar, when present) or a
launch manifest. The default target is `x86_64-linux`; `aarch64-linux` uses the
pinned cross compiler and runtime archive. The executable contains the production
runtime and compiler support libraries statically; its only shared dependencies are
libc and the platform ELF loader. It runs independently of Hirsel and the source checkout.

Debug builds from both LLVM and Cranelift emit DWARF 5 source line tables on
x86-64 and AArch64. Cranelift uses final machine source ranges and emits compile
unit and subprogram entries with relocated code addresses. Both backends prefer
an operation's source or cleanup anchor, falling back to its enclosing block;
LIR does not retain a separate span for every scalar instruction. Generated
Cranelift helpers and thunks have no source rows. These are line tables, without
local-variable descriptions.

```sh
gdb -batch -ex 'break hello.hir:3' -ex run ./hello-clif
llvm-dwarfdump --verify ./hello-clif
llvm-dwarfdump --debug-line ./hello-clif
```

Use an executable source line for the breakpoint. Cross-target line tables can
be inspected with the same `llvm-dwarfdump`; running AArch64 needs qemu or an
AArch64 debugger. The CLI regression compares LLVM and Cranelift line sets per
function for straight-line code, branches and loops on both targets, and checks
address-to-source lookup through `llvm-dwarfdump --lookup`.

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
including an explicitly empty snapshot. No other authority is implied. A bare
`hirsel run program.hir`, in either engine, admits the empty-grant record: a root-free
`main()` runs, and an entry taking `Io`, `Args` or `Env` is refused at launch
(`{"kind":"not_granted","root":"Io"}`, exit 1), with a line naming `--grants` and
`--simulation` (a simulated world, interpreted only).

Interpreted host runs (`--run` and `--grants`) have no default step budget;
simulated runs and runs without `--run` or `--grants` keep a 1,000,000,000-step budget. `--max-steps <n>` selects a
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
hirsel test app/manifest.json --bless             # accept the actual snapshots
hirsel test app/manifest.json --bench --bench-time 500
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

Interpreted tests run under a budget of 10^9 interpreter steps each, the `run`
default; `--max-steps <n|unbounded>` replaces it, and a test that exhausts it fails
with a tooling limit. Native tests have no step budget.

A test reports through its stdout, one escaped line per record (see
[core-library.md](spec/core-library.md)): `test.log` text, the `test failure: seed=S
case=C` line of a failed case check, and `test.expect_snapshot` records. Under each
failed test the runner prints its report, indented, with log text unescaped, so
`test.assert_eq` shows both values and a failed property shows its shrunk
counterexample followed by the seed and case that `--seed` replays. A passing test's
report is not printed.

### Snapshots

`test.expect_snapshot(t, name, text)` compares `text` with the committed file
`snapshots/<file>.<test>.<name>.snap` beside the test's source file, where `<file>` is
the source file name without its extension and `<test>` the test function's name. The
file holds the text byte for byte. After the test finishes, each missing or different
snapshot fails it, naming the file, and the report shows a unified diff from the
committed text to the actual text: three lines of context, anchored on lines unique to
both sides (Go's internal/diff), so the same inputs always print the same diff. The
test itself keeps running after a mismatch. Recording the same name twice in one test
fails it. `--bless` writes every recorded snapshot instead of comparing, creating the
`snapshots/` directory as needed, and never deletes files; a test that otherwise
passes then passes. Interpreted and native runs compare the same files.

### Benchmarks

`--bench` runs the package's `@bench` functions instead of its tests, under the same
selection, filters and seeds. A bench takes no parameters or one borrowed `Test`, as a
test does. For each bench the runner generates loop entries calling it a fixed 0, 1,
10, ... 10^9 times, and times whole launches: one call to warm up, the zero-call
launch as the baseline, then one call, then the least power of ten predicted to run
for 1.2 times the target time but at most a hundred times the last count, until a launch spends at least the target time beyond
the baseline or the 10^9 count has run (as Go's testing.B grows b.N). `--bench-time
<ms>` sets the target, 1000 ms by default. Each bench prints one line:

```text
bench app@1/parse_header ... 1000000 iterations, 41.3 ns/iter
bench result: ok. 1 measured; 0 failed; 1 total
```

`ns/iter` is the measured launch's time beyond the baseline divided by its count,
rounded to one decimal. There are no thresholds: a bench fails only when a call
fails, which prints `FAILED (seed=S): <failure>` and exit 1. Native benches build at
the default optimization level; interpreted benches run with no step budget, and
their times measure the interpreter. `--bench` takes neither `--bless` nor
`--max-steps`.

`--json` writes one schema-1 object with `tests` and `summary`. Each test has
`name`, `seed`, `status` (`passed` or `failed`), `failure` (null on success) and
`report`, the unindented report text. The summary has `passed`, `failed` and
`total`. With `--bench` the object has `benches`, each with `name`, `seed`,
`status` (`measured` or `failed`), `failure`, `iterations` and `nanoseconds` (the
measured launch's time beyond the baseline), and a summary of `measured`, `failed`
and `total`. Package checking errors use the ordinary check diagnostics and prevent
execution.

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
tools/capped target/debug/hirsel precheck stdlib/std@1/hirsel.json -o target/debug/hirsel-std.precheck
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
`HIRSEL_PACKAGE_CACHE=off` checks everything fresh. The precheck compiler and
Buck2 test tools share the optimized `//tools/buck2:checked` profile, with
debug assertions and integer overflow checks enabled.

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

Cargo needs no build service. What it builds runs locally, so it runs under
[`tools/capped`](#run-locally):

```sh
cargo build --workspace
tools/capped --long --timeout 1800 cargo test -j 2 -p hirsel-lir
tools/capped target/debug/hirsel-conformance --verbose --shard 1/1 \
  --path conformance/SEM/SEM-1-format-into-appends-run.hir
```

`hirsel-conformance` reports counts by rule prefix; `--json` gives tool
output. It finds the repository from the current directory; `--root <repo>`
selects another checkout. The whole corpus is a many-program run:
`python3 tools/buck2/conformance.py` runs its shards on the pool. `--update-passing` (run on this host by `tools/bless.sh passing`) adds newly
passing tests to `conformance/passing.txt` without removing existing entries.
Every in-gate corpus file must pass whether or not that list names it; a listed
file must pass even when excluded from the gate, and the list is the native
gate's baseline, so add a new corpus file's path to it (the list is sorted).

## Run locally

The host is shared. A program run outside Buck2 (a built `hirsel`, a
`hirsel-conformance --path` selection, a probe) runs through `tools/capped`, one
program at a time:

```sh
tools/capped target/debug/hirsel run hello.hir
HIRSEL_MEMORY_LIMIT=2G tools/capped --timeout 60 target/debug/hirsel check app/manifest.json
```

It runs the command in a systemd user scope with `MemoryMax` of
`$HIRSEL_MEMORY_LIMIT` (bytes or a `K`, `M`, `G` or `T` count; default `4G`), no
swap, a `CPUQuota` of `$HIRSEL_JOBS` CPUs (default 2) and a timeout (`--timeout`,
default 300 s), and passes both variables on to the command. Without a reachable
systemd user manager it falls back to process limits (`RLIMIT_DATA` at the cap, a
thread limit and CPU affinity to `$HIRSEL_JOBS` CPUs) and says so. Concurrent runs
take numbered slots in the existing lock directory (`$XDG_RUNTIME_DIR`, or `/tmp`
when unavailable). The default slot count is `floor(MemAvailable / MemoryMax)`
at startup, clamped to 2 through 8; `HIRSEL_CAPPED_SLOTS` overrides it with any
positive count. A queued run prints each holder's pid, cwd, command and age,
the elapsed wait every five seconds, and its total wait on acquisition.
`--timeout` above 900 s requires `--long`, which takes a separate single slot
even with a shorter timeout, leaving the ordinary slots available to short jobs.
Slots are released on exit or crash. At exit it prints the scope's peak memory,
wall time and exit status. A run that reaches the cap or the timeout is a finding: report the program
and its peak rather than raising the cap. Runs over many programs (a directory, a
shard, a campaign) go through `tools/gate` or their Buck2 targets, which run on
the pool within their measured reservations.

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
tools/gate --suggest                            # what the finished change can affect
tools/gate --tests suggested --why "<one line>"  # check it once
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

`tools/gate` checks a finished change, once. It diffs the worktree against its
merge base with `origin/main`. Every gate runs the *floor*; the lane chooses
everything else and says why:

```sh
tools/gate --suggest                                  # the floor and what the change can affect
tools/gate --tests suggested --why "<one line>"       # the floor and the whole suggestion
tools/gate --tests hirsel-driver-test-it-stdlib conformance:conformance/SEM/SEM-3-toml \
  agreement:conformance/SEM/SEM-3-toml --why "toml decoder only; no checker code changed"
tools/gate --why "<one line>"                         # the floor alone
```

The floor:

- documentation no build reads (`*.md`, `LICENSE`, `docs/`): its added links
  resolve, links into deleted paths are gone, and no whitespace errors;
- the gate's own tooling under `tools/`: its tests; the Lean model under
  `proofs/` (and its vectors in `native-tests/model/`): its audit and vector
  replay on this host;
- the build-file check, formatting, Clippy and the workspace build, which
  includes checking std once (the std precheck). They run first, and when one
  fails the gate stops before any test;
- every test target of a crate whose source changed (unit, integration in
  all shards, binary unit tests and doctests), and of every changed test file;
- every changed or added corpus case with its twins (the cases with the same
  name up to its outcome word: `-ok`, `-bad`, `-run`, `-fail`, `-reject`,
  `-accept`), the cases a `passing.txt` edit names, and every corpus case whose
  import closure includes a changed std module. std is source too: those
  cases are its tests.

`--tests` takes Buck2 test targets (`//:name` or `name`; a sharded binary's name
runs every shard), the whole campaigns `conformance`, `agreement` and
`native-smoke`, `conformance:<path>` (the corpus cases under a path),
`agreement:<path>` (those cases' variants in the seed-1 sample: exactly the
variants the whole campaign checks of them), `native:<program>` (the LLVM
smoke matrix and Cranelift parity for one program), `all-tests`, and
`suggested`. Chosen corpus cases run in slices of at most 340 (conformance) or
260 (agreement) cases (`gate-conformance-selected-shard-<i>`,
`gate-agree-selected-shard-<i>`); more than half the corpus runs the whole
campaign.

`--suggest` computes what the change can affect (`tools/buck2/impact.py`):

- a crate's library source: every Buck2 test reading it, directly or through
  a dependency, and its row of `CRATE_CAMPAIGNS`. Front-end, validator and
  interpreter crates select the whole conformance campaign and agreement
  sample, backend, runtime and LLVM crates the native smoke, the shared
  contract crates, the engine and the driver all three. Every crate in the
  dependency closure of a campaign's crates must select it; the gate refuses a
  table that does not;
- a std module (`stdlib/std@1/`): the tests reading std, and the agreement
  sample's variants of the cases whose import closure includes it. Agreement
  compares the source checker, the IR checker and checked interpretation on
  mutated variants of a case's own source; a std edit changes no checker or
  interpreter, only what the variants import, so only the variants of the
  cases that import it can change. A case reaches std only through its
  sidecar's dependency; core and std share some module heads, and
  `list.push` names core where `list.sort` names std (LANG-4);
- a corpus case: its twins and their variants in the sample, never the
  sixteen whole-campaign shards; a package fixture under `packages/`: the
  cases whose packages reach it;
- data every case reads (the spec's rule table, a corpus list the runner
  reads, native fixtures): its whole campaign;
- `hirsel-cli-test-examples` only when `crates/hirsel-cli`,
  `crates/hirsel-driver` or `examples/` change; for a std edit `--suggest`
  names the examples that import it so the lane can add them;
- a build-wide input (`Cargo.toml`, `.buckconfig`, `tools/buck2/`,
  `third-party/`): every test and campaign.

The floor and the chosen tests run as one Buck2 invocation after the build. The
summary lists every target; a failure names its failing cases and the command
that reruns each alone (for agreement, `agree --seed 1 --limit 5000
--sample-path <case>`, the gate's own variants of that case). A passing change
is recorded with its identity (the patch-id of the lane's own diff without
context lines, and the crates it touches), its floor, its selection and its
`--why`. After main moves and the change rebases cleanly, `tools/gate` only
builds and checks formatting. After an edit to a passed change (a one-file
fix, a hand-resolved conflict), `tools/gate --extend --why "<line>"` runs the
floor of the paths whose change differs from the recorded pass and the test
targets that own them, plus any `--tests`, and records the edited change. With
`HIRSEL_GATE_SLOTS=dir:N` set, gates use two independent FIFO pools. The gate
classifies its settled floor plus choice before queueing: no whole
conformance/agreement campaign, no native/LLVM/Cranelift campaign, and at most
four distinct test targets (including selected corpus shards) uses express;
everything else uses normal. Four allows a conformance slice (20 cases), an
agreement slice (260 cases), and two ordinary test targets. Sharded tests
count each resolved shard. `--full` uses normal; a passed change's build-only
check uses express; tooling-only gates take no slot.

The normal pool keeps `dir/count`, `dir/queue` and `dir/slot-<n>`; N seeds a
missing count. Express uses `dir/express-count` (default 2), `dir/express-queue`
and `dir/express-slot-<n>`, so it never waits for normal gates. Each pool
re-reads its count every poll. `tools/gate --set-slots N` and
`tools/gate --set-express-slots N` atomically change the respective count;
neither option chooses a gate's class. Lowering a count leaves existing
holders running. The gate output, green receipt and landing log name the pool.
Already-queued gates keep their loaded code and their normal queue. Unset,
there is no host slot cap.

Nothing runs the full suite on a schedule. Per-push selection and its floor
are the only routine net, so choose what the change can break, and say in
`--why` what you skipped and why that is safe. The full suite (`tools/gate
--full`, below) runs on demand at milestone cuts.

`tools/land` lands the committed change: fetch, rebase onto `origin/main`,
then the workspace build, Clippy, formatting and the build-file check on the
rebased tree, and push, repeating when main moved. It never runs tests: it
refuses a change `tools/gate` has not passed, and stops on a rebase conflict
(resolve it, run `tools/gate --extend` on the result, land again). It prints
the gate's recorded floor, selection and `--why`, and appends each push with
them to `hirsel-landings.jsonl` in the clone's common git directory, so a red
main maps back to the push and to what it chose to skip.

After those checks, land runs the built `hirsel doc stdlib/std@1/hirsel.json` and commits a
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

For changes to `crates/hirsel-llvm`, `hirsel-cranelift`, `hirsel-lir`, `hirsel-rc`
or the runtime, `tools/gate --suggest` proposes the same changed-program filters
through the LLVM native smoke matrix and Cranelift parity (`native:<program>`). With no changed runnable fixtures,
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

`tools/gate --full` is the full suite, run on demand at milestone cuts (nothing
schedules it):
every test, budgeted source/IR and native agreement, release tests,
the `full_gate_` tests in both debug and release, the derived native program and pairwise configuration covers and
every `tools/check-native.sh` runtime, sanitizer, toolchain and Lean check. It
and `tools/check-native.sh` run every stage even after one fails and fail at
the end. `tools/main-full-run.sh` runs it on a fresh detached worktree of
`origin/main` and writes `latest.json`. `tools/main-stall-alarm` appends to
`alarms.log` beside it when main gets no commit for 40 minutes while lane
worktrees are moving; run it from whatever loop watches the lanes.
