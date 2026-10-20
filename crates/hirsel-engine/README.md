# hirsel-engine

The query engine of the driver layer: per-definition queries and caching,
packages, the builtin registry, compile-time evaluation and the validated native
input. It reaches signature normalization, body checking, modes and lowering only
through the `Phases` trait object it defines; hirsel-driver implements it, and
`Compiler` builds fresh sessions from it. No implementation crate depends on
this crate.

Source indexing publishes declaration facts keyed by DefPath, with ordered name
sites for each owner. Resolver and signature adapters share one immutable module
index and metadata snapshot per manifest revision; header normalization reads
only its owner's sites. Explicit build-input changes still require a new manifest
revision. `work::read(Counter::ModuleDeclarationRows)` counts source fact composition for scaling
tests. The core registry shares its immutable signature map across adapters.

Query results are memos with typed reads (see "Memos and invalidation"). Provider
reads share the output; public query calls return owned output snapshots.
`work::read(Counter::CacheValidationRows)` counts deep verifications of marked memos.

Syntax and positions are item-local (design rule 6). `parse_file` retains each
item's `Arc<ItemSyntax>` when its DefPath, text hash and occurrence match an
earlier one, and only moves its anchor with `SourceMap::move_anchor`; a changed
item at the same (DefPath, occurrence) keeps the old anchor. Nothing downstream
holds a file offset, so no cached signature, resolution, body, mode result,
TIR or diagnostic is rewritten after an edit: inserting or removing other items,
or moving an item to another file, leaves every memo of the item in place.
Items removed from one file wait in a detached pool until the next parse, so a
cross-file move keeps the same syntax. Package images, prechecked records and
artifacts carry anchors as (source ordinal, base) and are placed on load, never
rebased.

Each revision computes one frozen `SignatureSnapshot` (`snapshot/`): every
signature by `DefId`, each opaque type's private view (seen only by its own
package's bodies) and V's verdict on every view. V validates a package's bodies
against `snapshot.validation(package)`, the public `ViewTable` with that
package's private views in place; compile-time evaluation validates a
materialized owner against `validation_with`. Only the replaced views and the
verdicts that read them are checked again, and a new snapshot reuses the
previous one's digests and the verdicts whose reads still hold. Failed headers
are retried: a snapshot is kept only when every signature normalized.

Compose phases through per-definition queries; record dependency fingerprints for later caching.

Queries, V, comptime and the program check carry `hirsel_intern::timings`
interval and phase guards and counts (docs/building.md, "Compile timings").
Dropping a `QueryEngine` drops its tables inside the `merge` interval; with a
heap meter installed (`--mem-usage`) it records the bytes each table frees, and
`release_shared_caches` records and frees the process's package caches and V
signature inputs once compiling is over. A process that turns
`free_sessions` off leaks its sessions' tables instead, unless a heap meter is
installed.

`LoadedProgram::check(LoweringPolicy)` selects which bodies a check lowers.
`Comptime` (`hirsel check`) lowers exactly the bodies compile-time evaluation
executes or materializes into: constants, bodies holding or reading a
compile-time value, the functions evaluation calls and the generic instances
CT-5 selects. `Reachable` (run, test, build, the harnesses) lowers every body of
the root package and the dependency bodies it reaches. Lowering builds no source
diagnostic except through compile-time evaluation (CT-1, CT-2, CT-4), so both
report the same diagnostics; the conformance harness checks each corpus file
under both and requires them equal. `Verify` is the session's one V switch:
`Compiler::with_verify` sets it, and `QueryEngine::run_v` is the only place V
runs or is skipped. Sessions without V use their own shared package cache, and
`precheck_package` refuses them.

`QueryEngine::semantic_program_revision(entry)` produces the shared run-config
`ProgramRevision` for interpreter, heap and native launch. It follows checked
signature/resolver reads, private representations, implicit finalizers, type fact helpers,
serializable checks and
compile-time execution reads; every reached source declaration contributes its
signature and complete token content, including private bodies and constants.
Comments/spaces, source positions, file order, compiler provenance and native
instances/options are excluded. Newline boundaries remain because LANG-1 uses
them. Unrelated source definitions do not participate.

Interpretation (`run_snapshot`, `run_observed_snapshot`, `run_observed_checked`)
enters every definition through the engine's `TirProvider`. Under
`QueryEngine::closure_assertions` (on in builds with debug assertions, and in
the conformance runner in every build) that provider refuses, as an operational
`semantic closure` error, any definition outside `semantic_program([entry])`:
finalizers, function values and generated instances included, whatever a run
executes must be in the program its identity names.

`QueryEngine::definition_hashes(roots)` gives every definition the roots reach
its LANG-6 definition hash, and each definition with TIR its call sites with
their callees' hashes (`definition_hash`, `package_definition_hashes` and
`hirsel hash` read it). A definition with TIR hashes its signature and TIR, a
source or prechecked type its owner view, and a registry operation, extern,
signature fragment or generic generated declaration its path, signature,
semantic tokens and reads. It is a query over existing TIR; nothing is cached
across calls. `definition_groups(roots)` returns the same closure's recursive
groups with each member's hash preimage, which `hirsel-code-store` stores as
code objects.

`QueryEngine::unsafe_reach(root)` walks that same closure breadth-first from any
root definition and returns the shortest path to the first extern or
`UnsafeOperation`-role core operation it reaches (CAP-6), or none. It is the
checker query behind `hirsel check --safe` and the `admit.safe@1` admission
policy; it adds nothing to TIR or LIR. `callee_facts(engine, entry)` lists, for each
definition the entry's closure references as a value (a runner call's possible callee), its
signature fingerprint, code hash and `unsafe_reach`, so a run's runner arrival checks the
callee's identity and `admit.safe@1` without the engine (`SimulationConfig.callees`).

Successful native import retains that same semantic graph privately in
`ValidatedNativeInput`. Its `semantic_program_revision(entry)` works after the
engine is dropped, closes only the requested entry's dependencies, and preserves
old frozen revisions across later source edits. Raw `NativeInput` has no new
claimed revision field; import derives the graph from the checked source again.

The engine's build script snapshots a conservative common runtime/core bundle:
interpreter, runtime (including its headers), the driver's host adapter, kernels, native library bodies,
core ledger/constant/protocol definitions, the included native operation-review
table and exact libc/Unicode dependency
versions/checksums. It uses the same shared graph encoder and only repository-
relative component names. All engines use this identical bundle, independent of
target/profile/topology. Runtime implementation or comment-only edits can
conservatively change it. Launch never reads checkout files to compute identity.
`DefPath` display emits the versioned path bound by `ValidatedRun`.
This identity is separate from `native::input_identity` and artifact provenance.

Allowed dependencies: the shared data crates, check-protocol, run-config, validate, interp, resolve, parser, lexer, fmt and graph, and hirsel-guard for the compiler thread's stack. Never signature, check, modes, lower, the native backends, the CLI or the conformance runner.

Invariants and boundary:

- DefinitionQueries exposes parse_file, item_signature, check_definition and lower_to_tir; production queries compose all landed M1a phases.
- CheckOutput owns the SourceMap used by all its diagnostics; the conformance adapter resolves lines from byte spans.
- DefinitionKey hashes exact item text, complete used signatures and environment, independent of file positions/topology (LANG-6, PROG-5).
- Dependency order and repeated reads do not change keys; conflicting hashes for one dependency reject a mixed snapshot.
- Successful and failed lookups depend on their observed result, including visibility, namespace and local reservations.
- Public phase traits are driver-layer adapter contracts; implementations expose equivalent free functions using data-crate protocols and never import the engine or driver.
- QueryError distinguishes language diagnostics, missing phases and operational failures. Recovery diagnostics may accompany parse output.
- Runtime inputs, hash seed and entry are explicit; stdout bytes are exact (#12). No implicit filesystem/network capabilities.

Phase traits describe driver-layer adapters. Implementation crates expose free
functions with the same shared inputs/outputs; the engine forwards lexer, parser,
formatter, resolver and interpreter traits to those it may depend on, and
hirsel-driver implements `Phases` for the rest. Neither depends on a phase crate
through the other, so the graph stays acyclic. Lexer,
parser and formatter may return `(value, Vec<Diagnostic>)` instead of importing
QueryOutput from types; the adapter adds the empty dependency list. CheckOutput
is the engine's tool-facing adapter. RunOutcome is re-exported from
hirsel-run-config, the single host outcome type shared with native execution.

The interpreter returns evaluator values and TIR failure anchors;
`project_outcome` resolves them into the shared outcome, including every cleanup
cause, parked wait and interpreter-only tooling diagnostic. GEN-5 graph assembly belongs to the
driver: `gen5_source_graph` reads each definition's checked instantiations (type and
size arguments) for the source verdict, and `InstanceGraph::native` adds the
finalizer, representation and materialized instances the selected TIR needs
(spec #87). Each `InstanceEdge` keeps its owning store and the callee's binder. Failed queries
must preserve dependency reads in the provider recorder even when QueryError
contains only diagnostics. Do not cache a partial result as a successful definition.

Implemented session API:

- `QueryEngine::new(QueryEnvironment, PhaseAdapters)` owns the immutable source
  map, the manifest index and per-definition caches. Call
  `parse_file` for each source snapshot with its manifest-assigned `FileContext`.
  Replacing a source path removes deleted names; adding names changes the identity
  revision. Package/module identity is explicit; standalone CLI files use
  `standalone@0` when there is no explicit launch input.
- `parse_definition` returns a view of the indexed definition's
  owning token/AST snapshot. File discovery scans the new file once; downstream
  queries reuse unchanged item snapshots. `recompute_count` counts definition
  view/phase computations, rather than file discovery scans.
- `resolve_definition`, `item_signature`, `check_definition` and `lower_to_tir`
  stop on the first error. The latter runs body check, modes, then lowering.
  Signature construction uses the revision-1 `SignaturePhase` adapter and never
  body inference. Successful and deterministic rejected queries are cached;
  operational and unavailable results retry. Each engine has a fixed set of adapters for its session.
- `dependency_reads` retains signature reads, failed signature requests and
  successful/failed manifest lookups even when a phase returns diagnostics.
  Cached keys validate every read signature and all explicit environment revisions;
  Lookup receipts are replayed against the current manifest; unrelated membership
  changes retain results. Module-head reads and same-module value probes are validated too. These reads follow decisions through check, modes and lowering.
  Function signature input text excludes the body but includes written contracts.
  An unchanged definition keeps its arena IDs. On indexing a new revision, the
  driver re-spans reused owners, diagnostics and lowered sites to current bytes.
  Copy-on-write owners keep already published snapshots immutable. This retains
  semantic results without translating their arena-keyed decisions. Changed or
  deleted inputs retire their caches; unchanged headers survive body edits.
- `compiler_adapters` wires the real lexer, parser, formatter, resolver, signature
  normalizer, body checker, authority, ownership, lowering and library interpreter. The
  lexer/parser traits return Result with QueryError as do semantic traits.
- `check_path`, `run_path`, `format_path` distinguish tool/IO errors from language
  diagnostics. `check_file` returns per-check completion beside diagnostics; a clean
  partial check is Incomplete, including an empty file. Parsing errors can
  already reject source; no partial clean result is accepted or executed.

`//:hirsel-driver-test-it` runs them. Fake-adapter integration tests cover order,
short-circuiting, body/signature invalidation, failed reads and snapshot retention.

The formatter adapter's additive `format_query` method translates the formatter's
`unchanged` fallback flag into NotImplemented. The frozen `format` method remains
available, and fake adapters need implement only that method. Production formatting
uses the fallible wrapper after rejecting parser errors, so unsupported syntax
cannot produce a false successful `fmt --check` result.

`CompilerState` retains shared Resolution outputs (reads, primitive/generic
bindings, contextual names, qualifier paths, local functions, reference scopes and
deferred names), together with the owning AST/item snapshots. Signature normalization
uses explicit Item/Header resolver views over the original syntax. Header traversal
excludes implementation/destructor/constant execution expressions while retaining
contracts, distinct facts and finalizer binders. Adapters pass the same Arc syntax
owner to both resolver snapshots. Sibling
header reads go through SignatureProvider, avoiding stale same-file shortcuts.
Duplicate items keep one DefPath plus addressable candidate ordinals; Metadata
receives every declaration; Item supplies exact name-token spans for LANG-7 diagnostics.
Recursive type groups (`queries/type_groups.rs`) come from the written type paths
of a package's type declarations and manifest lookups, never from a signature, with
Tarjan components and `hirsel_graph::scc_keys` over the members' paths and texts. A
member's signature query normalizes the other members at the stage the normalizer
asks for, through its own recorder (`SignaturePhase::staged_signature`, which
publishes nothing), and records every member's text among its representation reads,
so an edit to any member invalidates all of them. A failed probe of a member's
published signature, which may be the one under construction, is not a read.
The signature adapter preserves candidate source order when an edited declaration
and a retained declaration come from snapshots with differently ordered FileIds.
Explicit manifest exposures remain local build inputs; foreign metadata now uses
shared Metadata and semantic SignatureMetadata. BuiltinRegistry supplies signature-only fallback after program definitions.

The interpreter adapter's `run_query` receives SourceMap and retains output IO /
invalid-TIR errors as RuntimeError; `run` now maps those to OperationalFailure,
never NotImplemented. This source/output projection stays driver-owned. Sized
provider forwarding wrappers were removed; implementation functions accept dyn
providers. Artificial limits project to RunOutcome::ToolingLimit, with #34 cleanup
still provisional.

`SignatureProvider::signature` and `TirProvider::definition` return Arc aliases
SignatureRef and DefinitionRef. DefinitionQueries returns shared signature/lowered
results; SignaturePhase and LoweringPhase produce owned data, wrapped once at
publication. Resolver/normalizer snapshots and interpreter frames retain the same
handles. LookupOutput always carries the current manifest revision, including on
misses. Recorder operational failures override swallowed phase rejections and are
never retained in deterministic result caches.

`check_progress`/`CheckOutput.completion` report diagnostics so far and Complete or
Incomplete { phase }. The production runner uses this status instead of its global
complete hook; matching rejection diagnostics remain usable from partial checks.
check_snapshot is the stricter execution helper: it requires no errors and complete
checking. Source accept/run/fail uses the complete installed pipeline; compiler invariant failures remain tool errors.

Interface/query domains are version 2, matching semantic signature encoding v2.
Retained bridges: immutable syntax snapshots and header isolation, duplicate
candidate indexing, build-manifest exposures/FFI inputs, formatter fallback
projection and interpreter source/output projection. Native resolver envelopes have been retired; shared Resolution tables retain
their syntax owners.


Production body queries return `TypedBody`: the owning syntax/item snapshot,
checked tables, H plans and selected unit/bool rows. Mode queries consume it and
return `AnalyzedBody` with the refined cursor plans, formal frame results,
allocation promises and ownership facts. Each query
caches the complete artifact with its dependency reads; later phases consume its
retained syntax owner. Native controls use shared unit/bool constructors and retain H's
first-native-row selection. Cursor plans own their element and step layout once.
Authority runs before
ownership; both run on source errors and their diagnostics merge through the
single driver `select_diagnostics` policy. Operand prerequisites and specialized
owners precede independent source ordering. The frozen Diagnostic lacks obligation
kinds, so known producer messages distinguish generic copying from other bounds
and repeat invocation from required multiplicity.

`mode_definition` exposes the enriched body without requiring executable code for
a type or bodyless declaration. File checking obtains source diagnostics from all
independent definitions before lowering error-free executable bodies. `lower_to_tir`
validates before publication in every build. Violations are OperationalFailure tool
errors naming the definition and violation. No invalid TIR is published or executed.

`run_snapshot` prepares the interpreter with the entry signature, RuntimeTypes
from declared type signatures, and SimulationConfig. Production execution calls
`library::launch_with_runtime`; direct hand-built TIR clients retain the core
interpreter boundary. User finalizers refer to their lowered owner entry, while
core File/AtomicFile cleanup uses Library's native handle hooks. No execution
result is cached on a signature-only DefinitionKey.

The real-phase edit gate compares incremental TIR and every type graph against a
fresh engine, modulo definition-relative source locations and ephemeral TypeId
provenance. It checks body/signature edits, failed reads/recovery and unrelated
expression-index shifts. The opening source example still encounters phase-owned
invariant failures.

The mode adapter runs `hirsel_check::audit` after ownership and merges its source
diagnostics through the same docs/spec/diagnostics.md selection. Representation decisions consume the
checked source types and declared signatures before any TIR publication.

Compile-time scalar materialization runs after H/J/I and before K publication.
`QueryEngine::materialize_constant` evaluates through the same interpreter and
Library dispatcher as runtime, with no root capability grants and fixed seed 0.
Private zero-parameter evaluation artifacts carry the checked expression's result
type and are fully validated before execution; exported signatures and source
mode decisions remain unchanged. Constant evaluation caches its value together with the initializer substitutions.
The evaluator passes substitutions directly in the lowering artifact;
K consumes evaluated `Constant` plans, including
resolved constant references. Nested materializations run inside out. Dependency
cycles cite their earliest reference (CT-4); runtime failures become CT-1 at the
comptime expression, with the operative failure attached. Static errors retain
their usual owner.

`comptime_limits` supplies artificial interpreter guards. Compiler dependency
recursion also has a maximum depth of 32. `last_comptime_limit` retains the typed
limit for evaluation step exhaustion (CT-1 at the enclosing comptime expression)
and operational call/dependency-depth exhaustion. The numeric budget and charge
model remain unsettled (#7). Changed limits invalidate materializations and
lowered results, including cached step-exhaustion diagnostics. Operational
failures are retryable. Materializations and lowered clients record executed
implementations and the signatures used to check them. Nested evaluations and
cached constants pass those reads to their callers. A same-signature body edit
invalidates its executed users; unrelated edits retain both values and lowerings.
Execution values are never reused on signature fingerprints alone. Aggregate materialization transports typed constructor data for tuples, records,
distinct values, variants, sequences and boxes. Declared fields are imported and
generic arguments substituted in the driver; K imports this producer graph and
checks its root against the existing expression judgment. Finite data trees are
reified and lowered with worklists, without a data-nesting cutoff; this is
separate from the operational compiler dependency-depth guard. Explicit integer Order
constants materialize their declared record with a private named comparator field;
K imports that function type and emits its ordinary capture-free forwarding body.
Well-typed resource/capability/closure results, including nested fields, report
CT-2 at their initializer or explicit comptime expression under conservative spec
#44. Static errors retain their ordinary owners. Partial or inconsistent evaluator
values and layouts unavailable to the initializer remain operational failures.
The manufactured aggregate boundary tests are green; source tuple/Percent/Result
construction drift is dependency-owned K, while distinct fact execution is N.

Distinct `where` code has a private owner-derived `$fact$0` snapshot, containing
only the declared predicate as a function body. Its parameter/result convention
and fingerprint come from the owner's signature. It goes through ordinary body,
mode, lowering and full validation queries, and remains inaccessible to source
identifiers. Static checking also visits unused predicates. Constructor plans
call the predicate before creating the value and check FACT-2 at the construction
span; runtime metadata and the interpreter require no contract special case.

The ownership adapter forwards I’s shared `OwnershipFacts` directly to K.
Source operand evaluation and callee consumption are distinct; the adapter
performs no passing-mode inference.

Comptime runtime metadata names K's published `entry_body()` layout without
lowering every finalizer in the program. The evaluation provider lowers and checks
an actual finalizer only when the interpreter schedules it. This prevents
unexecuted finalizer bodies from adding false compile-time dependency cycles;
executed cleanup keeps the runtime scheduler and ordinary failure policy.

Generic graph substitution takes the declared `NominalRoles` and normalizes
`FunctionType.error` from its substituted result, including nested callbacks.
The shared `result_error` helper recognizes Result by nominal identity. Pure
imports preserve existing channels; body checking needs no post-substitution repair.

Mode wiring forwards shared unique and guarded-pattern rows unchanged. Capture
creation context also uses the shared type, with no enum translation.

Core registry property summaries use hirsel-types after declaration snapshots
are available. Builtin spelling allowlists for Plain and aggregate Sendable have
been removed. Review-table declaration text is unchanged; derived semantic
fingerprints reflect the shared summaries. Driver phase wiring does not choose
property semantics or repair validator evidence.

Canonical nominal roles include Tasks and Scope and flow through Metadata into
signatures. Registry/query domains are v5. The registry and checkers use the shared
builtin held-authority rule; Tasks/Scope are no longer J intrinsic role strings.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped `RelationPlace { parameter, fields }` selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; it is also valid on function values under revision 6.
The validator derives call results independently from their declaration.

The heap keyword uses the private registry operation and normal call validation. Canonical v6 fingerprints include allocator roles and facts; aliases retain the fact independently of value storage placement.

Capture modes pass from J to both I and K as shared rows. Call-argument uses and the result/allocation facts travel in the owned analyzed artifact without conversion.

C3 publication validates declared call origins against semantic binding and storage
flow; CallOrigins is removed and no origin relationship remains unchecked. The
registry/query domain uses v7 fingerprints, including ArgumentBrand selectors for
position cursors. Pipeline mutations cover loop/closure incarnation rules and
closure invocation after the retained owner ends.

The field-path origin regression validates Log.file of .dir against Log.dir. A
paired TIR mutation changes both projected types and access tags to another Log
parameter and is rejected by independent binding/storage validation.

`run_observed_snapshot` exports semantic observation v1 with the canonical result
type, complete indexed failures, ordered effects and final simulation state.
Driver runtime and comptime execution select `TraceMode::Effects`, retaining every semantic effect without storing per-instruction diagnostic events. Production conformance compares this record. The ordinary CLI exit/failure adapter
projects the same execution; tooling limits never become language observations.

`run_observed_checked` executes the same program under checked execution:
`checked_runtime_types` adds the declared facts it
enforces, `@noalloc` definitions/destructors, complete library signatures and
`arena.with`, to the ordinary runtime metadata. It returns the same observation,
or `RunOutcome::Trapped` naming the broken memory-model rule.

`owner_representation(owner, definition)` requires the matching defining owner and
exports the retained private graph with its independent revision. Public signature
queries preserve opaque identity. Layouts without a source shape are unavailable.

Resolution, body/mode checking and V use package-scoped views of retained opaque
owner records. These views keep the one public fingerprint and never enter public
signature queries, normalization or native signature snapshots. Private source
receipts independently invalidate owner body caches; exact private graph equality
retires V proofs. Initializer materialization uses the same explicit package view,
and interpreter runtime metadata retains private field counts. Native layout/H
continue to consume the existing private representation records. Package versions
are distinct owners. LANG-13 still restricts destructuring a type with drop to its
defining module, even within its package.

`export_native_input` retains a complete checked program selection in owned
snapshots: the semantic closure (the program revision's) of every definition
outside the dependency packages, so a dependency definition the program never
reaches is never compiled natively. `import_native_input` rechecks it against current sources and produces
`ValidatedNativeInput`; a backend can retain that artifact after dropping the
query engine. The snapshot carries the native GEN-5 `InstanceGraph` and one
`SourceAudit` per executable definition (noalloc, cleanup and invocation promises);
import recollects both. Forged flags, graphs, audit records, materializations or
stale bodies fail closed, and a growing native instance graph is unavailable.
The snapshot also carries the CT-5 `InstanceBodies` table, recomputed on import
and checked by `hirsel_validate::validate_instance_bodies`.

A generic generated declaration (`queries/generation.rs`) is checked bodyless and
is never lowered (`generation::executable`). `select_generated_instances` walks
checked calls and instantiations from roots under closed bindings and stages
each new instance as a hidden private item (`name$instance$<hash>`) whose source
is the declaration's text with its generic list dropped and parameters spelled,
padded to the declaration's lines; the semantic program and lowering repeat the
walk until no instance is new. At most `MAX_GENERATED_INSTANCES` (256) per
declaration, then CT-1. Re-indexing a declaration's source retires its instances.

A *precheck* (`queries/precheck.rs`) is a dependency package checked once, at
toolchain build time, instead of by every program: `LoadedProgram::precheck`
writes it after a complete check with no diagnostics, refusing a package whose
queries read outside its closure. It holds, per definition, the semantic-revision
node and the lowered, validated TIR (spans by manifest ordinal, TypeIds given
fresh provenance on decode), completed native source audit, compile-time
substitutions and evaluated constant with its execution reads, decoded on first
use. Native export reads these audit products instead of repeating H and modes;
constant queries retain their original execution dependencies. Lowering replays
the execution reads it adds to enclosing frames. Its key is the compiler
identity, the package's checked-input revision and the compile-time limits. Loading
adopts `Prechecks::toolchain()` (the toolchain's `$HIRSEL_PRECHECK` or
`hirsel-std.precheck` beside the executable) by default, `Prechecks::of` an
explicit set, and `Prechecks::none()` checks every package from source. With a precheck adopted, `check` skips the
package's files, GEN-5's program graph covers only the other bodies (a
prechecked package reaches nothing outside its closure, so it closes no cycle
through them), `lower_to_tir`, `semantic_program`, source audits and constant
evaluation take its products, and other queries still compute from source, so
diagnostics, revisions and replay identity are unchanged. Re-indexing one of its files drops the precheck.

The single-threaded engine lives on a compiler thread. Use
`with_compiler_stack(|| { ... })` to construct the engine and keep its complete
incremental session inside that closure. The CLI and conformance runner establish
this boundary; path checking/formatting establish it for library callers too.
The helper states the stack size once and reuses an existing compiler thread.
Parser construction bounds syntax height (including loop-built binary/postfix
nodes) at 1,024; exhaustion is a deterministic `parser-tooling-limit`, never a
LANG-1 rejection or a partially published tree. Explicit source grouping also
counts toward this implementation safeguard, although grouping has no AST node.

Native snapshots retain the registry's operation inventory. Import rejects a
forged inventory; availability comes from executable dispatch tables and declaration
identity comes from the canonical header digest.

Registry schema constraints may require an existing language Bound at instantiation.
They name a TypeParameterRef from the exact declaration binder and carry the
diagnostic rule; E/H reuse their ordinary proof with that actual type. Encoding
and independent validation reject foreign Body/depth/ordinal references.
SortedMap insertion takes its key and value, including resource keys; no
Copyable constraint is added beyond its written signature.

Execution API errors retain prior stdout in ExecutionError. Launch carries the
original categorized cause plus output; ObservedExecution.outcome is an inner
Result, with tooling_output when no semantic observation exists. Pre-execution
launch errors stay outer errors. Ordinary run adapters write captured output
before returning the cause; unavailable execution remains exit 2.

Violated executable unchecked preconditions stop with UndefinedBehavior carrying
`undefined behaviour detected`, operation, source site and contract. Prior stdout
is retained, but DET-1, semantic parity and cleanup guarantees do not apply (#49).
No language failure is synthesized; observable returns None and CLI exits 2.
Malformed operation schemas remain API errors.


`ObservedExecution.exit_report` is the optional PROG-3 host diagnostic derived from the interpreter's returned value. It is outside the semantic observation and captured program output.

## Syntax review dispositions

The nine grammar/spec questions retain existing behavior where the documents do
not decide it.

| Question | Disposition |
|---|---|
| Nesting | Parser source/tree limit 1,024, distinct tooling outcome; explicit compiler thread stack. Spec #33 records the policy and measurement. |
| Label order | Written order retained; spec #51 asks whether labels may be reordered. |
| Float overflow | Existing infinity behavior retained; spec #52 asks about finite-literal overflow. |
| `_` parameter names | Grammar requires a name and reserves `_` for discard patterns; rejection retained. |
| Grouped `return` newline | Grouping treats newline as whitespace; retained pending spec #53. |
| Variant/function name collision | Expected variant precedes same-named package function; retained pending spec #54. |
| Namespace shadowing | Grammar forbids local/value shadowing only; other namespaces retain independent lookup. |
| Malformed-syntax rule IDs | Existing specialized IDs retained; spec #55 records section 23 versus corpus obligations. Missing closing boundaries now have empty spans. |
| Canonical formatting | Equal trees and idempotence enforced across every parse-clean corpus file; spec #56 asks about original-byte fidelity. |

Versioned paths stop before division after a dotted member or before a non-name
operand, as the grammar already requires. The ambiguous `pkg@1/a / b` spelling
retains greedy module separators pending spec #59; type-path separators remain
unchanged. Contract `result`/`before` are rejected outside `ensures` with FACT-1;
nested function bodies establish their own contract context. Mutual aliases are
LANG-7 at the alias target; direct self-alias layout keeps MEM-7. Invalid UTF-8
is SEM-3 at the first invalid boundary before indexing, with original prefix
coordinates. Brace recovery counts consumed and skipped nested braces.

Nominal expansion proofs retain separate conservative policy bounds in
hirsel-types: structural properties cap distinct nominal applications at 128,
and Data/Key declaration proofs cap expansions along a path at 128. These can
be reached through shallow syntax referencing many declarations; they are not
parser nesting safeguards. Their policy is owned by the shared type contract.
The private MapInsertionOrder/SortedMapOrder cursor declarations are consuming only and retain distinct owning cursor representations. Borrowed and mutable map iteration uses noalloc private length declarations plus protected entry projections. Registry declarations and dispatch-derived operation availability include these length operations.

AnalyzedBody retains I's shared `OwnershipFacts`, including J's identity introductions filtered by I's initialized-storage proof. Lowering reads those rows directly, without a driver filter or Plans copy. Independent validator mutation tests reject alias introductions substituted for required fresh joins.

Identity introductions are restricted by I's initialized-binding proof at the same expression convergence or loop head. Returning paths do not enter that fixed point; bindings moved on any incoming path receive no introduction. K transports the remaining J/I rows, so naming a join never reads unavailable storage.

Materialization reifies data-only Map and Set values as explicit TIR constructors,
retaining insertion order and omitting removed entries. Checked key/value rows
supply Map entry tuples with their own canonical summaries. Native snapshots carry
these constructors unchanged and reject forged materialized keys. Dictionary-bearing
SortedMap and non-data payloads remain CT-2 under spec #44.


TirProvider serves shared hirsel-tir Materialized snapshots and the complete
constant initialization order. The interpreter decodes compiled values once before
entry, with dependencies first and source-order ties (#63); it runs no initializer
user code at startup. CT-1 providers enumerate no unrelated constants and request
only their dependency closure. Native snapshots retain the same data rows and
order, including core dictionary constants. Import rejects a forged order.
This uses signature v9/query v10/interface 8 domains.

O lowers an inline comptime child into a closed zero-input initializer. It keeps
only contracts whose callable owners are declared inside that child; helper
calls resolve their ordinary signatures and contracts. H rejects ambient runtime
bindings and abstract generic mentions before this conversion (spec #65).

The scaling gate successfully lowers and independently validates accepted source
trees at the parser's 1,024-height boundary: nested blocks, nested calls, else-if
chains and binary chains. Each engine lives entirely on the compiler stack. This
complements the allocation-site stack-margin test that deliberately stops at the
deepest missing lowering decision; boundary success is checked through the real
query and publication path. Cold source regressions count actual pass work,
without depending on elapsed time. Literal height 1,600 remains the parser's
operational rejection; bounded chains with 1,600 total statements are measured
separately.

`tools/capped "$(.buck2/bin/buck2 build //:hirsel-driver-example-corpus_dump --show-full-simple-output)" <source> --canonical`
compares every root after canonical type numbering. The helper partitions complete
type graphs, including every qualifier and recursive edges, then numbers equal rows
at their first occurrence in the stable dump and prints the reachable type rows.
Unused arena rows and duplicate row allocation do not affect that comparison.
Without `--canonical`, dumps retain their original type indices. Diagnostics and
query errors are always emitted without normalization. The comparison regression
checks duplicate sharing, a changed qualifier, recursive graphs and a definition
whose name resembles a type token.

## Native stages (`src/native/`)

`native::build` lowers a validated snapshot to a Ready program for one target:
the snapshot is adapted to the plain program the native passes read (`adapt`),
then import, placement, insertion, elision, reuse and Ready lowering run per
unit, and each stage's output is used only after that stage's validator in
`hirsel-lir` sealed it. `Options` selects the target and whether regions,
elision, reuse and flat walking run; none changes what a program observably does. The driver
depends on `hirsel-llvm` unconditionally, so building it needs LLVM 21.1.8 at
`/usr/lib/llvm-21` (or `LLVM_SYS_211_PREFIX`).

A native failure is a `NativeError` naming the stage, the definition and whether
a validator (rather than a pass) refused. It is a tool outcome, never a source
verdict: the same static diagnostics come first, and a program native code
cannot build yet still checks and runs on the interpreter.

Native process entries must be public, complete, monomorphic functions returning
core `Result[(), E]`. Zero-parameter entries use `hirsel_rt_v2_launch_zero`, with
no host-root acquisition; Io/Args and Test entries retain their declared address
passing modes.

A Ready program runs three ways, each producing one `Observation` (schema
`OBSERVATION_SCHEMA`: exit code, output bytes and the ordered failures with
rule, definition, line and column):

- `run_heap` / `heap_campaign`: the checked heap executor, the second with its
  fault-injection campaign. Io is independently modelled for stdout/print, paths, directory/file leaves, MonotonicClock/WallClock/Random and typed Test launches; H owns handle cleanup and iterator environments.
- `link` and `launch`: `emit` writes one object per group of a `Partition`
  (`Whole`, `Singleton`, `Random(seed)`), a `main` shim calls the runtime
  launcher with the entry, the link manifest is checked (each export defined
  once, each import resolved, every runtime primitive call matching its export's
  physical signature), and the executable is linked against `libhirsel_rt.a` for
  x86-64 or AArch64 (run under `qemu-aarch64-static`). `observe` reads the
  launcher's record; a crash, a refused launch or a block left live is a tool
  error, never an observation.
- `Observation::of_reference`: the interpreter's observation, the oracle.

`differential` runs one program under a list of `Configuration`s and reports
`Agree`, `Mismatch` or `Unavailable` for each. `smoke_matrix`, `full_matrix`
(both targets, regions on and off, insert-only/elide/elide+reuse, heap executor,
unoptimized and optimized executables, production mode) and `topology_matrix`
(22 partitions) name the configurations; `MATRIX_VERSION` versions the names.
Matrix version 2 names flat walking and adds an optimized whole-program no-flat-walk ablation on both targets.
`real_io_replay` runs an executable on real Io with its completion tape
recorded, then on simulated Io consuming exactly that tape.
`Toolchain::under_memcheck` launches x86-64 executables under Valgrind.

Effects (resource, finalizer and capability events) are not part of the common
observation: the heap executor and the launcher do not report them yet.

`run_path_native` is `hirsel run --native` without a record, under the empty-grant record. It builds the runtime archive from
this checkout with cargo into `HIRSEL_NATIVE_CACHE` (default
`$TMPDIR/hirsel-native-runtime-<effective uid>`); the cache directory must be
owned by the current user with mode 0700. Every archive reuse verifies a private
SHA-256 receipt for its input key; corrupt, foreign and unrecorded archives are
rebuilt. Publication uses the per-key lock and atomic archive/receipt renames.
There is no installed runtime yet.
Archives in that directory are keyed by runtime and dependency sources, the
header, locked dependencies, build flags, target and toolchain versions. Builders
hold an OS lock per key and publish a complete archive by atomic rename, so
concurrent checkouts share identical inputs without replacing another runtime.

Native runtime failure codes include the `failure` rule of each called manifest
row, alongside instruction failure-edge rules. The heap adapter models the four
text operations as byte values and input-relative ranges, leaving allocation and
owner-credit changes to H.

PROG-1 package loading is in `package.rs`. `load_program` indexes the selected
JSON graph before checks, retaining all source FileContexts and a declared entry.
`LoadedProgram.check` checks every selected body once per dependency package revision,
with per-package dependency heads composed by `CompilerState.index`. Checking
includes compile-time evaluation for every selected body; lowering and V do not.
The entry's own package lowers every executable body. A dependency lowers at check
time only the bodies whose lowering evaluates at compile time (constants, `comptime`
expressions and source-constant reads), because only that evaluation reports source
diagnostics from lowering; every other dependency body lowers when the program
package reaches it. Reachability is `semantic_program`'s read closure, so the check
lowers exactly what a launch's semantic program revision can encode
(`queries/lowering.rs`, `tests/it/lazy_lowering.rs`). `.hir.json` is an exact sidecar; there
is no ancestor search, std fallback or source discovery.

The heap adapter models byte/position/path bridges with exact input-relative ranges. Wave-1 ordinary std algorithms remain Source-origin units; the source-origin and resource failure integration controls live in tests/it/stdlib_wave1.rs.

Revision 6 publishes nominal borrowed-loop associations and scoped callable state
promises. Core Unicode views use their nominal declarations rather than collection
selector labels. Implementations requiring preservation or creation-envelope proofs
currently reject explicitly as not yet provable.

## Memos and invalidation

Every per-definition query result (signature, resolution, check, modes,
lowering) is a memo (`queries/memos.rs`, `memo/`): its value, the adapter's
reported detail, and typed `Reads`, a sorted set of `(ReadKey, observed value)`
over signatures, failed signatures, lookups (provider, manifest index and
module heads, negative ones included), private representations, executed
bodies and signatures, cited text, the definition's own declaration candidates
and the environment (design §1b). A memo's own content is its text: the item's
text hash (headers' hashes for a signature) with its package's closure
manifest.

The `MemoGraph` keeps each memo's record and the reverse index `ReadKey →
subscribers`. Indexing a file diffs each affected definition's inputs (body,
headers, candidates, anchors, name kind and visibility) and marks exactly the
subscribers of every read that can have changed, plus the memos keyed on a
definition whose text or anchors changed; a name appearing, disappearing or
changing kind marks every lookup. Nothing else is touched. A memo with no mark
is valid as it stands and stamped when read; a memo marked by a read is
deep-verified by observing each read again, and recomputed only if one differs;
a memo whose own text changed is recomputed. Counters record each mark by read
kind (`--timings`, `invalidated by <read>`), and `memo_stats` the memos touched.

Executing or materializing anything merges its complete reads into every open
evaluation frame, on hits too: a lowered body's memo reads, a constant's or
staged value's execution reads, a generator's check. A constant whose recorded
execution closure still observes the same values is reused.

Compile-time dependency depth is logical height (`memo/depth.rs`): each
constant, staged value and generator result keeps `DepthEvidence`, its height
and the overflow table `first[m]`, composed from its requests in request order.
A request at stack length n fails at its own site when n ≥ L =
`min(call_depth, 32)`, else at `first[n + 1]` if any, whether the result was
computed or reused, so the verdict and site equal a cache-free serial
evaluation. Limit failures are operational and never cached.

Dependency packages are checked in each engine unless a precheck covers them;
cross-program reuse of checked results goes through the result store
(`store.rs`: `none@1`, `mem@1`, `dir@1`, `sim@1`).

Runtime archives have independent `RuntimeBuild` choices: `system_allocator`,
`measurement` and `diagnostic`. `Toolchain::in_tree` selects the production
archive; `in_tree_with_runtime` makes these choices explicit and includes them
in the immutable archive key. `in_tree_diagnostic` is the correctness harness
choice. `link` sets `EmitInputs.measurement` from the archive selection so
compiled drop edges and runtime count paths are accounted exactly once.
Measured `launch` writes `dir/measurement`; direct measured executables take
`HIRSEL_RT_MEASUREMENT` as their output file. This measurement-tool output is
outside semantic RunConfig and Io tape inputs.

Production launches report `audit unavailable` without a live-storage scan.
Diagnostic and measurement archives report the existing exact exit audit;
`observe` refuses unavailable or nonzero audits for these archives. Poisoning
is still selected separately at launch, so native correctness matrices exercise
both poisoning modes while retaining the leak oracle.

The heap Io model gives each published file an allocation token independent of
its bytes and handle number. Opened grants retain that token; replacement creates
a new token, unlink removes its reachable name, and atomic commit moves the
temporary token. Narrow compares tokens after checking the target tree/root.
Forking the model retains token identity without exposing host addresses.

Native `Options.launch` carries pure `native::LaunchInputs` through ReadyProgram.
`build` binds every launchable entry to the checked frozen semantic revision;
direct `build_program` clients must call `bind_revisions` before linking. The
linker validates an explicit conformance fixture record and derives seed/identity
ABI data only from that record. Allocation receives only hash_seed. The Ready
cache includes every launch input. Nonzero fields extend configuration names with
canonical hash/random/test suffixes; seed-zero names remain unchanged.

`entry_roots` checks the exact public monomorphic core Result/root entry contract.
`admit_run` checks the semantic revision, entry, launch kind and all required
roots before invoking the host authority. No supplied record selects explicit
denial defaults. `read_run_record` uses the shared absolute-path record reader.

`native::profile_sources(snapshot)` retains definition-owned file text and paths.
`native::link_profile(..., Some(&sources))` opts into DWARF line tables; a measurement
archive additionally selects source-site call scopes. `link` and `emit` retain ordinary
production emission. Profiling is artifact tooling, outside semantic run identity.
The heap Io adapter independently selects configured virtual mounts and retains
read/read_write rights on every directory grant. Narrow preserves those rights;
write/remove and every atomic mutation return FileError::NotGranted before effects
when the selected grant is read-only. File generations and fake Test trees keep
their existing independent identities.

Native import infers body result-arena summaries after callable specialization.
The driver checks their complete call-graph binding, then every unit's independent
C return proof, before publishing a program. Placement may omit an arena payload's
credit walk only under C's same-token child proof.


`tests/it/stdlib_net.rs` and `stdlib_uri.rs` port Rust core::net parsing/display
and Zig HostName/Uri vectors through the ordinary std package graph. IP/socket
parsing and host validation are source noalloc functions. URI components own
Raw/Encoded Strings, and decoding retains binary Bytes. The two SEM-3-net-*
conformance programs cover these public functions in the filtered native matrix.
Every vector runs in ordinary and checked execution, including Encoded component
copying, parsed formatting, resolution and the independent Unique regression.

The heap Io adapter marshals generic aggregate operands to the runtime's
cloneable `io_net_script::ScriptIo` for network calls. This reuses native policy,
stream branding and ScriptNet transitions, while H alone allocates, counts and
drops program storage. Logical network handle identities are mapped separately
from H's other Io grants, and cloned campaigns retain independent peer progress.

The checker phases have separate crate owners: hirsel-signature normalizes headers
while hirsel-check checks bodies and source audits,
and hirsel-lower emits TIR and owns its printer. Driver adapters import each
owner directly; no hirsel-check facade remains for signatures or lowering.

Checker plans and private fact conventions come directly from shared
hirsel-check-protocol; the three implementations do not depend on one another.

Local binding reservations use ordinary fully qualified lookups in their declaring
module. Their negative reads and namespace inputs share normal cache validation;
a shadowed root fallback does not enter the semantic dependency closure.

Selected dependencies automatically expose their top-level source module heads alongside
explicit manifest aliases, scoped to the importing package. Core and std heads merge
through the resolver's ordinary declaration index. Parsing std rejects a definition
whose module and name duplicate core, including unused and private declarations;
versioned DefPaths and signature identities remain unchanged.

RuntimeTypes retains the checked nominal signature snapshots needed to specialize SER-2 schemas. These declarations come from the registry and source program, independently of incoming bytes.

Callable materialization reports CT-2 for named function references as well as
closures. Compile-time computations may invoke a contextually instantiated generic
reference and publish its ordinary data result; callable constants remain excluded.

`Compiler::load_test_package` indexes an explicit manifest without requiring a
launch entry. It retains the same package graph, source contexts and checking
policy; test discovery selects ordinary functions after checking the package.

`Compiler::load_package_declarations` loads the validated manifest graph for
signature tools, including libraries without an entry. It parses/indexes source
and resolves header paths through the selected dependency aliases and PROG-2
access rules, without checking, lowering or evaluating bodies or loading prechecks.

Size-dependent inline comptime expressions publish hidden size-generic constant
signatures. Generic TIR reads those constants with lexical size actuals; each
selected instance materializes through the existing evaluator and data boundary.
The cache retains its size actuals and executed implementation/signature reads,
and budget changes retire it. Program checking selects instances after complete
signatures and GEN-5; failures cite both the expression and its selecting call.
Native snapshots retain the selected immutable values for ordinary static reads.

Repeat-literal counts use the ordinary closed compile-time evaluator as u64.
Before lowering, O checks Array/Inline target kind and exact Inline length.
Counts depending on abstract size parameters report CT-2 at the count; this
form currently requires a definition-time-known size.
