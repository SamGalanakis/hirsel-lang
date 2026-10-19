# Native benchmark references

This suite measures actual native Hirsel source against the frozen phase-1
C and safe Rust references. Local x86-64 reports are informational: missing
configurations, counters, workloads and native AArch64 timings are reported as
incomplete. Benchmarks are not wired into either repository gate or a scheduled
workflow; the harness's own tests (`bench/native/tests`) run in `tools/gate`
whenever a file here outside `programs/` changes. Reports are written to the gitignored `bench/native/out/` by default.

The nested Rust project has its own `[workspace]` and lockfile; the parent
workspace selects only `crates/*`, so none of these dependencies enters it.
There is no compiler implementation dependency and no compiler tuning here.

From the repository root:

```sh
bench/native/run --verify --output /tmp/reference-correctness.json
python3 -m unittest discover -s bench/native/tests -v
bench/native/run --cpu 24 --case binarytrees-small --output /tmp/reference-samples.json
bench/native/analyse /tmp/reference-samples.json --output /tmp/analysis.json --regions /tmp/regions.md
```

The analyser exits 2 for `incomplete`, `inconclusive` or `fail`, and 0 only for a
complete native pass. With no samples it writes the complete regions template
and lists every missing target. A local/subset/reference-only run is deliberately
incomplete. Never label emulator timing as AArch64 performance. Run the same
suite on a dedicated native AArch64 host for that target's primary evidence.

`analyse --embedded-manifest` reproduces an archived dataset against its own
frozen contract after sources evolve; snapshots cannot be pooled.

`--reuse-build build/build.json` verifies the sealed source and binary hashes
before using existing builds. `--build-only` writes that inventory. `--freeze`
seals source hashes after an intentional code change; `generate` regenerates all
fixed fixtures and independent Python byte oracles. Freeze before evaluation,
retain old raw reports with their embedded manifest, and use a new dataset after
any change. Checks reject silently edited inputs, toolchains or references.

## Workload contracts

Both implementations use equivalent node/ownership algorithms; safe Rust uses standard Vec/Box ownership and Rc for shared heap lists.
Tree and AST nodes are individually allocated. Trees use conventional two-child nodes (16 bytes on both targets), with Box ownership in safe Rust and plain references in bumpalo. A reusable depth-bounded worklist releases heap trees without recursion or allocation during release. For AST nodes the index table is a teardown worklist,
not a contiguous replacement for tree allocations. Heap list ownership uses Rc::get_mut for in-place reuse and iterative Rc::try_unwrap cleanup. Heap nodes and owned byte
buffers are released completely before the routine returns; arena storage resets
at the corresponding epoch. No recursive destructor uses the native stack.
All sizes and seeds are in `manifest.json`; 17 cases × three sizes = 51 inputs.

| Program / subcases | Algorithm and oracle | Natural lifetime |
|---|---|---|
| binarytrees | Stretch tree, retained long tree, depths 4..d in steps of 2 and 2^(d-depth) temporary trees; checked node counts | Each temporary tree; separate retained long-tree arena |
| expr-compiler | Copy/lex a fully parenthesized corpus, allocate integer/x/add/multiply AST nodes, recursively evaluate at x=3 with checked u64 operations | Corpus invocation |
| json-roundtrip | Lossless JSON AST with scalar/string spelling owned per node, nested linked array/object children and owned object keys; validate escapes and numeric tokens and structurally emit identical bytes | Document invocation |
| wordfreq | Copy UTF-8 tokens, seeded FNV hash buckets with separate chaining, owned keys and insertion-ordered entries; exact word/tab/count lines | Text invocation |
| records-messages/scalar | Inline four-u64 records, 16 scalar update passes, checked sum | Record batch |
| records-messages/flat | Pointer-free LE id/kind/length/payload messages, sequential walking, copy/update every sixteenth message; exact checksum | Message batch |
| batch-validation | Parse the fixed code/amount lines into short-lived event nodes, check code<25 and amount<=900, produce success/error results and checked totals | Parsed batch |
| persistent-list-map/unique | Individually allocated list, map (+1) and reverse by rewiring unique nodes, update new head (+7), iterative release; exact ordered values | List invocation |
| persistent-list-map/shared | Retain original root; build detached mapped/reversed nodes and release shares independently; emit both complete ordered lists | Independently owned roots; arena explicitly invalid |
| saxpy-checked/f32, f64 | Allocate x/y, y=1.5*x+y in source order, no contraction, hash exact IEEE bits | Array invocation |
| saxpy-checked/integer | Checked u32 multiply then add, stop at the first overflowing index; checksum records prefix and failure index | Array invocation |
| slots-churn | Narrow 8-bit generation model, two retained anchors, stable index traversal, stale lookup after remove, retire generation 255 before wrap | Slot container invocation |
| closure-iter/scoped, erased | Add captured 3, filter even, multiply captured 2; scoped specialization versus allocated erased stage table and intentionally indirect dispatch | Callback pipeline invocation |
| resource-failure/throughput, heavy | Deterministic simulated input/transaction/destination bytes, checked writes, commit or abort at fixed fractions 1/17 and 1/2, exactly-once reverse ordered cleanup | Resource operation; all resources cleaned before reset |

The JSON fixture is canonical JSON and includes every short escape, escaped
Unicode/surrogate pairs, UTF-8 source content, booleans, null, signed integers,
fractions and nested arrays/objects. This is a **lossless roundtrip parser**:
it preserves escape spelling rather than decoding and re-encoding strings.
All references use that same algorithm. It is not a full JSON-library benchmark.
Inputs outside the registered grammar are not evaluation cases.

The slots model is implemented in every reference. Its reduced generation width
makes retirement observable in the medium and large fixtures; it does not claim
native Hirsel Slots support. That final obligation stays open until native
executables implement the same operation and traversal contract.

The workloads take their algorithms, not unsafe lowering code, from an early
Rust prototype: the tree depth schedule and expression recurrence, the
code/amount formulas, and the payload and LE flat-message formulas. Sizes are parameterized, arithmetic checks are
explicit, JSON/UTF-8 coverage is expanded, and independently generated byte
oracles replace the prototype's checksums. Step-0 system-only timings are not
reused as evidence.

## Binarytrees gate

Both binarytrees rows gate at **1.25×**; each input must meet the frozen
one-sided upper 95% bound, with the fastest baseline reselected in each paired
bootstrap resample:

- **Arena:** Hirsel `binarytrees-region` (MEM-10 caller arena) versus the
  fastest valid C program, including C/arena, C/mimalloc and C/system.
- **Heap:** the frozen Hirsel `binarytrees` versus the fastest C malloc
  program, C/mimalloc or C/system. C/arena does not enter this denominator.

These are separate obligations; passing one does not waive the other. Rust
columns remain useful context. The analyser applies the C denominators sealed in
`manifest.json`'s `binarytrees_gate`. Native inventory records carry
`binarytrees_storage: heap|arena`, independently of configuration/partition slot
names; an ordinary `binarytrees` build defaults to heap. The builder labels both
source forms explicitly. A report remapping an arena executable into a frozen
slot must preserve that metadata. Archived manifests retain their own contract.

The C arena reference adapts the recursive root-first construction and
left-child checksum of Benchmarks Game [C gcc #3](https://benchmarksgame-team.pages.debian.net/benchmarksgame/program/binarytrees-gcc-3.html),
with the retained temporary pool across depths of [C gcc #2](https://benchmarksgame-team.pages.debian.net/benchmarksgame/program/binarytrees-gcc-2.html).
APR headers and `apr-1` pkg-config metadata are absent on the measurement host;
`c/binarytrees-arena.h` therefore uses a minimal linked-chunk bump pool. Each
64 KiB payload chunk holds individually requested 16-byte nodes. Clear resets
the cursor and retains chunks; complete pool teardown is inside each epoch.
No exact tree-size reservation, contiguous tree indexing or checksum formula
replaces node construction/traversal. Allocation counters count node requests,
not backing malloc calls; reserved high water includes chunk headers and both
simultaneously live pools. `arena_reserved_bytes_sum` sums the temporary pool's
reserved bytes at each clear. Timing builds compile out all counters.

Departures from the upstream programs follow the already frozen workload:
no OpenMP; create the retained tree before the stretch tree in separate pools;
reuse the stretch pool for temporary trees; use `2^(depth-d)` temporary trees
rather than the Game's factor-16 larger count; accept the registered file input;
emit the existing total node count and harness digest rather than Game lines.
Checked u64 addition, baseline Clang flags, repeats, input loading, output and
complete cleanup use the same harness as the malloc references. The upstream
BSD notice is retained in `third-party/NOTICES.md`.

## Allocators and build pins

Clang 18.1.3 (`-O3 -ffp-contract=off -fno-lto`, baseline CPU) and Rust 1.99.0
(optimized, overflow checks on, LTO off, baseline CPU) are pinned. Safe Rust
uses `System` or `mimalloc::MiMalloc` as the global allocator. C uses malloc/free
with system malloc or linked mimalloc 2.1.2. Rust pins mimalloc 0.1.39 and
libmimalloc-sys 0.1.35 (bundled mimalloc 2.1.2), and bumpalo 3.19.0 in its own
lockfile. Every transitive crate is locked; secure/debug features are off. Ambient allocator options and Cargo profile/Rust flag overrides are removed before building or running; the declared baseline flags are installed explicitly.

C has an arena variant for binarytrees. Rust has arena variants for: binarytrees, expr-compiler, json-roundtrip,
wordfreq, records-messages/flat, batch-validation and persistent-list-map/unique.
All other subcases are excluded explicitly in the manifest: shared lists have
independent lifetimes; the others primarily exercise computation or traversal.
There are no Zig or jemalloc references. Installed jemalloc is unused.

The installed mimalloc package version is pinned in the manifest. If that exact
package is unavailable, `allocators.py` downloads its digest-pinned source
archive and builds under `build/allocators/`; it never installs system packages.

Primary build policy is one workload implementation translation unit per
language, no LTO or PGO and no host-specific CPU tuning. Compile invocation times
and `.text` bytes are reported separately for timing and instrumented binaries;
compile times explicitly say whether caches were retained. They are not compiler
phase, cold build or incremental feedback claims. External libraries' text is
not folded into the executable's `.text` size.

## Measurement and analysis

The runner first checks **all** 51 inputs against their exact emitted bytes in
all valid language/allocator variants, for both timing and counter builds. That
is 912 checks: 456 byte/counter checks and 456 three-epoch checksum checks. Timing uses one shared repeat count, calibrated against the
fastest participant to at least 500ms of CPU work; scheduler stalls cannot
under-size the cell. The validity floor remains 250ms elapsed. Five complete warmup rounds
precede thirty paired rounds. Each round independently shuffles participants
with the fixed manifest seed; every sample is a fresh pinned process. Input file
loading, construction, output digest and full cleanup are included for every
language; no language gets an excluded fixture-preparation phase.

Raw output, exit code, time, repeats, order, UTC time, CPU affinity, load, governor
and observed frequency are retained, including failures and short/noisy samples.
There are **no automated exclusions**. A rerun is a new dataset; duplicate target
datasets cannot be pooled or cherry-picked by the analyser. Instrumented runs
occur after all timings, use different binaries and one logical epoch, and must
have identical checksum output. Peak RSS is from GNU time and is kept separate
from wall times, text size, compile costs and counters.

Counters have explicit meanings: `allocation_requests`/`requested_bytes` count
workload-owned node and buffer requests (including bump requests);
`copied_bytes` counts explicit token/message/transaction copies;
`count_operations` counts persistent-list retain/release operations;
`reuse_hits`/`reuse_misses` count in-place versus detached mapped nodes;
`arena_peak_reserved_bytes` records the arena capacity high-water mark.
These are not an interception of internal libc/library allocations or allocator
size-class reuse. `arena_reserved_bytes_sum` is a diagnostic sum at reset points,
not a peak. Counter telemetry is compiled out of timing executables.

For each case/target/configuration, resample the same thirty paired indices for
Hirsel and every valid reference, 10,000 times at the fixed seed. Recompute the
median of each column and the fastest baseline in **every** resample. The 95th
percentile is the one-sided upper bound. Report ratios/upper bounds against each
variant and each language as well as the overall fastest baseline. A complete
pass requires every upper bound in both regions-on whole/singleton modes to be
<=1.25 on both targets. Target/variant geometric means are descriptive only.
A bound more than 10% above its point estimate is preregistered as inconclusive;
no noisy sample is deleted. Missing target, variant, counter run, correctness
row, unsupported operation, primary partition or explicit skip is incomplete.

## Native handoff and dedicated machines

`build-hirsel --case records-messages/scalar` compiles the ordinary source in
`programs/` with `common.hir`, checks/export/imports the frozen native snapshot,
then emits LLVM O3 objects and links the in-tree production runtime. The helper
has its own workspace and lockfile. It reports source-to-executable compile time,
runtime build time, partition, pass switches, source/binary hashes and text size.
No interpreter, compilation, Python launcher or diagnostic audit enters a timed
native process. Both primary partitions and regions-off size class run the same
source. The default compiler uses O3; C/Rust references retain their pinned O3.

`--hirsel build/hirsel.json` accepts per-configuration `cases` maps, each with
`command` argv, source/binary SHA-256, `compile_seconds` and `text_bytes`.
Separately instrumented executables additionally supply `counter_command` and
`counter_binary_sha256`. Legacy single-executable entries still use the same
contract. The runner verifies bytes and three-epoch checksums for every available
native case, records absent telemetry, and samples only built configurations.
The analyser reports all missing obligations and cannot pass partial evidence.
Native paths resolve under `bench/native`; the timing process is launched directly
with `CASE INPUT ROUNDS [--emit]`. Runtime diagnostic/tape/replay environment
options are cleared. Missing counter cells remain pending rather than zero.

`--machine machine.json` supplies preregistered `dedicated: true`, `target`,
`cpu`, `governor`, `cpu_feature_baseline`, documented `os` and `microcode`. The
runner also records the actual host data. Absence or a changed governor/CPU makes
a gate claim incomplete. Local reports can validate the harness without that
declaration. Native performance evaluation needs dedicated x86-64 and AArch64
machines. The complete generated regions template compares
each configuration to every valid reference variant for binarytrees, batch
validation, expression compiler and JSON at all sizes and both targets.

`check-program --case CASE` derives a small-input source control and runs it
through `native --full --filter` under a temporary conformance root. It includes
all 44 named configurations on both executed targets. The source port and
common functions are unchanged; the control entry supplies fixed bytes directly.
The sealed input suite separately checks actual command-line/file execution.
Native compilation records additionally retain the compiler revision, helper
executable digest and linked runtime archive digest. Archived datasets keep their
own frozen source contract.

Resource-failure timings are the identical explicit **source model** from the
phase-1 references. Real resource drops are a separate, dependency-owned row B;
its correctness control and labelled draft references are preserved under
`programs/controls/` and `resource-drops/`. Neither an empty finalizer nor stdout
logging is reported as effectful destruction timing.

`build-hirsel` now builds all four allocator/partition configurations and an
independent measurement executable for each. `--configuration` restricts builds
for diagnosis. Native counter launches set `HIRSEL_RT_MEASUREMENT` to a fresh
admitted file; production timing launches clear it. Runtime class allocations,
increments/decrements, copies, reuse, live payload and region high-water bytes
retain their own names and units, separately from reference workload counters.
The separate `arena_high_water_bytes` gauge remains `unavailable`; actual
`region_high_water_bytes` counters describe native arena storage. Ordinary
numeric formatting selects the std@1 source package explicitly; its dependency
source hashes accompany each
native build record. Historical core-formatting datasets remain sealed separately.
