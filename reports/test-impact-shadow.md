# Shadow test impact

`hirsel_driver::testing::impact` is an opt-in library diagnostic over a fixed pair
of explicit graph snapshots and their selected tests. Ordinary `hirsel test`
discovery, output, launches, V and admission are unchanged. There is no result
store, automatic omission, cached execution, native-code equality promise or
new semantic identity contract.

`QueryEngine::semantic_program_nodes(roots)` is a read-only batched export of the
existing semantic program product. It includes the runtime node and preserves
versioned package and nominal names. It does **not** certify closure completeness.
The tool compares `ProgramNode` content and sorted edges directly and binds both
full graph products through the existing `ProgramRevision::encode`. Its private
batch root is diagnostic, never a runner/program revision. Snapshot IDs and the
full explicit old/new selection and input records are returned in the report.

The consumer supplies per-node evidence for calls, function-value alternatives,
bodies/constants, private representations, implicit finalizers, selected generic
and generated instances, generator inputs, and core/runtime selection. Evidence
must be bound to the snapshot and establish each class, including absence. The
bounded fixture producer is the only producer established by this change; there
is deliberately no production `all classes complete` adapter. Compiler signature
reads and empty capability sets cannot establish execution completeness. In
particular, ongoing engine O2 completeness work is not a finished guarantee.

Inputs explicitly bind arguments, fixture/provider, seed/scheduler,
fuel/stack/budgets, runtime/test protocol, selection configuration, and
compiler/backend provenance (a canonical non-applicable value is allowed for
ordinary language tests). The producer supplies canonical bytes from its own
validated input encoding, reports input decode/validation errors, and separately
states its deterministic environment contract. This in-memory API is not a new
persistent format or untrusted-input verifier. Live filesystem/network/time,
FFI and unmodeled environments must supply an error. A recording grants no
current host authority and performs no current host effects.

Each side builds dense reverse edges and SCC membership with `hirsel_graph`'s
existing iterative Tarjan helper, then two ordered multi-source reverse BFS
passes: changed nodes and incomplete nodes. Changes include both old and new
edges, removed nodes, additions, content and targets. No test-by-definition
matrix or per-root closure hash is built. Missing and conflicting graph nodes
refuse complete answers. A test is unaffected only when neither complete
reachable graph has changed and its declared inputs are equal. Unknown takes
precedence over affected. Affected reports retain a nearest changed node/path
on each side; these are bounded witnesses, not a list of every transitive change.
BFS ties use seed identity then predecessor identity, both canonical order.
All incomplete node reasons are visible in `unknown_dependencies`, with a nearest
reachable reason attached to each unknown test.

`validate` invokes both sides for every union-selected test, unconditionally,
including affected, unknown, added and deleted entries. Missing entries return
an oracle error, rather than a saved observation. Original predictions remain
in each fresh `OraclePair`; a runtime error or an unequal fresh observation
under an unaffected prediction makes the returned verdict unknown. Equal
observations never upgrade dependency predictions. Callers assert dependency
classification independently of oracle agreement.

## Prior art and deviations

* Ekstazi `b6ee1f2bed42c6bb343e74759078dbd6458f4f5a`,
  `org.ekstazi.core/src/main/java/org/ekstazi/data/DependencyAnalyzer.java:270–315`:
  separate dependencies from results; absent/empty data reruns. Unlike Ekstazi's
  last-run dynamic resources, this tool requires explicit conservative static
  closure completeness; dynamic coverage cannot bound Hirsel execution.
* Unison `cea754e162146df4e6fb355cf20c614809580d82`,
  `codebase2/codebase-sqlite/sql/create.sql:121–143`: supporting term/component
  identity precedent only. No watch-result storage is ported.
* `crates/hirsel-graph/src/lib.rs:164` supplies SCCs;
  `crates/hirsel-run-schema/src/program.rs:41` supplies the existing canonical
  graph encoder. Ordered BFS uses Rust std `VecDeque`; no new hash, Tarjan
  implementation or engine-wide incremental aggregate is introduced.

## Validation and cost evidence

See the companion lane report for measured instruction counts, exact executed
case counts and landing SHA. The integration module `test_impact` includes a
6,144-edit exhaustive three-node graph matrix (512 graphs × 12 edits), explicit
missing-class and input-field tests, insertion/edge shuffles, ID salts, parallel
callers, and source fixtures for disjoint calls, owning-type finalizers, private
representations, generated bodies/inputs and function values. Fixtures are
constructed in isolated source maps; no unrelated source files are edited.
Fresh interpreter and diagnostic Cranelift observations are compared through the
existing native differential harness, with normal validation still enabled.
Native observations cover exit/output/failure; the existing native harness does
not expose ordered finalizer-effect observations, so direct owner-edge and
content-change assertions remain necessary.

`IndexStats.payload_bytes` counts retained dense reverse-edge, SCC-membership
and BFS successor array payload only. `Snapshot::payload_bytes` separately
counts logical graph/input byte and evidence payload. Duplicated map indexing
keys are excluded, as are containers, allocator overhead and spare capacity;
neither metric is a total-memory or peak-RSS claim.
The index is rebuilt for each explicit pair: update cost equals reconstruction,
not a claimed engine incremental update. The ignored `shadow_measurement` test
provides index, interpreter, setup and native modes for capped instruction probes.

## Recommendation

Keep shadow use opt-in. The bounded pure fixtures establish useful precision,
but the production completeness producer and a real test duration/impact census
are not established here. Automatic selection needs a separate design after
those exist, including explicit stale/failure handling and oracle auditing.
No favorable fixture benchmark enables skipping or reuse. The historical
57.917210 G tasks request median is motivation from the old 83193f1e bundle,
not the current residual, an individual-test cost or a suite-speedup baseline.

## Measured bounded fixture

Checked-profile instruction probe, ten rounds in seeded (`20261021`) interleaved
order, `perf stat -e cycles,instructions`, capped at 4 GiB and two CPUs. Each
invocation constructs two fresh checked source engines/graph products and one
report. Setup then does 100 black-box references; index instead does 100 fresh
old/new comparisons; interpreter instead executes both pure tests 100 times.
The native mode additionally builds ten source variants and compares both test
entries freshly against diagnostic Cranelift (20 native launches per invocation).
These are different native and pure batches, not a per-test cost distribution.

| Mode | Median total instructions | Median paired instructions minus setup |
|---|---:|---:|
| Setup/harness/two fresh graph products | 435,377,813 | — |
| 100 paired index constructions/reports | 479,701,062 | 44,333,987 |
| 100 warm interpreted two-test batches | 1,124,072,893 | 688,634,798 |
| Fresh native fixture matrix, 20 launches | 5,429,162,815 | 4,993,818,682 |

Thus comparison upkeep from already supplied graph products is about 443,340
instructions per pair, versus 6,886,348 for the warm interpreted pair. Both
indices are reconstructed; there is no incremental-update speed claim. The
435.4 M setup/harness instructions include source checking and graph production;
that gross cost alone exceeds rerunning this tiny interpreted pair. Building
graphs solely for this use loses at this scale. Existing resident products could
change the accounting, but require measured integration and completeness evidence.

Each real pure graph has 6 nodes, 13 edges and 6 SCCs. The two sides each report
344 bytes of retained dense index payload and 8,641 bytes of logical graph,
evidence and input payload, with the exclusions stated above. Observed scoped
peak was 21 MiB for the pure modes and 77 MiB for the first native matrix run,
under the unchanged 4 GiB cap. These peaks include the harness, and are not index
allocation measurements.

For the disjoint edit the affected fraction is 1/2, unaffected 1/2, unknown 0/2.
The independent three-node body-edit subset gives 384 useful unaffected answers
out of 1,536 edits; all 6,144 content/edge edits agree with direct closure equality.
Each deliberately incomplete-class fixture returns one unknown with its named
class and leaves the disjoint complete test unaffected. Without production
class evidence, all selected production closures are unknown: **inadequate
precision for an automatic selector**, not a useful completed production adapter.

Observed elapsed medians per complete probe invocation were 0.216 s setup,
0.216 s index, 0.267 s interpreter and 1.003 s native. The seeded session's median
paired **gross** index/interpreter elapsed ratio was 0.812. These include setup,
process launch and 100 operations; subtraction is too noisy for a net timing
claim, and the native batch has different work. No suite speedup follows.
