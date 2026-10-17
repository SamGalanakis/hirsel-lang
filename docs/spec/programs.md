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

The launch input is a UTF-8 JSON object with exactly `schema: 1`,
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
 "dependencies":[{"manifest":"../stdlib/std@1/hirsel.json","package":{"name":"std","version":"1"},
                  "expose":[{"head":"json","module":["json"]}]}]}
```

## Dependency exposure

Each `expose` entry is exactly `{head, module}`, interpreted within that dependency's
package. It exposes a module head, never unqualified values. Exposures belong to the
importing package and never leak into another package's view. Same-package lookup precedes
dependency lookup. Several distinct package and module targets for one head are LANG-4 at
its use, even if only one target contains the requested member; repeated identical targets
are not ambiguous. Full versioned paths can name selected public definitions, including
transitive public signature types, without exposing a short head. Publicness, opacity and
internal ancestry remain [PROG-2](#prog-2-visibility). Only the compiler's `core@1`
supplies nominal roles; a source package cannot select `core@1`, and names such as
`Result`, `Io` or `std` grant no roles or privileges. Core module heads keep their default
exposures unless an explicit dependency head replaces them.

## Launch

`hirsel check launch.json` and `hirsel run launch.json` (interpreted or native) use this
graph and its entry. A command on a source file uses that file's exact `file.hir.json`
sidecar when present, else `standalone@0`; no parent manifest is searched for. A sidecar
must assign the source to its root package. Formatting operates on one source file and
needs no dependency bodies. Capability implementations and simulation inputs are separate,
explicit launch inputs ([run configuration](determinism.md#run-configuration)); selecting a
package grants no authority. An explicit `--run /absolute/run.json` selects freshly admitted
host Io in either engine. Without `--run`, source-run tooling keeps its simulated world:
captured stdout, empty virtual directories and no network. This simulation grants no real
host authority.

The standard library is the source package `stdlib/std@1/hirsel.json`
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
change it. The representation, and a sealed capability's implementation authority ceiling,
are visible only in this owner's view. [DROP-4](failure.md#drop-4-commit) and resource
destructuring keep their defining-module restrictions.

### PROG-3: Entry

An entry is any `pub fn` whose parameters are root capabilities (`Io`, `Args`, `Env`,
`Unsafe`) and which returns `Result[(), E]`. A completed entry returning `Ok(())` exits 0;
`Error(e)` exits 1, independently of `E`'s variant numbering and without implicit output.

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

### PROG-4: Interop

`extern "c" fn` definitions require `Unsafe` and use parameter modes: a borrow means C does
not retain the value; `take` transfers it, released through the runtime. Their types are
limited to `@layout(c)` data, flat values, sized numerics, raw pointers and byte views.
Callbacks are non-capturing `FnPtr`s with a context passed `mut ctx: unique C`. Foreign
handles are resources. A safe wrapper declares its kind and whether it has a deterministic
model; those declarations are trusted. Packages containing externs declare `unsafe ffi`.
Foreign declarations are monomorphic and state an explicit aliasing contract. An
`extern` outside a package marked `unsafe ffi` is PROG-4.

**Unspecified:** the `unsafe ffi` package metadata, wrapper kind and model metadata, and the
complete foreign ABI for `FnPtr` callbacks, exports and retention.

### PROG-5: Topology

A program may be compiled as any partition of its definitions into units, from one
definition per unit to the whole program, each unit closed-world inside its declared
boundary. The partition never changes what is accepted or what a program means; calls
across a unit boundary use the calling convention that signatures define. Topologies, unit
settings and caching are tooling.

### PROG-6: Grant admission

Optional not-before, expiry and revocation conditions on launch grants are checked at
admission only and recorded as admission evidence. They do not silently revoke an admitted
in-process grant. A policy that requires later revocation is implemented as an explicit
recorded capability input, or refused. Replaying an accepted admission does not revalidate
it against the current host, while beginning or resuming real host execution requires a
fresh admission under the host's authority ([admission](determinism.md#run-configuration)).
