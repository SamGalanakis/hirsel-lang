# Building and running Hirsel programs

Browse the [full signature index](api.md) for every public core and std declaration.

New to the language? Start with the [runnable language tour](tour.md), then
try the [runnable tools](../examples/README.md).
[Replay a service](replay-a-service.md) records a ledger service under injected faults,
replays it from its tapes and trials a changed function against recorded calls.
The [concurrent agent guide](concurrent-agents.md) covers compute fan-out,
actors, shared state and scheduling tests. To work on the compiler, runtime
or libraries, see [Contributing to Hirsel](contributing.md).

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
hirsel doc stdlib/std@1/hirsel.json          # core and std signatures on stdout
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

Release builds use LLVM. Combining `--release` with `--backend cranelift`
is refused; use Cranelift for default or debug builds.

### What a native build needs

`build`, `run --native` and `test --native` read their tools from absolute paths in
the environment; nothing is searched on `PATH`. Each command needs only the tools
of the targets it builds for:

| variable | needed for | names |
|---|---|---|
| `LLVM_SYS_211_PREFIX` | every target | the pinned LLVM 21.1.8 tree, whose `bin/ld.lld` links every executable |
| `HIRSEL_LINK_KIT` | every target | the link kit: every link input besides the program, its runtime archive and foreign libraries |
| `HIRSEL_RUNTIME_ARCHIVES` | every target (optional) | prebuilt runtime archives, `<dir>/<triple>/<variant>/libhirsel_rt.a`; without it the runtime is built once with Cargo from the source checkout |
| `HIRSEL_NATIVE_CACHE` | every target (optional) | where Cargo-built runtime archives are kept |
| `HIRSEL_QEMU_AARCH64`, `HIRSEL_AARCH64_SYSROOT` | `aarch64-linux` | the user-mode emulator AArch64 executables launch under, and the sysroot it loads them from |
| `HIRSEL_AARCH64_CC` | `aarch64-linux` without `HIRSEL_RUNTIME_ARCHIVES` | the cross compiler driver Cargo links the AArch64 runtime's build scripts with; no program link runs it |
| `HIRSEL_VALGRIND`, `VALGRIND_LIB`, `HIRSEL_VALGRIND_RUNTIME` | memcheck runtime builds only | the pinned memcheck, its support directory and its libc |

So an x86-64 build needs LLVM and the link kit alone. Install the pinned trees with
Buck2 from the repository root, and export the paths it prints:

```sh
.buck2/bin/buck2 build //tools/buck2/native-toolchain:llvm //tools/buck2/native-toolchain:kit //:hirsel-rt-archives --show-full-simple-output
# AArch64 targets also need:
.buck2/bin/buck2 build //tools/buck2/native-toolchain:arm //tools/buck2/native-toolchain:qemu --show-full-simple-output
```

`LLVM_SYS_211_PREFIX` is the `llvm` directory and `HIRSEL_RUNTIME_ARCHIVES` the
`hirsel-rt-archives` directory. `tools/buck2/native-toolchain/environment.tsv`
lists the path of every other variable inside its toolchain directory
(`tools/native-toolchain/README.md` installs the LLVM tree without Buck2).

`build`, `run` and `test` validate every lowered body with V, the second checker,
as checks do. `--no-verify` skips V: the program, its output and every
diagnostic are the same, and only a compiler bug V would have reported can go
unnoticed. `check` lowers only the bodies compile-time evaluation executes, so V
costs it little. The conformance, agreement and native harnesses always run V
and refuse `--no-verify`.

`build` accepts a source file (and its `.hir.json` sidecar, when present) or a
launch manifest. The default target is `x86_64-linux`; `aarch64-linux` uses its
own runtime archive and link kit, with no cross compiler. Every executable is
linked by the pinned `ld.lld` with the link kit Hirsel builds and ships (no host
compiler, crt, libgcc or sysroot), so the same inputs give the same bytes on any
host. The executable contains the production runtime, compiler builtins and
LLVM's unwinder statically; its shared dependencies are glibc's libraries and
loader at the glibc 2.28 ABI floor, so it runs on any Linux with glibc 2.28 or
newer, independently of Hirsel and the source checkout.

Debug builds from both LLVM and Cranelift emit DWARF 5 source line tables on
x86-64 and AArch64. Cranelift uses final machine source ranges and emits compile
unit and subprogram entries with relocated code addresses. Both backends prefer
an operation's source or cleanup anchor, falling back to its enclosing block;
LIR does not retain a separate span for every scalar instruction. Generated
Cranelift helpers and thunks have no source rows. These are line tables, without
local-variable descriptions.

A debug build writes its debug information beside the executable as
`<build-id>.debug`, which the executable's `.gnu_debuglink` names; gdb finds it
there. Keep the two files together.

Artifact source names are logical: `<package>@<version>/<manifest-relative path>`
(for example `std@1/json.hir`), including profiling strings and failure locations
in both backends. Standalone sources use `standalone@0/<file name>`. Generated
bodies append their declaration/instance suffix to the logical source name.
Physical source paths remain available to compiler diagnostics. This prefix
mapping, like `-fdebug-prefix-map`, makes artifacts independent of the checkout,
cache and scratch directories.

DWARF uses the constant compilation directory `/hirsel`. Remap each package
prefix to its local manifest directory before setting breakpoints or listing
source, including dependencies in other directories. GDB matches the package
prefix in the relative filename. For a manifest declaring
`src/main.hir` in `myapp@1`:

```sh
gdb ./app-debug
# At the gdb prompt (replace /checkout/app with the local manifest directory):
set substitute-path myapp@1 /checkout/app
list myapp@1/src/main.hir:3
break myapp@1/src/main.hir:3
run
```

To check remapping manually, build with `--debug` for each backend, start gdb
from a directory outside the checkout, set the substitution above, and confirm
`list` shows the source line and `run` reaches the breakpoint. The split debug
file must stay beside the executable. The two-root CLI regression compares
executables and split debug files and verifies logical DWARF and failure paths.

```sh
gdb -batch -ex 'break hello.hir:3' -ex run ./hello-clif
llvm-dwarfdump --verify ./<build-id>.debug
llvm-dwarfdump --debug-line ./<build-id>.debug
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
(`{"kind":"not_granted","root":"Io"}`, exit 3), with a line naming `--grants` and
`--simulation <json>`. A simulated world requires that explicit option and is available
only in the interpreter; ordinary runs follow [entry admission](spec/programs.md#prog-3-entry).

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

Interpreted runs, native runs and directly run executables share the
[PROG-3 exit policy](spec/programs.md#prog-3-entry). On an x86-64 host, test an AArch64
artifact with the pinned qemu and a glibc for AArch64 (the cross sysroot here):

```sh
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" ./hello-arm64 \
  --hirsel-run /absolute/path/run.json
```

### Native runs without an executable

`hirsel run --native`, `hirsel test --native` and the native agreement and
differential campaigns write no executable. A program links once, per changed
revision, into a content-keyed program *image*: a shared object of its units
and the same launcher objects an executable links, plus the builtin-support
archive and the link kit's glibc stubs, linked by the pinned `ld.lld`
(`-shared -Bsymbolic`, `--eh-frame-hdr`, exporting only `main`). A prebuilt
*runner*, the runtime archive and the kit's unwinder linked once per target and
runtime archive, loads it and runs its `main` in a fresh process, exactly as the
executable's `main` runs: the same arguments, environment, streams, outcome and
exit status. An unchanged revision reuses its cached image and runs no linker.

Runners and images live in the native cache (`$HIRSEL_NATIVE_CACHE`, or the
driver's build directory) under `runners/<build-id>/` and `images/`. A runner's
build ID is its compatibility identity; each image carries its runner's in a
`Hirsel` ELF note and is keyed by its own build ID, a hash of every input by
content. The least recently used images past 512 are removed when no run holds
them.

Before an image is published, its imports are checked: each is a runtime symbol
the runner exports (and its dynamic table was checked against when it was
built) or a kit library's symbol at the version the executable would bind, none
is weak, it needs only kit libraries and has no text relocation, executable
stack or writable code. A program with any foreign-call surface (an extern, a C
callback or an `unsafe_ffi` link input, even `link: []` calling libc), a memcheck
run, an lld error and a refused image run as a linked executable instead; each
fallback is counted with its reason, which `--timings` reports and the native
campaigns gate (only `foreign` and `memcheck` fallbacks pass). A runner that
cannot load an image (one linked for another runner, a truncated file, a
`dlopen` error) reports a tooling outcome, `Tooling(ImageLoad)`, before any
program code runs.

A runner whose startup audit admits every constructor that runs before its
`main` (glibc's and crtbegin's, AArch64's CPU probes, Rust std's unread argv
capture) serves bounded runs (tests and campaigns) from one fork server per
runner: each run is a forked child in its own process group, which takes the
run's streams, environment and directory, checks the image's note, `dlopen`s it
and calls its `main`. `run --native`, runs under a launch wrapper and runners the
audit refuses start a fresh runner process per run (`hirsel-runner exec`), in
the launcher's process group as an executable would be. AArch64 runners run
under qemu like executables, children forked inside the emulator.

The native smoke gate and `tools/gate --full` hold the two paths to each other:
on both targets every case runs as its linked executable and as its image in the
runner, and stdout, stderr, exit status and observation record must agree byte
for byte; under the first configuration each path's recorded Io tape must replay
through the other with the same observation.

## Run a package's tests

```sh
hirsel test examples/calculator/hirsel.json
hirsel test app/manifest.json parser decode      # match either substring of the stable path
hirsel test app/manifest.json --seed 42 --json
hirsel test app/manifest.json --native
hirsel test app/manifest.json --native --backend cranelift
hirsel test app/manifest.json --bless             # accept the actual snapshots
hirsel test app/manifest.json --bench --bench-time 500
hirsel test app/manifest.json --explore 100       # 100 seeded random@1 schedules per test
hirsel test app/manifest.json --explore 100 --scheduler pct@1
hirsel test app/manifest.json race --scheduler random@1:7   # replay one schedule
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
at least one failed. Native mode links the package's tests once, as one image
(or, when the package calls C, one executable), and runs it in a fresh process
for each selected test, with the same output and seeds.

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

### Schedules

A test's tasks (`test.tasks`) run under the run's scheduler (`inputs.scheduler` in
[determinism.md](spec/determinism.md#run-configuration)), `fifo@1` by default.
`test.clock(t)` supplies a fake clock starting at zero; it advances to the earliest
live timer only when no task can run. Blocked tests fail with their outstanding waits.
`--scheduler <name@1:seed>` runs every selected test once under that scheduler.
`--explore <n>` runs each selected test under seeds 0 to n-1 of `random@1`, or of the policy
`--scheduler <name@1>` names (`random@1` or `pct@1`, without a seed), and stops that test
at its first failing schedule, a Blocked run (CONC-8) included. The runner then prints the
failing scheduler and how many schedules ran, `(seed=S, scheduler=pct@1:7, schedule 8)`,
and under the failure's report a `replay:` line: the `hirsel test` command that reruns
exactly that schedule, in the same engine. A passing test prints how many schedules it
explored. With `--json` each test also carries `scheduler`, `schedules` and `replay`.
The same seed selects the same schedule interpreted and native.

For application-level races, add `--race` to an interpreted test or exploration:

```sh
hirsel test app/manifest.json --explore 100 --race
hirsel test app/manifest.json --explore 100 --scheduler pct@1 --race --sim-points root-release,atomic-store
```

On failure its report names both operations in each race candidate: task spawn
path, access occurrence, operation, source file, line and column, and the shared
object. It watches atomic loads/stores and `shared.thaw` versus root releases.
These are cross-task accesses to the same object with at least one write, not a
happens-before proof or a causal diagnosis of the assertion. Synchronised and
intentional accesses can appear too. The report keeps at most 16 distinct source
pairs; a passing test emits no candidates. Hirsel remains data-race-free (CONC-4).

`--sim-points` optionally adds a suspension before each `atomic.store`
call and after each root release, including aggregate cleanup. Choose either name
or both, comma-separated. Atomic operations already suspend after the operation;
the extra store point explores its entry too. These simulator-only points can
explore interleavings outside ordinary cooperative suspension points. Both flags
are off by default, require interpreted tests, and are included in the printed
replay command. The same seed **and point settings** reproduce the run. Diagnostics
and point markers stay outside semantic observations and Io tapes.

The simulation JSON also accepts `scheduler` (for example `"pct@1:7"`), `race`
(a boolean) and `sim_points` (the same comma-separated names). The interpreter API
exports candidates separately from its semantic observation; test reports display
them on failure. These settings do not change production or native scheduling.

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

### Durable recording

A run record's `recording` member chooses the level: `"record"` writes the tape above,
`"durable"` writes a durable journal instead, with each request synced before it is issued
and each completion before the program sees it
([Recording levels](spec/determinism.md#recording-levels)). Both need `--record`
(`--hirsel-record`); `"none"` refuses one; absent, `--record` alone records the tape. A
complete journal replays with `--replay` exactly like a tape, in either engine. A journal
cut short by a crash refuses replay and names the requests in doubt, which the operator
reconciles, never by retrying blindly.

```sh
hirsel run app.hir --grants durable-grants.json --record run.journal   # "recording": "durable"
hirsel run app.hir --replay run.journal
```

Durability costs two barriers (a write and `fdatasync`) per request that does not park.
Measured 2026-10-19 on one IO-heavy program (2,000 appends to one file, 500 whole-file
writes and 100 `put_new(Durable())` calls: 2,604 requests), compiled, as a single capped
local run per level with the median of three, on ext4 over datacenter NVMe with
power-loss protection:

| Level | Median wall time | Recording size |
|---|---|---|
| none | 73.5 ms | none |
| record | 82.9 ms | 208 KB |
| durable | 481.3 ms | 508 KB |

The durable level adds about 150 µs per request here (two `fdatasync` calls of about 75 µs
each). On a drive without power-loss protection a barrier commonly takes milliseconds, so
expect seconds for the same program. Requests that park (network, process, stdin and timer
waits) share barriers through group commit.

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
revisions and replay identity are the same with and without a precheck.

## Compile timings

`check`, `build`, `run` and `test` take `--timings`, `--timings=json` and
`--mem-usage` anywhere before `--`. Both reports go to stderr after the command
finishes; other commands refuse the options (exit 2). Off, each timing point
costs one relaxed atomic load and reads no clock.

```sh
hirsel check --timings app/hirsel.json
hirsel build --timings=json --backend cranelift app/hirsel.json -o app 2> timings.txt
hirsel check --mem-usage app/hirsel.json
```

The report keeps three measures apart (`hirsel_intern::timings`):

- **Intervals** are the wall time of the driver's non-overlapping steps. An
  interval entered inside another pauses it, so intervals add up to the wall
  time they cover; `(untimed)` is the rest of the command's wall time (CLI
  startup, and the program's own execution under `run` and `test`). Only
  intervals add up. Today's driver maps onto them as follows:
  - `image open`: reading and decoding the std precheck;
  - `S0`: loading the package graph and sources, parsing, publishing std's
    declarations into the session, and the program check's own steps;
  - `parallel`: checking each source file's definitions and lowering the
    program's reachable closure, which drive each definition's resolve,
    check, modes, lowering and V;
  - `merge`: GEN-5, publishing checked packages and dropping the session;
  - `plan`: exporting, importing and adapting the native input;
  - `units`: each unit's LIR import, placement, RC, reuse, cleanup lowering,
    LIR validation and code generation;
  - `link`: the toolchain, the link and the output copy;
  - `S1` and `parallel2` stay 0 until their driver steps exist.
- **Phases** are work time (wall) and CPU time inside each phase, excluding
  the phases nested in it, summed over every thread, with their entry counts.
  Engine bookkeeping for a query (cache validation, read recording) counts as
  that query's phase. `verify in comptime` is the part of `verify` run inside
  compile-time evaluation. Phases overlap the intervals and are not added up.
- **Wait** is time threads spend waiting for each other; 0 while compiling is
  single-threaded.

Counts: `definitions` (bodies checked), `units` (native units built), `V runs`
(full V runs; a definition V already passed is skipped), `lowered` (bodies
lowered; under `check`, the bodies lowered at check time), `recomputes`
(per-definition queries computed rather than reused) and store and object
hits and misses (0 until those caches exist).

`runner` reports the native runs made as program images in a runner (`image
runs`), how many of those images were already cached (`cached images`), and each
kind of fallback to a linked executable with its first reason (`fallback: foreign:
the program calls C (1)`); see "Native runs without an executable".

`--timings=json` prints the same report as one JSON line, the last on stderr,
which compiler-bench reads:

```json
{"schema":1,"command":"build","wall_ns":712000000,"untimed_ns":9000000,
 "intervals":[{"interval":"image open","wall_ns":101000000}, ...],
 "phases":[{"phase":"parse","work_ns":3000000,"cpu_ns":2900000,"calls":128}, ...],
 "comptime_verify":{"work_ns":0,"cpu_ns":0,"calls":0},"wait_ns":0,
 "counts":{"definitions":2,"units":168, ...},
 "runner":{"image_runs":1,"cached_images":0,"fallbacks":[]}}
```

Every interval, phase and count appears, in that order, including those still
0. `crates/hirsel-cli/tests/cli/timings.rs` checks the schema.

`--mem-usage` prints the process's peak RSS and the heap bytes held at the end
of the command, largest first: the decoded std precheck (`precheck`), each
table of the session's query engine as it is dropped (`engine: ...`), then the
shared package cache (std's image) and V's retained signature inputs. A table
still shared with the package cache frees nothing when the engine drops, so
std's share appears under `package cache`. With `--timings=json` the same
figures go into the JSON's `memory` object instead. Heap bytes are the bytes
the binary's allocator (mimalloc) handed out and got back, which `--mem-usage`
alone counts, read before and after each measured load or drop. Counting adds
an atomic update to every allocation, so time a run without `--mem-usage`.

## Inspecting dependencies

`hirsel deps card <manifest.json> [--package name@version[/module]] [--json]`
reports computed authority, weight, public API exposure, replay constraints and
health. See [Dependency cards for agents](dependency-cards.md) for field semantics,
cache reuse and optional verified code-store provenance.
