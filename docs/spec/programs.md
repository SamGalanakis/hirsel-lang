# Programs and packages

A program is a set of top-level definitions, not a set of files. Definitions belong to
packages at exact versions; a JSON launch manifest assigns every source file its package
and module and selects dependencies explicitly. Nothing is discovered from the file system
or downloaded.

### PROG-1: Names

A name is a path within a package (`pkg/path.name`); dependencies are named packages at a
version. Every type a dependency's public signature mentions is nameable by its full path
(`json@1/Value`). Within a program, a definition is identified by its fully qualified
package, version, module and item path. Content digests identify revisions or artifacts and
never establish nominal type equality. One implementation revision is selected for each
definition in a program. Two versions of a package define distinct types.

**Revision swaps.** Hirsel has no live-swap feature today: a run executes the one revision
selected at launch. Any future feature that replaces revisions during a run must obey this
rule. A definition's selected revision may advance only at recorded swap points. A named
function value follows its definition, so later calls use the current revision. A closure
keeps the revision that created it. A run's tape records every swap point.

Core nominal roles use the compiler-supplied `core@1` definition paths: core types are
rooted directly under `core@1`, and operations are `core@1/module.item`. A source file
checked on its own belongs to the package `standalone@0`. The manifest and dependency
exposures supply identities, never file discovery or spelling.

## Package selection

The launch input is a UTF-8 JSON object with exactly `schema: 1` (or `schema: 2`, below),
`package: {name, version}`, `sources: [{path, module}]`,
`dependencies: [{manifest, package, expose}]`, and a launch-only `entry: {module, name}`. A
dependency manifest may omit `entry`. Names, the entry name, module segments and exposed
heads are ASCII identifiers. A version is a canonical decimal `u64` major (no leading zero
except zero itself), compared exactly, with no ranges or latest-version resolution. Paths
are resolved relative to the declaring manifest, never relative to a source module.

Each source has one explicit module-segment array (empty for the root); directory names and
file names never enter nominal identity. A source path may be assigned only once. All
selected source declarations are indexed before any header or body query. Each
dependency's requested package and version must equal its manifest's identity. Selecting
the same canonical manifest twice is one package. Different manifests selecting one package
version, cycles, absent files, duplicate object keys, unknown or missing fields and
malformed metadata are tool-input errors, not language diagnostics.

```json
{"schema":1,"package":{"name":"tool","version":"1"},
 "entry":{"module":[],"name":"main"},
 "sources":[{"path":"main.hir","module":[]},{"path":"text/wrap.hir","module":["wrap"]}],
 "dependencies":[{"manifest":"../stdlib/hirsel.json","package":{"name":"std","version":"1"},
                  "expose":[{"head":"json","module":["json"]}]}]}
```

### Implementation slots

A `schema: 2` manifest has the schema-1 fields except the edge form, plus `parameters`
and `dependency_catalog`. `dependency_catalog` maps a frame-local edge ID to exactly
`{manifest, package}`; a catalog entry alone selects nothing. `dependencies` is an array of
`{use, bindings, expose}`: `use` names a catalog ID (at most once), `bindings` is a
whole-slot override object (empty allowed) and `expose` is as above. `parameters` maps a
slot name to `{signature, default?}`; a slot without `default` is required. A declaration
reference is `{dependency, module, name}`, where `dependency` is a catalog ID of the frame
the reference is written in, or `self`. A binding is exactly `{"implementation": reference}`
or `{"parameter": {dependency, name}}`, which forwards the whole effective slot `name` of a
dependency the frame selects (or of its own slot, with `self`). There are no partial maps,
expressions, positional actuals, implicit provider search or root precedence.

An edge's actual resolves in the frame that writes it; an omitted slot's default resolves in
the provider's own frame. An overridden default is never evaluated, so its catalog target is
never opened, though every reference is still checked against its frame's catalog, edges and
slots. A used reference to an unselected catalog ID selects that target as an edge of its
frame, with the target's own defaults and no exposures; a signature reference always does.
Forwarding must be acyclic: a cycle is refused naming its slots in order. A required slot
with no actual, a binding of an undeclared slot and a reference or forward its frame cannot
name are tool-input errors; each names its selecting path.

Each nominal package/version has exactly one effective slot map per program. A map is
normalized to terminal declarations, `(package, version, module, item)`, in slot-name order:
catalog aliases of one package, forwarding and explicit or omitted defaults that reach the
same declaration are equal, while separately named equal-bodied implementations are not.
Every selecting edge must yield an equal map; otherwise loading refuses, naming the two
shortest selecting paths (ties by spelling) and both maps, independent of manifest and edge
order. Each slot's signature and implementation must name declarations the slotted package
can see ([PROG-2](#prog-2-visibility)). The signature must be a `signature` and the implementation an
`implementation` of exactly it, accepted by the [GEN-6](generics.md#implementation-contracts)
matcher; otherwise loading refuses naming the slot, and for a matcher refusal its member
and rule. Schema-1 manifests declare no slots, and a schema-1
edge leaves every slot of its target at its default.

```json
{"schema":2,"package":{"name":"mac","version":"1"},"sources":[{"path":"api.hir","module":["api"]}],
 "parameters":{"H":{"signature":{"dependency":"std","module":["digest"],"name":"Digest"},
                    "default":{"implementation":{"dependency":"std","module":["sha256"],"name":"sha256_impl"}}}},
 "dependency_catalog":{"std":{"manifest":"../stdlib/hirsel.json","package":{"name":"std","version":"1"}}},
 "dependencies":[{"use":"std","bindings":{},"expose":[]}]}
```

## Dependency exposure

Each directly selected dependency implicitly exposes the first module segment of its
declared source modules. Each `expose` entry adds a `{head, module}` alias within that
dependency. It exposes a module head, never unqualified values. Exposures belong to the
importing package and never leak into another package's view. Same-package lookup
precedes dependency lookup. Identical targets deduplicate; distinct targets remain
ambiguous independent of member membership, except matching core/std modules merge as
[LANG-4](language.md#lang-4-names) specifies. Core module heads keep their default
exposures; a non-std dependency head can replace a core default as before. Full versioned
paths still reach selected public definitions, including transitive public signature
types, without exposing a short head. Publicness, opacity and internal ancestry remain
[PROG-2](#prog-2-visibility). Only the compiler's `core@1` supplies nominal roles; a source
package cannot select `core@1`, and names such as `Result`, `Io` or `std` grant no roles
or privileges.

## Launch

`hirsel check launch.json` and `hirsel run launch.json` (interpreted or native) use this
graph and its entry. A command on a source file uses that file's exact `file.hir.json`
sidecar when present, else `standalone@0` with the toolchain's `std@1` as its only
dependency; no parent manifest is searched for. A sidecar
must assign the source to its root package. `hirsel fmt examples/configctl/hirsel.json`
formats only that package's explicit direct sources, in manifest order with canonical
paths deduplicated; no entrypoint or dependency bodies are needed. `hirsel fmt file.hir`
formats just that source. `--check` never writes and exits 1 for differences or 0 if
unchanged. Every selected source is read and formatted before any writes; unchanged
files are skipped. Write-time I/O errors are reported, but multi-file filesystem
atomicity is not promised. Capability implementations and simulation inputs are separate,
explicit launch inputs ([run configuration](determinism.md#run-configuration)); selecting a
package grants no authority. An explicit `--run /absolute/run.json` selects freshly admitted
host Io in either engine. Launch defaults and argument snapshots follow
[PROG-3](#prog-3-entry). A simulated world requires explicit `--simulation <json>` and is
available only in the interpreter; it grants no host authority.

The standard library is the source package `stdlib/hirsel.json`
([standard library](std-library.md)). It is selected by the same manifest mechanism as any
third-party package, can be replaced by another ordinary source location, and has neither a
builtin fallback nor a registry entry. Source bodies keep their source origin through
native specialization and cannot emit library-only representation instructions. File
partition and native unit topology change neither package identity nor meaning.

### PROG-2: Visibility

Definitions are package-private by default, `pub`, or `pub opaque` (constructors and fields
hidden; bounds stated). `internal/` path segments are visible only beneath their parent. A
public signature cannot mention a private type. The opacity owner is the defining package
at the exact selected version; file, module partition and compilation topology do not
change it. A `signature` or `implementation` is private or `pub`, never `pub opaque`.
An implementation's type member is a transparent alias of the type it names, so
it exposes no more than that type: an opaque provider type stays opaque through it, and a
dictionary's `of` identity is unchanged ([GEN-6](generics.md#implementation-contracts)).
The representation, and a sealed capability's implementation authority ceiling,
are visible only in this owner's view. [DROP-4](failure.md#drop-4-commit) and resource
destructuring keep their defining-module restrictions.

A package's effective [implementation slot](#implementation-slots) map never enters nominal
identity: definition paths, opacity owners, brands, codecs, reflection, definition hashes and
the run revision a tape binds are those of the unconfigured package. The map keys only the
package's own checked products. Selecting a provider grants no capability: every effectful
operation still declares and receives its authority.

### PROG-3: Entry

An entry is any `pub fn` whose parameters are root capabilities (`Io`, `Args`, `Env`,
`Unsafe`) and which returns `Result[(), E]`, `std@1/process.ExitCode`, or
`Result[std@1/process.ExitCode, E]`. A completed entry returning `Ok(())` exits 0;
`Error(e)` exits 1, independently of `E`'s variant numbering.

`std@1/process.ExitCode` is an opaque unsigned byte status. `process.SUCCESS` is 0,
`process.FAILURE` is 1, and `process.exit_code(n: u8)` constructs any status from 0
through 255. A bare `ExitCode` or `Ok(code)` exits with that status. `Error(e)` still
renders e and exits 1. A bare `u8` is not an eligible entry return type. The status is
a return value: normal cleanup and drops finish before the host exits; recording
and replay reproduce the same status without a new capability effect or trait.

A **call entry** (planned) is the callee of a runner call: any top-level definition an
`Entry` names ([RUN-3](runners.md#run-3-entries)). It is launched as its own run with
`launch` `call`, its argument from the run record's `call` member, its capabilities from
the place's grants and admission at the place ([RUN-10](runners.md#run-10-callee-runs)).
Its outcome is reported to its caller, not as an exit status or launch report.

Both interpreted and native launchers use the same run outcome and exit policy:

| Run outcome | Exit code |
| --- | --- |
| Returned `Ok(())` | 0 |
| Returned `Error(e)` | 1 |
| Returned `ExitCode` or `Ok(ExitCode)` | its unsigned byte value |
| Language failure (FAIL-1), including cleanup causes | 2 |
| Refused launch or request, replay divergence, or tooling failure | 3 |
| Blocked (CONC-8), with its parked waits | 4 |
| Host run budget exceeded | 5 |
| Environmental stop, including codec or recording limits | 6 |

Artificial interpreter limits, detected undefined behavior and checked-execution
traps are tooling stops (3), outside language failure semantics. A run stopped as
Blocked or by a host budget performs no further cleanup. Hosts receive
`hirsel_run_config::RunOutcome`; standalone launchers, including `hirsel run`,
obtain their process status only from `RunOutcome::exit_code()`. `hirsel run --json` reports a returned status as
`{"schema_version":1,"run":{"outcome":"returned","exit_code":n}}`. Source rejection
remains a compiler-command error (1); invalid command arguments remain usage
errors (2).

`hirsel test` and `hirsel test --native` run every selected test independently,
then apply the same exit policy to the suite's aggregate result: `Ok(())` when
all tests pass and `Error` when any test fails. A test's failure report retains
its individual run's cause, including an individual test's tooling stop. Suite
setup or reporting errors use code 3.

When `main` returns `Error(e)`, the host writes `error: <e>` followed by a newline
to standard error before exiting 1. String error values render as text; other error
values render in debug/value form. This launch report requires no stderr grant and
is not program output. The report is written after the program's buffered stdout
is flushed. Replay reproduces it from the returned value, without an additional
tape event. After writing the report, the host finalizes the returned Result,
including its error value. Finalizer output follows the report in both engines.

```hirsel
pub fn main(rt: Io, args: Args) -> Result[(), OutputError] {
  let out = match io.stdout(rt) { Ok(s) => s, Error(_) => return Ok(()) }
  out.print(out, "hello\n")
}
```

A single validated run configuration fixes the selected program revision, the entry and
launch kind, the capability providers and effective grants, explicit `Args` and `Env`
snapshots, and independent hash, Random and Test seeds. Capability admission completes
before entry; an absent, unenforceable or unauthorized grant is a launch error. Package
selection alone grants nothing, and a serialized grant description is inert input to the
launcher, never authority itself. An omitted grant record means the empty grant set;
requesting an absent root capability is a launch error before entry. A standalone
executable does not implicitly grant its working directory, process arguments,
environment, standard streams, clocks, network or `Unsafe`. Empty `Args` and `Env` are
explicit grants. An explicitly granted `Io` exposes only the projections its validated run
configuration enumerates; a missing projection is an expected grant error, not a
fabricated capability. The ambient heap permission is unchanged. Launch, header and
admission refusals are tooling errors before entry, not source diagnostics or language
failures.

An entry may return `Result[(), String]` to report invalid application input:
`Error("bad input: expected one decimal number")` is an ordinary returned error
and exits 1; `Ok(())` exits 0. Helpers returning `Result[T, String]` propagate
through plain `?`. Applications need no fabricated host file error and no new
standard error wrapper. Returning an error is not a language failure.

A launcher may bind a grants template to the selected program in one step
(`hirsel run <source> --grants <template>` or a standalone executable's
`--hirsel-grants <template>`). A template supplies only the schema-1 run fields
`inputs`, `io`, `args`, `env`, `unsafe` and `admission`; the launcher supplies
`schema`, `program`, `entry` and `launch`. Omitted fields use denial defaults and
zero seeds. Binding fields, unknown or duplicate fields and invalid grants refuse
launch. The completed record follows ordinary validation and admission. Standalone
arguments after the template path replace its Args snapshot as with `--hirsel-run`.
For `hirsel run --grants`, arguments after `--` replace the template's Args with
that exact UTF-8 snapshot, including an empty snapshot. Without `--`, the
template's Args remain. `hirsel run --run` keeps the bound record's Args
authoritative and rejects trailing arguments. A relative directory `host` in a
grants template resolves against the template file's directory, then becomes a
canonical absolute designator before record validation and host pinning. Bound
records continue to require canonical absolute hosts. An argument starting with
`{` supplies inline JSON to `--run` or `--grants` (standalone:
`--hirsel-run` or `--hirsel-grants`); otherwise it supplies a path. Inline templates
require absolute directory hosts. `hirsel run <source|manifest> -- args...` without
a record or template grants only Args, with that exact UTF-8 snapshot (including
an explicitly empty snapshot); it implies no other authority.

The interpreted host launcher (`--run` or `--grants`) has no default execution
step budget. Simulated CLI runs default to 1,000,000,000 steps. An explicit
`--max-steps <n>` selects a nonnegative u64 bound; `--max-steps unbounded` removes
the step bound. These are tooling policies, not language failures or authority.

Reference interpreter fixtures default to 1,000,000 TIR steps and call depth 256.
An instruction or terminator charges one step; an operation implemented by the
host completes within its call's charge. Source std functions execute ordinary
TIR and receive no discount. Dispatch specialization and checked execution keep
the same charges. Counts describe a particular lowered program, not a stable
source-level complexity measure or the host `budgets.limits@2` fuel unit.

Application tests that need more work must declare a finite bound per fixture
and pass it explicitly (Rust: `run_observed_snapshot_bounded` or
`run_observed_checked_bounded`). The returned `step_usage` records the requested
limit and consumed steps, including a tooling stop. It stays outside semantic
observations and tapes. The bounded helper restores the previous adapter policy
after success, tooling stops and API errors; it never changes the default.
Application bounds below the default are refused. Lower-level `Limits` may select
zero for an evaluator test; a run completing in exactly its bound succeeds.
A tooling stop has no completed observation and cannot count as a semantic pass.
Keep host CPU and memory caps independent of the step bound.

### PROG-4: Interop

`extern "c" fn` definitions require `Unsafe` and use parameter modes: a borrow means C does
not retain the value; `take` transfers it, released through the runtime. Their types are
limited to `@layout(c)` data, flat values, sized numerics, raw pointers and byte views.
Callbacks are non-capturing `FnPtr`s with a context passed `mut ctx: unique C`. Foreign
handles are resources. A safe wrapper declares its kind and whether it has a deterministic
model; those declarations are trusted. Packages containing externs declare `unsafe ffi`.
Foreign declarations are monomorphic and state an explicit aliasing contract. An
`extern` outside a package marked `unsafe ffi` is PROG-4.

**Package metadata.** A package manifest marks its package `unsafe ffi` with one more
field, `"unsafe_ffi": {"link": [...], "headers_checked": false, "threads": "home",
"wrappers": [...]}`. `link` lists the native libraries its externs resolve against, in
link order: a library name (`"z"`, linked as `-lz`) or a manifest-relative path to a `.a`,
`.o` or `.so` file. `headers_checked` states whether the declarations were checked against
the C headers; it is trusted and reported, never verified. Only the root launch manifest
may select a package marked `unsafe ffi`, as itself or as a dependency; a dependency
manifest selecting one is a tool-input error, so every foreign library of a program is
chosen at its root. `hirsel check` reports each `unsafe ffi` package in the selected
closure with its link inputs, `headers_checked` and the effective `threads` declaration.

The `unsafe_ffi` object gains the trusted member `threads`: `"home"` (default; the run's
admission thread), `"serial"` (any thread, one call chain at a time per package; a
callback's nested call to the same package re-enters) or `"any"` (any thread,
concurrently).

**Dispatch precedence:** a call inside an active foreign interval runs on that interval's
thread; otherwise a call to a `home` package runs on the admission thread, `@blocking` or
not, and occupies it until it returns; otherwise a `@blocking` call runs on the bounded
pool; otherwise it runs on the calling thread. A callback that can reach a `home` package
may be handed only to an extern whose effective thread is the admission thread, that is, a
`home` package's extern; otherwise the build refuses, naming both. Callbacks still run on
the calling thread during the call. The outermost entry of a `serial` package's gate is a
synchronising operation (CONC-4) and its order a scheduling input (S-3); nested re-entry
is not.

```json
"unsafe_ffi": {"link": ["z"], "headers_checked": false, "threads": "home",
               "wrappers": [{"module": ["zlib"], "name": "crc32", "kind": "pure",
                             "model": "deterministic"}]}
```

**Aliasing contract.** An extern's doc comment is its aliasing contract
([MEM-9](memory.md#mem-9-unsafe)); an extern without one is PROG-4.

**Wrappers.** Each `wrappers` entry `{module, name, kind, model}` declares an ordinary
function of the package a safe wrapper. `kind` is `"pure"` (its result depends only on its
arguments), `"handle"` (it acts on foreign state reachable from the handles it is given) or
`"effect"` (it acts on the host outside Hirsel's capabilities); `model` is
`"deterministic"` (its results and effects are a function of its arguments) or `"none"`.
Both are trusted. An entry naming no function of the package, or an extern, is a
tool-input error.

**Foreign ABI.** An extern is a direct call of the C function with its name in the
platform C calling convention. Capability parameters are not passed. A sized integer or
float passes as the C type of its width (`i32` as `int32_t`, `f64` as `double`); `Ptr[T]`
and `FnPtr[...]` as non-null pointers; `Option[Ptr[T]]` and
`Option[FnPtr[...]]` as the same one-pointer ABI (zero for `None`, the pointer for
`Some`); a borrowed `Bytes` as two arguments, `const uint8_t *` and
`size_t`; a `@layout(c)` or flat record as its address (read-only when borrowed, writable
for `mut`); and a `mut` number or pointer as its writable address. A result is `()`
(`void`), a sized numeric, a pointer or one of those nullable pointer options.

**Null at the boundary.** `Ptr` and `FnPtr` are non-null. Every pointer C hands to
Hirsel as one of those types is checked for zero before Hirsel uses it, including
extern results even when discarded, callback parameters and pointer words written
through `mut` arguments. Zero violates the foreign contract and ends the process
with PROG-4 and the active extern call's source site; invalid foreign storage cannot
be safely cleaned up. A nullable pointer option admits zero as `None` and otherwise
contains `Some` of the non-null pointer. Passing `None` to C passes zero. These
checks do not establish validity, lifetime or aliasing beyond non-nullness.

**Retention.** A borrowed or `mut` argument is valid only for the call; C must not retain
it. `take` passes only numbers and pointers, whose transfer carries no release, so C never
holds counted storage: taking a byte view or a record is PROG-4, and so is `mut` of a byte
view. A foreign handle is a raw pointer held by a resource whose `drop` releases it through
the library.

**Callbacks.** A callback type is `FnPtr[fn(mut ctx: C, ...) -> R]` whose first parameter
is the context, whose other parameters are numbers and pointers passed by value, and whose
result follows the extern rules. Hirsel forms a callback value by naming a non-generic
definition that is not an extern where a callback type of its shape is expected; a closure
or a generic definition there is a type error. C may also supply `FnPtr` values through
results, callback parameters and writable arguments, including their nullable options.
Every `FnPtr` is opaque and non-callable from Hirsel: it may only be passed back to C;
there is no unsafe indirect-call form. Wrapping a named callback in `Some` and using
`None` forms the corresponding nullable callback option. Any other
`mut` parameter of an extern is a context `mut ctx: unique C`, passed as an opaque
exclusive address that C only hands back to a callback; `unique` is canonical for a plain
`C`. Every other `FnPtr`, a closure type `fn(...)`, and a context that is not `mut` are
PROG-4. C calls a callback as `R (*)(void *ctx, ...)` on the calling thread during the
extern call. A language failure inside a callback ends the process, as a failing
`unsafe ffi` finalizer does ([FAIL-3](failure.md#fail-3-what-runs)): the C frames in
between cannot run cleanup.

**Exports.** Hirsel definitions are not exported to C: C reaches Hirsel code only through
callbacks.

**Determinism.** Foreign calls are outside DET-1. A run may be recorded and replayed only
if every definition that calls an extern is a safe wrapper with a deterministic model, and
replay calls that wrapper again; any other reachable use of an extern makes the program
non-replayable, and recording or replaying it is a launch refusal: `hirsel run` names that
use, and an executable built from the program refuses `--hirsel-record` and
`--hirsel-replay`.

**Engines.** The reference interpreter does not call C: launching a program whose entry
reaches an extern in the interpreter is a tooling refusal naming the extern, and the heap
executor refuses an extern call as unsupported. Native builds link the root's `unsafe ffi`
libraries after the runtime.

### PROG-5: Topology

A program may be compiled as any partition of its definitions into units, from one
definition per unit to the whole program, each unit closed-world inside its declared
boundary. The partition never changes what is accepted or what a program means; calls
across a unit boundary use the calling convention that signatures define. Topologies, unit
settings and caching are tooling.

### PROG-6: Grant admission

The host launcher reads explicit schema-1 `{"schema":1,"revoked":[reference,...]}` evidence once from `--revocations <file>` (`--hirsel-revocations` for executables); its file digest and reference decision are recorded, and a missing source refuses a grant requiring revocation evidence.

Optional not-before, expiry and revocation conditions on launch grants are checked at
admission only and recorded as admission evidence. They do not silently revoke an admitted
in-process grant. A policy that requires later revocation is implemented as an explicit
recorded capability input, or refused. Replaying an accepted admission does not revalidate
it against the current host, while beginning or resuming real host execution requires a
fresh admission under the host's authority ([admission](determinism.md#run-configuration)).

### Native artifact source paths

Native artifacts name source files as `<package>@<version>/<manifest-relative path>`
in profiling, failure tables and DWARF, independently of host checkout roots.
Standalone files use `standalone@0/<file name>`; generated bodies retain the
logical name with their generation suffix. DWARF's compilation directory is the
constant `/hirsel`; GDB substitutes the leading `<package>@<version>` in a
relative source filename with that package's local manifest directory. This is source-prefix mapping,
as with `-fdebug-prefix-map`; physical paths used for compiler diagnostics do
not contribute to artifact bytes. The native link kit and toolchain still
contribute to build identity.
