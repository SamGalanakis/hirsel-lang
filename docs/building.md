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

## Start a program or package

Discover exact options with `hirsel check --help`, `hirsel run --help`,
`hirsel build --help`, `hirsel test --help` or `hirsel help fmt`.
`-h` and `hirsel help COMMAND` show the same command help without source,
manifest, toolchain, cache or grants. Use help before a source operand;
`hirsel run app.hir -- --help` passes `--help` to the application.
Compiler flags differ from standalone launch prefixes: see the
[native recording recipe](native-verification.md#record-remove-host-access-replay).
For a standalone executable, runtime options such as `--hirsel-record` and
`--hirsel-grants` may appear in any order before the first application argument
or `--`. The delimiter ends runtime option parsing and is omitted from Args.

A single `.hir` file needs no manifest: it belongs to `standalone@0` and selects
exactly the `std@1` shipped with the toolchain, reusing its std precheck. An exact
`<source>.hir.json` sidecar overrides that selection; a sidecar without std is the
explicit std-free form. No parent manifest is searched and no dependency is downloaded.
Selecting std grants no authority.

```sh
hirsel new demo
hirsel test demo/hirsel.json
hirsel run demo/hirsel.json --grants demo/grants.json
# The generated source also runs on its own:
hirsel run demo/main.hir --grants demo/grants.json
hirsel new mylib --lib
hirsel test mylib/hirsel.json
```

`new <dir> [--lib]` accepts a new or empty directory whose final name is a package
identifier (letters, digits and underscores, starting with a letter or underscore;
reserved keywords are refused).
It writes a schema-1 `hirsel.json` for `<dir name>@0`, `main.hir` with a greeting
and one `@test`, and `grants.json` granting only stdout. The manifest selects the
installed toolchain's std manifest by its resolved path. The default package has a
`main` entry taking `Io` and prints with an f-string; `--lib` omits the entry and
writes a public greeting function with a test. Non-empty directories are refused.

The generated std dependency path is absolute to the creating toolchain. When
checking an example into this repository, use `../../stdlib/std@1/hirsel.json`
from its manifest so another checkout selects its matching std. Keep absolute
paths for a package tied to an installed toolchain.

`check`, `test` and `doc` accept a library manifest without `entry`; `build` and
`run` require a launch manifest with an entry. To add a CLI without turning a
library into an application, keep a second launch manifest listing the library
sources plus a driver. [snapshift](../examples/snapshift/README.md) shows both
manifests and direct typed JSON decoding with a checked Date.

## Document a package

```sh
hirsel doc app/manifest.json                 # Markdown on stdout
hirsel doc app/manifest.json -o app/api.md    # write a page
hirsel doc app/manifest.json --json          # schema-1 JSON
hirsel doc stdlib/std@1/hirsel.json          # core and std signatures on stdout
```

`doc` accepts a source file or an explicit package manifest, including a library without an entry.
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

## The resident compiler

`check`, `build` and `run` start a compiler server for the workspace on demand.
It keeps the loaded program and query results between commands. A body edit
reuses callers checked against the same declared signatures; a signature edit
invalidates the definitions that read it. Diagnostics use the current source
positions, including when an item moves.

```sh
hirsel serve --workspace . --idle 900  # foreground server, seconds until idle exit
hirsel serve --stop --workspace .     # stop this workspace's compiler server
hirsel check app/hirsel.json          # use the workspace server
hirsel check --watch app/hirsel.json  # check after edits until interrupted
HIRSEL_DAEMON=off hirsel check app/hirsel.json  # a fresh compiler process
```

A workspace is the nearest ancestor of the entry with a `.git` file or
directory, or the entry's directory outside a checkout. Its socket lives in a
user-private directory under `$XDG_RUNTIME_DIR` (or the system temporary
directory). `HIRSEL_SERVE_IDLE` sets the default idle timeout, in seconds;
`--idle` overrides it. A busy server lets the client compile in its own
process. `serve --stop` requests a clean shutdown, waits for the socket to be
removed, and succeeds if no server is running. It waits up to 30 seconds for a
busy server to finish its command. Commands with `--timings` or `--mem-usage`
run in their own process, as do programs reading a terminal. A client with different compiler code,
environment or file creation permissions uses a fresh process.

Definition results also persist under `$XDG_CACHE_HOME/hirsel/results-v2`
(or `$HOME/.cache/hirsel/results-v2`). A new process verifies the recorded
signature, lookup, representation and execution reads before using a result.
Lowered TIR carries the validator's own recorded reads; missing or stale
validation evidence runs V before use. A corrupt entry is a cache miss.

One-shot compiler fronts release checked and analyzed bodies on publication.
Their summaries, diagnostics and dependency receipts remain cached. Library
hosts can select `Compiler::with_retention(Retention::Resident)` to retain front
bodies; resident `Session` instances do so. This choice changes retention and
recomputation, not program output.

Library hosts can supply `CompileConfig` through `Compiler::with_config`,
`QueryEngine::with_config` or `Session::open_configured`. It selects jobs,
verification, retention, lowering, the result-store provider and verification,
and total `hold_bytes`. The default budget is min(256 MiB, 10% of
`HIRSEL_MEMORY_LIMIT`); explicit zero disables retained holds. Active evaluation
artifacts remain in-flight memory. A configured Compiler is Clone, sharing its
provider across fresh sessions. Use `LoadedProgram::check_configured` to select
its configured lowering policy, or `check(policy)` for an explicit override.
Native object storage uses the driver's existing separate provider.

`HIRSEL_RESULTS` selects `dir@1` (the default), `mem@1`, `none@1` or `sim@1`;
`HIRSEL_RESULTS_DIR` overrides the disk directory. `HIRSEL_VERIFY_CACHE=1`
recomputes disk hits and compares their canonical bytes. `--verify-cache` on
check, build and run selects the same policy and bypasses resident dispatch.
`none@1` disables query-result writes while keeping installed ordinary std
products eligible. V's independently issued accept verdicts use a separate
host-local cache under `$XDG_CACHE_HOME/hirsel/validator-v2` (or
`$HOME/.cache/hirsel/validator-v2`), including with `none@1`. `HIRSEL_PRODUCTS`
and `HIRSEL_RESULTS_DIR` cannot select this namespace. Each receipt binds exact
validated content, compiler/V build, options and current validation reads;
unreadable or mismatching receipts run V again. A missing cache location uses
process-local V authority. Use a fresh absolute `XDG_CACHE_HOME` with
`HIRSEL_DAEMON=off` for a fresh-verifier reference check. Source snapshots are
renewed from disk results after 64 edited revisions to bound retained history.

### Isolated candidate checks

`hirsel candidate --workspace DIR` reads version-1 JSON lines on stdin and
writes one JSON reply for each line. It opens a structured connection to the
same resident socket as ordinary commands. A connection owns its base and
candidate handles; closing it releases them. Transport refusal/failure exits 3
without checking the unedited disk program.

Start by sending `{"version":1,"op":"capture","entry":"hirsel.json"}`.
The reply's `ok.base` is an owned base handle; `ok.sources` lists each source's
canonical `file`, full-content `expected` digest and UTF-8 byte length. Submit
an alternative against that handle:

```json
{"version":1,"op":"submit","id":1,"base":1,"priority":"selected","edits":[{"file":"main.hir","expected":"<digest from capture>","edits":[{"start":0,"end":0,"replacement":"// alternative\n"}]}]}
```

Use the returned `ok.lease` in `result`, `cancel`, `observe_disk` and `release`.
`result` returns `{"pending":true}` while the worker runs, then a receipt with
completion/verdict, fixed On/Comptime policy, package scope, snapshot diagnostics,
checked-product digest and independent head/disk freshness. Polling does not
perform compiler work. An incomplete result has no successful checked product.
Different request IDs with exactly identical resulting inputs compute once;
waiter cancellation remains independent. Reusing an ID with different edits
rejects. A selected priority affects queued admission only.

`promote` takes `id` (an idempotent operation ID), `lease` and the expected
`base` handle. A complete successful candidate promotes only if that base is
still the exact head generation. It selects an in-memory head and writes no
source files. `head` returns another owned base handle; `release` drops a base
or candidate handle. `counters` reports actual loads, recomputations, retained
bytes and queue/compute time.

`dependencies` preserves the version-1 `Unknown` string when either complete
checked endpoint is unavailable, including fresh captures and incomplete child
checks. A promoted head retains its checked evidence. An explicit unchanged-base
submission also certifies that exact base for subsequent receipts (even when the
complete check rejects the language program). No implicit base check runs.
A completed receipt never changes retroactively when evidence arrives later.

When both endpoints are available, `dependencies` is an additive JSON object
with `kind: "Known"` and `format: "hirsel.checked-read-delta.v1"`. `parent` and
`child` each bind the complete input digest, compiler/environment, package scope,
Verify/Comptime policy and checked-product digest. `reads` is sorted by exact
canonical owner bytes then read-key bytes; both are lowercase hex using the
existing persisted DefPath/ReadKey encodings. `before`/`after` are exact observed
value digests: null/value means added, value/null removed, unequal values changed.
Equal reads are omitted. Negative lookups and private-view reads participate.
This is a checked-read difference, neither minimal invalidation nor execution or
test impact. Absolute child products may coalesce across parents; deltas are
computed per receipt. Retained evidence and differences count against existing
workspace/result quotas; refusal preserves promotion eligibility rules. If evidence
metadata cannot be retained under quota, the promoted base stays `Unknown`.

The enabled policy is serial frozen-input checking: one active candidate uses
the host's compiler worker budget, with at most 64 owned handles, 1024 new-work
requests per connection before new-work admission stops, and 8 MiB frames.
Polling and other controls count toward that threshold. Exhaustion refuses new work while
allowing cancellation, release, promotion and existing-result lookup. An idle
connection closes after 30 seconds without a frame; disconnect requests
cancellation and drains active work at the core's safe boundaries. The server
then exits to reclaim process-global intern tables. Live connections are never
silently replaced to reclaim memory. Legacy commands and other connections
receive Busy while this structured connection owns the host. Fast engine
forks and concurrent distinct candidates are not enabled. Ordinary single
checks continue using the ordinary resident path.

## Build and launch a program

```sh
hirsel build hello.hir -o hello                 # LLVM O2, production runtime
hirsel build app/manifest.json --release -o app # LLVM O2
hirsel build app/manifest.json --release --lto -o app-lto # whole-module LLVM O2
hirsel build hello.hir --debug -o hello-debug   # Cranelift, with DWARF source line tables
hirsel build hello.hir --debug --backend llvm -o hello-llvm-debug  # LLVM O0
hirsel build hello.hir --target aarch64-linux -o hello-arm64
```

The edit loop's commands, `build --debug` and `run --native`, compile with
Cranelift unless `--backend llvm` asks for LLVM; default and release builds use
LLVM O2. `--lto` explicitly requests whole-module LLVM O2;
it cannot be combined with `--debug` or `--backend cranelift`. Combining
`--release` with `--backend cranelift` is refused. The backends' own
IR verifiers (LLVM's module verifier, Cranelift's `enable_verifier`) run only with
the explicit `--validate-stages` policy (or the harness default
`HIRSEL_VALIDATE_STAGES=1`) and in
compilers built with debug assertions, as every test build is.

### What a native build needs

`build`, `run --native` and `test --native` read their tools from absolute paths in
the environment; nothing is searched on `PATH`. Each command needs only the tools
of the targets it builds for:

| variable | needed for | names |
|---|---|---|
| `LLVM_SYS_231_PREFIX` | every target | the pinned LLVM 23.1.4 tree, whose `bin/ld.lld` links every executable |
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

`LLVM_SYS_231_PREFIX` is the `llvm` directory and `HIRSEL_RUNTIME_ARCHIVES` the
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

A debug build keeps DWARF in the executable. GDB and LLDB read it directly;
there is no debug sidecar or post-link objcopy pass. For packaging, a separate
explicit `llvm-objcopy --only-keep-debug` / `--strip-debug --add-gnu-debuglink`
operation can split the executable after building.

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
`list` shows the source line and `run` reaches the breakpoint. The two-root CLI
regression compares executables including embedded debug information and verifies
logical DWARF and failure paths.

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
(`{"kind":"not_granted","root":"Io"}`, exit 3), with a line naming `--grants` and
`--simulation <json>`, followed by a working inline `--grants '{...}'` template
for exactly the entry's requested roots. The hint grants `Io` only stdout, `Args`
an empty snapshot, `Env` an empty map and `Unsafe` its explicit root, as requested.
It changes no grants until the caller supplies it. A simulated world requires that explicit option and is available
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

See the [checked native argv recipe](native-argv.md) for exact argument vectors,
empty process args and a preserving POSIX wrapper.

Interpreted runs, native runs and directly run executables share the
[PROG-3 exit policy](spec/programs.md#prog-3-entry). On an x86-64 host, test an AArch64
artifact with the pinned qemu and a glibc for AArch64 (the cross sysroot here):

```sh
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" ./hello-arm64 \
  --hirsel-run /absolute/path/run.json
```

### Compiler worker limits

Set `HIRSEL_JOBS=1` when running several CLI invocations on a shared host. This
bounds compiler workers; it grants no program authority. A source build with many
workers can exhaust thread or address-space limits before resident memory reaches
the reported memory cap. The repository's `tools/capped` wrapper also sets a
worker budget; installed CLI users can set `HIRSEL_JOBS` directly.

If the OS refuses a compiler worker, Hirsel warns once on stderr, including the
OS error and the `HIRSEL_JOBS=1` remedy. It stops creating workers for that
frontier. Already started workers finish the queued work; if none started, the
compiler thread finishes it, as with jobs1. Results retain their input order and
compilation keeps its usual exit status: successful compilation still exits 0,
source errors still exit 1. This warning is an operational resource failure,
not a source-language error, and does not add a diagnostic to `--json` output.
Task panics still propagate after workers have joined.

This follows Rust's fallible scoped-thread builder and explicit join contract
([Rust `scoped.rs:256–268,293–323`](https://github.com/rust-lang/rust/blob/a7c37f9f0ee5e93396106e9de20f4a86b4154e63/library/std/src/thread/scoped.rs#L256));
the panic-payload controls follow
[`thread/tests.rs:110–118,247–263`](https://github.com/rust-lang/rust/blob/a7c37f9f0ee5e93396106e9de20f4a86b4154e63/library/std/src/thread/tests.rs#L110).
Hirsel adds queue draining with fewer workers and a once-per-process warning
because its results are independent of worker count.

### Scripted network grants

The JSON representation of a scripted peer differs from the Hirsel `NetPeer`
constructor: `resolves_to` contains IP **strings**. A `net.script@2` grant always
includes `peers`, even when it is empty. Listeners use `address` and `port` on each
client, rather than the source constructor's `from: SocketAddr` field:

```json
{"io":{"net":{"provider":"net.script@2","rules":[
  {"address":"check.test","service":"80","operation":"connect"}
],"peers":[{"address":"check.test","port":80,
  "resolves_to":["127.0.0.1"],"tls":"plain",
  "connections":[{"open":[{"send":[111,107]},"close"]}]}]}}}
```

For a server grant, use `"peers":[]` and add, for example,
`"listeners":[{"address":"127.0.0.1","port":18765,"clients":[{"address":"127.0.0.1","port":40000,"tls":"plain","steps":[{"send":[71,69,84,32,120,10]},"stall"]}]}]`
alongside an exact `listen` rule. See [linkcheck](../examples/linkcheck/grants.json)
and [kvd](../examples/kvd/session-grants.json) for complete launch templates.
An exhausted listener reports `ConnectionAborted`; keep its scripted client at
`"stall"` when testing graceful shutdown with a scripted signal. A
`signals.script@1` arrival is delivered only when all tasks are idle.

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

`--hirsel-trace-events events.jsonl` exports canonical events for both
`tasks.free@1` and FIFO `tasks.cooperative@1`. Free uses
`hirsel.free.events@1`: object histories, host-key epochs, outputs, observed
outcomes, each task's final phase and the public terminal outcome. CPU receipts
include width, action, count and outcome. The export works with recording,
replay, or a live run without a tape. Live export without recording discards
tape bytes and retains the semantic projection. Replay validates every record
and final phase before publishing the JSONL file.

A compiled executable can select a fresh Ordered execution policy while replaying
exactly the same host inputs. This supports scheduler diagnosis and tuning without
recording new effects:

```sh
./app --hirsel-replay-policy '{"commit":"dedicated","reorder":"off"}' \
  --hirsel-replay failing.tape
./app --hirsel-trace-events replay.jsonl \
  --hirsel-replay-policy '{"commit":"shared","reorder":"auto","run_ahead":8}' \
  --hirsel-replay failing.tape
```

`--hirsel-replay-policy` accepts an inline JSON object containing only `commit`,
`reorder`, `run_ahead` and `speculation_bytes`, with the same values and bounds as
Ordered placement. Unknown and duplicate fields refuse. The option requires
qualified Ordered execution, the FIFO `tasks.cooperative@1` provider and
`--hirsel-replay`; it cannot carry cores,
CPU affinity, sidecar paths or grants. It uses fresh validated host policy,
preserves the run identity, and never uses recorded placement provenance.
Changing commit or reorder policy must replay identically: canonical Ordered
events, program output and exit are unchanged. A divergent request or unconsumed
input still refuses normally. The input tape is immutable, and a trace destination
that aliases it is refused. Production Ordered is enabled for the FIFO
`tasks.cooperative@1` contract, including width one. Free and seeded non-FIFO
contracts keep their own admission.

`tools/check-native.sh` registers the Ordered acceptance corpus in ordinary
builds, including both native backends, live and
immutable-input replay at widths 1/2/4/8, shared/dedicated commit and reorder
off/auto. It runs on the host so the pool executor cannot clamp wider cells to
two CPUs. Physical perturbation uses forced migration at suspension; at width
one the replay perturbation cell is equivalent to its unperturbed counterpart.
Run a single case with:

```sh
.buck2/bin/buck2 test -c hirsel.tests=host \
  //:hirsel-cli-test-cli-shard-1 -- \
  --test-arg=ordered_qualification_corpus_saved_receiver --test-arg=--ignored
```

`--record` combines with `--run` or `--grants`; `--replay` takes neither, because the tape
binds the canonical run record and admission time it was recorded with. The tape is one
format for both engines: a run recorded interpreted replays compiled and the other way
round. Replay refuses before entry a tape bound to a different semantic program revision,
entry or root set, so editing a function body invalidates its recordings; while it
runs, a request that differs from the next recorded one, a missing event or an unconsumed
event refuses it after the output it confirmed. A tape cut short (no terminal frame)
never replays. The tape path must lie outside every granted directory. Tapes hold the run
record, including granted Env values, every byte read or written and every
`random.system@1` draw: treat them as secrets.

The on-disk format is binary v11 (`hirsel-io-tape 11` plus NUL), using
length-prefixed CRC32C frames and raw payload bytes. Durable journals use format
4 with the same frame codec. Use `hirsel tapes check` and `hirsel view` to inspect
recordings; text rendering is outside recording and replay. Existing v1 durable
fixtures must be regenerated by their owning tests. Legacy text tapes remain
readable. Recording preserves the header, completion and trailer flush points;
durable barriers are unchanged. See [binary tape format](spec/determinism.md#durable-journal).

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

## Ordinary std products

The toolchain precomputes std through the same query store used for user packages.
`hirsel precompute` checks a package with V enabled and persists independently keyed
products, including declaration heads, canonical type records, summaries, lowered
TIR, audits and compile-time values. It refuses diagnostics. Every reader validates
the recorded dependencies and establishes its own verifier authority.

Build and install the release toolchain with Buck2 from the repository root:

```sh
bundle="$(.buck2/bin/buck2 build --target-platforms //tools/buck2:release \
  //:hirsel-toolchain --show-full-simple-output)"
install -d "$HOME/.local/bin"
install -m 755 "$bundle/hirsel" "$HOME/.local/bin/hirsel"
cp -R "$bundle/hirsel-std.products" "$HOME/.local/bin/"
install -d "$HOME/.local/bin/stdlib"
cp -R "$bundle/stdlib/std@1" "$HOME/.local/bin/stdlib/"
```

Keep the executable, products directory and matching source tree together.
Cargo users can create the products with:

```sh
tools/capped target/debug/hirsel precompute stdlib/std@1/hirsel.json -o target/debug/hirsel-std.products
```

The default directory provider reads `$HIRSEL_PRODUCTS`, or `hirsel-std.products`
beside the executable, in addition to its writable local store. Explicitly configured
providers retain their selected roots. Missing, corrupt or stale records are ordinary
cache misses. Current compiler, schema, policy, limits and observed source/dependency
identities select each record. The store maps bounded chunks and restores only demanded
records and type rows. Syntax is materialized when a diagnostic or query needs it.
Read-only products cannot transfer a producer's verifier receipts. Diagnostics,
semantic revisions, private representations and native source audits retain their
source-check contracts.

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
  - `S0`: loading the package graph and sources, parsing, publishing std's
    declarations into the session, and the program check's own steps;
  - `parallel`: checking each source file's definitions and lowering the
    program's reachable closure, which drive each definition's resolve,
    check, modes, lowering and V;
  - `front prepare`: serial preparation of immutable P and S1 worker inputs,
    including signature snapshot normalization; this pauses its enclosing interval;
  - `GEN-5`: the whole-program generic-instance growth verdict;
  - `merge`: publishing checked packages and dropping the session;
  - `plan`: exporting, importing and adapting the native input;
  - `units`: each unit's LIR import, placement, RC, reuse, cleanup lowering,
    LIR validation and code generation;
  - `link`: the toolchain, the link and the output copy;
  - `S1`: dynamic compile-time Lower frontiers and canonical registration rounds;
  - `parallel2`: remaining ordinary lowering and staged-instance completion.
- **Phases** are work time (wall) and CPU time inside each phase, excluding
  the phases nested in it, summed over every thread, with their entry counts.
  Engine bookkeeping for a query (cache validation, read recording) counts as
  that query's phase. `verify in comptime` is the part of `verify` run inside
  compile-time evaluation. Phases overlap the intervals and are not added up.
  `lex` times tokenization alone. `parse` excludes nested lexer work;
  `declarations` times declaration indexing, excluding nested signature phases.
  `signature normalize` times header normalization alone, excluding header
  resolution and nested queries. `signatures` is the remaining query work.
  Broad source/declaration startup is the `S0` interval, not lexer work.
- **Wait** is time threads spend waiting for each other; 0 while compiling is
  single-threaded.

Counts: `definitions` (bodies checked), `units` (native units built), `V runs`
(full V runs; a definition V already passed is skipped), `lowered` (bodies
lowered; under `check`, the bodies lowered at check time), `recomputes`
(per-definition queries computed rather than reused), `invalidated by <read>`
(memos the engine marked dirty because one input of that read kind changed:
signature, lookup, name set, representation, executed body, cited text,
declaration candidates and the rest of the engine's typed reads), `mode memo
queries` and `mode memo hits` (the proofs and comparisons the mode passes ask
their query's `ContextMemo`, and the entries it reused, interior sub-proofs
included) and store and object hits and misses (0 until those caches exist).

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

S1 root task counts include registration retries and speculative suffixes.
`S1 discarded roots` counts outcomes withheld by prefix barriers, not their
CPU cost; `S1 registration rounds` counts restarted rounds. `S1 scalar roots`
exposes compatibility evaluation, including cached constant revalidation.
Physical Lower attempt/adapter counters include helpers and discarded work.
These timing names are additive schema-1 fields; artifact identities are unchanged.

Every interval, phase and count appears, in that order, including those still
0. `crates/hirsel-cli/tests/cli/timings.rs` checks the schema.

`--mem-usage` prints the process's peak RSS and the heap bytes held at the end
of the command, largest first: each session query table as it is dropped
(`engine: ...`), the shared package cache and V's retained signature inputs.
Tables still shared with another session free no bytes at that drop. With `--timings=json` the same
figures go into the JSON's `memory` object instead. Heap bytes are the bytes
the binary's allocator (mimalloc) handed out and got back, which `--mem-usage`
alone counts, read before and after each measured load or drop. Counting adds
an atomic update to every allocation, so time a run without `--mem-usage`.

## Inspecting dependencies

`hirsel deps card <manifest.json> [--package name@version[/module]] [--json]`
reports computed authority, weight, public API exposure, replay constraints and
health. See [Dependency cards for agents](dependency-cards.md) for field semantics,
cache reuse and optional verified code-store provenance.

## Native unit objects

Native builds lower and emit units on `HIRSEL_JOBS` workers (the host's available
parallelism by default). `HIRSEL_OBJECTS=dir@1` is the default writable store;
it retains checksummed fragments under
`$XDG_CACHE_HOME/hirsel/objects-v3/<target>-<backend>-<opt>` (or `$HOME/.cache`),
and `sim@1` keeps them in memory. `HIRSEL_OBJECTS=none@1` disables native
object storage. Each partition has a 4 GiB cap with least-recently-used eviction. The
first publication of each store handle collects old entries; later publications
collect every 64 MiB (or the configured cap, if smaller), bounding slack per
active writer. Result records use `HIRSEL_RESULTS=dir@1` by default, with a
2 GiB cap and the same collection policy; `HIRSEL_RESULTS=none@1` disables them.
Empty `XDG_CACHE_HOME` falls back to a nonempty `HOME`. Implicit cache
locations must be absolute; empty or relative variables never place a cache in
the working directory. If neither location is available, an ordinary native
build uses no writable object store and recomputes misses; the optional result
cache is off. The installed read-only std seed remains eligible.
`HIRSEL_OBJECTS=dir@1` explicitly requests a directory and refuses a missing
location with a diagnostic naming `XDG_CACHE_HOME` and `HOME`. Disk providers
retain their best-effort I/O contract: unreadable entries are misses, failed
writes are discarded, and an unwritable directory cannot produce a hit for a
result that was never stored. There is no shared temporary-cache fallback.
`HIRSEL_RESULTS_DIR` selects an explicit result directory (empty means unset);
library hosts selecting `store_by_name("dir@1", ...)` receive an error when no
location is available. The compiler's optional environment-selected result
provider treats that unavailability as off.

Buck2's test launcher gives each action a private cache shared only by that
action's threads and children, overriding inherited `XDG_CACHE_HOME`,
`HIRSEL_RESULTS_DIR` and `HIRSEL_NATIVE_CACHE`. It removes that directory when
the action exits, including test failures. Ordinary CLI commands continue to
reuse the user's cache across invocations.

Fresh user caches contain no other program's results. Only the installed
read-only std products can serve another program's first build.

Object and import keys describe the unit's
content and dependencies. Warm imports replay layout, ABI and library requests;
warm object hits skip placement, RC, reuse, Ready validation and emission.
`--validate-stages` always imports and recomputes object hits, comparing the
fragments with fresh emission.

Native objects use the same checked immutable artifact store as other products.
The configured local writable provider retains production associations; imported
content alone supplies no production authority. The object-directory format is
v3, with no reader for the former per-object framing or installed native seed.

The timing counts `native imports`, `import hits` and `import misses` distinguish
fresh SSA import from replay; `units` counts the placement-to-Ready pipelines run.

Link input digests are read once per file state and shared by native planning and
link command construction. Disposable `file-digests-v1` receipts live in the
private native cache. Reuse requires path, device, inode, mode, size, nanosecond
mtime and ctime to match; files changed in the last two seconds are re-read.
A miss verifies all bytes and checks metadata again after reading, retrying up to
three complete reads if metadata changes (including hard-link publication by a
shared build cache). Persistent changes or path replacement remain errors. This assumes
a local filesystem with coherent ctime updates, like Git's stat cache; receipts
are private to the current user and are not accepted as portable cache artifacts.
Both existing digest encodings are accumulated in one read to preserve build IDs
and native-plan identities. Missing, malformed or non-private receipts disable
reuse. Removing the digest cache changes performance only.
