# hirsel-conformance

Validate rule-linked corpus headers, classify compiler results and enforce the additive passing ratchet.

Allowed dependencies: hirsel-cli-args, hirsel-driver, hirsel-diag, hirsel-span, hirsel-syntax, hirsel-run-config, serde_json; no phase internals.

Invariants and boundary:

- Consumes conformance files, spec rule IDs and the narrow Driver adapter; produces counts/statuses and optional JSON.
- Every language diagnostic has a rule; rejection matches the first diagnostic and exact primary line resolved from its source snapshot.
- NotImplemented is distinct from rejection and never added to passing.txt.
- Header validation applies even while the compiler is unimplemented.
- Updating the ratchet only adds passing cases, never removes regressions; corpus files are not rewritten.

The binaries use `hirsel-cli-args` and pinned lexopt for argument parsing.
Common `--root`, `--hash-seed` and `--shard` values share validation and
argument errors print to stderr and exit 2. Paths retain OS bytes.

The runner prints `whole corpus` and `in-gate corpus` counts. JSON retains
`summary` for the whole corpus and adds `in_gate`, plus each test's `in_gate`,
`exclusions`, `actual`, `phase` and `unchecked`. Verbose output lists exclusions
and the legacy unchecked count, always zero. JSON also retains `unchecked` as
zero (null for tests without an observation); this compatibility field has no
internal counter or incomplete validation result. Full TIR
validation also runs in release conformance builds. Compiler invariant failures
are reported as tool errors, not language diagnostics or unavailable phases.

M2 exclusions come from the checked-in paths in `conformance/m2-runtime.txt`.
Open issue states are checked in at `src/open-spec-issues.txt`. Regenerate from
repository root after spec issue dispositions change (never during a test run):

```bash
gh issue list -R SamGalanakis/hirsel-lang --label spec --state open --limit 1000 --json number --jq '.[].number' | sort -n > crates/hirsel-conformance/src/open-spec-issues.txt
```

The runner intersects these numbers with each source's `// spec: #N` comments.
The passing ratchet includes every listed test regardless of gate membership.

## Front end and IR checker agreement

`agree` looks for programs the front end accepts but whose lowering the IR
checker rejects, and for internal errors and panics in any phase.

```bash
cargo run -q --release -p hirsel-conformance --bin agree -- --seed N --limit M
```

Every corpus program not expected to be rejected is parsed, and a fixed
catalogue of single edits is read off its syntax tree (`src/agree.rs`): passing
modes of parameters and arguments, `mut` on bindings, mode, brand and placement
annotations, a second use of a binding (also after `take`), statements moved
into or out of loops and branches, swapped or repeated statements, an operand
replaced by another binding of the same written type, an expression wrapped in
a called closure, stores into fields, elements and `mut` bindings, a local
or field returned, an assignment or call statement moved into a later
argument of a call in the neighbouring statement or tail (`f(a, { w; b })`;
a write through an index also after a copy of its list, so it detaches a
shared list), and an owned call argument received first by an
intermediate `let` whose relation names that binding or the innermost binding
in scope (#88), and a binding's value held as a tuple part of a list element
(`let agree_held: List[(T, u64)] = [(e, 0u64)]` then `let (p, _) = agree_held[0]`,
with written modifiers and relations kept on `p`, and literal and closure
initializers included), so what it holds is held through a sequence element;
when `p` has a written relation, a second variant writes it on the holder too,
so an arena-related list holds the element; and a block's tail naming a
binding of written type `T` held as `{ let agree_tail: List[T] = [v];
agree_tail[0] }`, so the block yields a part of storage it ends; the same
tail as the result of a closure call local to it (`(fn(agree_x: T) { agree_x })(v)`)
and of a closure that captures it (`{ let agree_f = fn() { v }; agree_f() }`),
so a share or a borrow of block storage leaves the block; and a typed binding's
value held as the tuple a called closure returns inside a list element
(`[(fn() { (e, 0u64) })()]`), so rows local to that closure reach a Global
element at depth; and a typed binding's initializer as the whole result of a
called closure (`let p: T = (fn() { e })()`), so the written type receives, at
every depth, a row no expected type shaped.
A block whose tail is a call is given the same call once more as a statement
before it, so the frame destroys a discarded temporary of that call's type.
`annotation/uses` writes `uses {}` on a function type nested in a written
type, a type argument included, so a narrower callable meets an exact edge.
Each edit replaces one node's source span.

`--seed` ranks the variants and `--limit` keeps the lowest-ranked M (default:
all). The CLI first applies `--variants-per-program N` (default 96) separately
to each program. Its catalogue is ordered by a fixed pseudorandom rank and the
seed chooses consecutive windows of N edits, wrapping at the end. Every edit
is reached within `ceil(catalogue_size / N)` consecutive seeds. This budget is
independent of the later global `--limit` and shard partition. `--exhaustive`
disables the per-program budget for an explicit exhaustive campaign.
The JSON `generated` is the complete catalogue; `eligible` is its size after
budgeting and before the global limit. Every shard reports both global counts.
A variant's rank depends only on the
seed, its file path and its edit, so the same arguments select the same
variants and adding a corpus file leaves the rest of a sample in place.
`--shard I/N` partitions that globally selected sample by stable variant rank,
after applying the limit. Catalogue counts remain those of the unsharded run;
outcome and edit counts sum across shards. `python3 tools/buck2/agree.py` runs the gate's
16 shards and rejects missing or inconsistent reports.
`--path PREFIX` (repeatable) keeps only the corpus files whose root-relative
path starts with a prefix, so `--path conformance/CLO/X.hir` checks exactly
the variants of that file the whole campaign would.
`--hash-seed N` independently selects runtime placement in conformance, native
and agree, including checked execution, exact-program replay and reductions.
Native artifact names retain nonzero launch inputs as canonical suffixes; every
existing seed-zero matrix name is unchanged.
Variants of programs the compiler does not accept today are skipped. Each
remaining variant goes through the driver in a fresh engine; variants of `run`
tests are also interpreted under a step budget, once normally and once under
checked execution. A checked run that traps is
flagged as `checked/<rule>`, one that observes anything else differently as
`checked/divergence`.

The JSON report counts variants that were rejected with a language diagnostic,
accepted and valid (with how many of those ran in both execution modes), or
unavailable (a phase reported not implemented), and
lists the flagged ones clustered by first violation kind and edit kind. Each
cluster carries its count and one representative, reduced by deleting
definitions and statements while the classification holds. The exit status is
1 when any cluster exists.

The crate's tests sample the live corpus with seed 1 and a limit of 64, and
fail on any cluster. The full gate checks the budgeted campaign in release mode, sharded (`python3 tools/buck2/agree.py --full`).

Each run prints to stderr, per stage (`originals`, the acceptance check of each
sampled program, then `variants`), its wall time, its five slowest items and
the five programs with the most time in all. A classification still running
after 30 s is named while it runs, and again each time its running time
doubles, so a variant that never terminates names itself before a timeout.

`fail RULE at line N` may include `// stdout:` directives. When present, they
assert exact output before failure, including shielded finalizer output during
unwinding; absent directives leave output unchecked. `// exit:` remains run-only.

Runtime conformance consumes `run_observed_snapshot`: exact output/exit and the
primary failure rule/site come from semantic observation v1. Complete causes,
effects and final simulation state remain available at that driver seam.

Every accepted test that executes is executed again with `run_observed_checked`.
The checked run must reproduce the outcome, observation and output exactly; a
trap or any difference is a report error, whatever the test's own status, so it
fails the gate without touching `passing.txt`. A test that normal execution
cannot run (an M2 `arena.with`) is compared only for traps.

Runtime gate exclusions have one authority: conformance/m2-runtime.txt (tab-separated
path and required behavior). Handwritten documentation never controls execution.

## Canonical corpus layout

`tools/gate` passes `--check-format` to the runner. Every discovered `.hir` file,
including unratcheted and runtime-excluded cases, must equal the output of the
same driver formatter used by `hirsel fmt`. Archived `reports/` directories are
not corpus inputs. An intentional syntax negative is exempt only when formatting
refuses it with the exact rejection rule and primary line in its header. IO,
operational errors, accepted programs and mismatched rejections never qualify.
JSON lists these dynamic `format_exemptions`; there is no path allowlist.

The `corpus-fmt` binary migrates files through the **release CLI**:

```bash
cargo build --release -p hirsel-cli -p hirsel-conformance --bins
cargo run --release -p hirsel-conformance --bin corpus-fmt -- \
  --hirsel /path/to/target/release/hirsel --evidence /tmp/corpus-format-evidence
```

It first runs the entire suite and saves `before.json`, formats copies with
`hirsel fmt`, compares ASTs with only source metadata erased, aligns significant
source tokens, then remaps rejection/failure headers to the same token. The
alignment accounts for generated grouping/separators and AST-identified trailing
closure `uses` clauses. Repeated tokens retain their ordered occurrence. Every
changed file is checked through the runner and must reach a formatter fixed
point. A changed verdict, rule, primary token, AST, comment byte or directive
restores the original file and records a reason. Only expectation line digits
may change in directives; header blank lines may normalize. Any other header/directive change is declined.

`after.json` is another whole-suite run. All verdict and (rule, primary token)
pairs must agree before the tool succeeds. `migration.json` lists changed and
declined paths, normalized-header count and each old line's mapped new line
(null for trivia-only/removed-token lines). Runner JSON exports `primary` with
rule, token spelling, aligned identity, raw token and physical line/column. An
opening grouping token projects to the first non-grouping token;
the raw token remains in the evidence and AST equality protects the projection.
The migration never updates `passing.txt` or excludes a semantic difference.

## Native differential gate (`src/native/`, `--bin native`)

`native-select --root REPO` reports the static campaign inventory without
checking or executing mutations. It lists every program's mutation count,
the baseline and generated program counts, and the exhaustive configurations.
The baseline contains passing `run` and `fail` cases; `accept` cases are never
executed by this campaign. Agreement executes only `run` mutations that remain
valid and within the interpreter budget. Its reported native count is therefore
an upper bound; an exact launch count requires running the campaign.

`native --smoke` (in `tools/gate`) runs the cases of
`native-tests/regressions/smoke.txt` and eight generated successes and eight
directed failures on the heap executor with fault injection and as an optimized
x86-64 executable on the diagnostic runtime archive and on the shipped (default)
archive, then records each on real Io and replays its tape; every one must
agree with the interpreter.

A native configuration names the runtime archive it links: `diagnostic`
(poisoning, header checks and the exit leak audit; its compiled fast paths stay
off, so every allocation, free and dup reaches a checked entry), `default` (the
archive `hirsel run --native` ships, with the inlined fast paths) or `system`
(the system-allocator archive). Every launch is killed after
`LAUNCH_TIMEOUT` (120 s, ten times that under memcheck) and that configuration
of that case fails by name.

`native --full` (in `tools/gate --full`) runs a derived cover of the executable
cases of `conformance/passing.txt` under a per-program pairwise cover of the
driver's legal matrix on both targets and
writes `native-matrix.tsv` (one row per case and configuration, headed by the
matrix and observation versions) and `case-walls.tsv` (each case's wall time,
every selected configuration included). Every disagreeing case and configuration must
be exactly one row of `native-tests/regressions/exclusions.tsv`, keyed by the
configuration's full name (see that directory's README);
`--bless` rewrites the file. Every mismatch leaves an artifact that is moved,
then replayed from a warmed engine, and each mismatching case is reduced while
keeping the same discrepancy. The cases of `native-tests/topology/cases.txt`
run under 22 link partitions, and 64 generated successes and 64 directed
failures run under their pairwise covers. `--jobs N` sets how many cases run at once
(default 4). Each run of `native` or `agree --native` keeps its scratch
directories, and its report unless `--out` names one, under its own run root
`native-gate/runs/<pid>-<time>` beside the executable, so concurrent runs in
one checkout never share a path; the root is held by a file lock, and the next
run removes every root whose owner has exited. Runtime archives built with
Cargo stay in the shared, locked `native-gate/runtime/cache`.
`native --sanitize` (in `tools/gate --full`) runs the smoke
cases, the allocating cases of `native-tests/regressions/memcheck.txt` and eight
generated programs as x86-64 executables under Valgrind memcheck, at O0 and O3:
on the system-allocator archive, where each fixed-size block is one libc
allocation, with regions and without, and on the memcheck archive, the shipped
size-class allocator whose client requests describe its slab blocks, region
reservations and arena blocks, with regions;
`--filter TEXT` runs the matching baseline cases instead. Under every mode
`--filter` may repeat; a case is selected when its ID contains any given text.
`native --full --shard I/N` runs the selected cases the conformance runner's
path-hash shard I of N holds (topology and generated cases included), and judges the exclusion rows of exactly those cases; the scheduled
full gate runs `NATIVE_FULL_SHARDS` of them, and `--bless` refuses a shard. `--replay DIR` and `--reduce DIR` act on one artifact.

`native --full --engine heap|llvm` retains only that engine's named configurations
(`--engine diagnostic|default|system`: native code on that runtime archive)
for profiling or focused verification. Its report judges and counts only the
selected engine; it never claims agreement for an engine it did not run.
`--bless` still requires the entire unfiltered baseline and both engines.

`agree --native heap|full` extends the agreement campaign: a valid variant the
interpreter runs is also run on the heap executor with fault injection (and as
optimized x86-64 executables under `full`), except under a configuration its
program has an exclusion row for. A mismatch, a heap violation and a stage
validator's refusal are discrepancies; their kinds must be exactly those of
`native-tests/regressions/agree-known.txt`. The default campaign in
`tools/gate` is unchanged and still requires zero flagged variants. Every
campaign prints each cluster's first variant to stderr as it finds it
(`agree: cluster {json}`), so a run stopped before its report still names the
kinds it saw. `--shard I/N` takes the same stable-rank stripe of the catalogue
under `--native` as without it; a shard fails on a new kind, and only the merge
of every shard judges a listed kind that no longer occurs. `tools/gate --full`
runs `tools/buck2/agree.py --native-full`: `AGREE_NATIVE_FULL_SHARDS` (16)
shards, each under `AGREE_NATIVE_FULL_TIMEOUT` (3600 s), merged from their
reports, or their streamed clusters for a shard that stopped.

An optional `// entry: name` header selects an ordinary public function instead
of `main` in the compiler, agreement and native runners. Test controls use a
`fn(t: Test) -> Result[(), E]` entry through the same matrix and observation
comparison as main; the native launcher grants Test alone with its recorded seed.

`agree --native full --program PATH` replays one exact source through the same
checking, checked execution and native comparison, retaining its package sidecar.
It applies no mutation, reduction, expectation or corpus exclusion. A discrepancy
exits 1; an unavailable run or one with no native agreement exits 2. The ordinary
campaign and its named-kind ratchet are unchanged.

A corpus source may carry an exact `.hir.json` sidecar assigning its package,
module, selected entry and ordinary source dependencies. Interpreter and checked
execution load that graph. The agreement campaign replaces only the primary
source text while retaining package metadata and dependency bodies; the native
matrix selects the same graph rather than compiling a source as standalone.
Dependency fixture sources live outside `conformance/` so each discovered corpus
source retains one expectation header.

The agreement catalogue mutates `sendable` on every writable type annotation and
nested part, including scalar, Bytes, generic, alias, tuple, Iter and callable
header positions. No callable-kind gate limits this edit.

The full gate adds a second placement seed (`--hash-seed 1`) over the interpreter
corpus and `native --hash-sweep --hash-seed 1`. The native sweep selects passing
executable corpus cases, omits cases with an LLVM exclusion row, and runs exactly
one optimized whole-program configuration on the shipped archive per target. Every selected
case must agree; unavailable cases fail. `--filter TEXT` narrows this sweep for
local checks, without changing the full gate's selection. The fast gate is unchanged.

`cargo run --release -p hirsel-conformance --bin profile -- PROGRAM [--cycles]
[--output DIR] [--root DIR] [-- PROGRAM_ARGS...]` builds an O3 measurement executable with source
line tables, runs it under `nice -n 10` with an 8 GiB process-tree RSS watch, and prints
up to ten source lines per nonzero counter kind with counts and percentages, followed
by `profile-json PATH`. Counts are physical ownership operations. This profile selects
regions off to expose heap snapshot duplication/release. Native benchmark ports under
`bench/native/programs` acquire the shared launcher automatically, retaining workload
line numbers; pass their normal case/input/rounds/emit arguments after `--`. The
input file is staged under an isolated runtime root. Other programs use an empty
root by default; `--root DIR` grants their input directory. Reports must be outside
the granted directory.

`--cycles` builds a separate production executable with line tables and samples it with
`perf record -e cycles:u`; `perf report --sort srcline` produces `cycles.txt` and up to
20 source rows. Unavailable/forbidden perf prints `cycles skipped REASON`. Counter
instrumentation never supplies cycle measurements. Artifacts default to
`target/source-profile/PROGRAM`. Source operations without individual LIR anchors use
the block anchor; runtime-only work appears at site 0. Full JSON retains function,
column and initiating LIR operation in addition to counts. Counts remain separate
from non-additive peak/gauge totals.

Runtime Fail header lines may belong to selected ordinary dependency sources and
therefore may exceed the primary fixture's EOF. Static Reject lines remain bounded
by that fixture; every line must be positive. Execution still compares the actual
rule and line exactly. The current header cannot name the source file; explicit
dependency-site validation needs a wider header/observation revision.


The full native campaign defaults to the derived `native-selection` Buck2
artifact. It records LIR opcodes and operand representations, runtime calls in
emitted LLVM IR, layout shapes and successful validation stages for every
executable input. Coverage keys retain their campaign: exact regression inputs
can cover agreement features, while full native needs a passing corpus witness.
Greedy coverage selection followed by deletion is
inclusion-minimal after historical pins, not globally optimal. Programs whose
features cannot be collected remain pinned. The corpus and front-end gate keep
every source. Individual C-validator and H-checker check kinds remain
uninstrumented; successful stages do not claim coverage of their individual
conditions.

`native-tests/regressions/history.tsv` is derived from all recorded exclusion
tables by `python3 tools/native-history.py`. Regression sources, smoke and
memcheck lists, current exclusions, and exact reduced agreement witnesses in
`native-tests/regressions/history/` are also pinned. Historical mutations are
replayed as exact programs, independently of the rotating per-program budget.

`native --full --seed N` rotates legal pairwise configuration covers; each
selected program visits every legal matrix row within 50 consecutive seeds.
Explicit `--filter` and agreement `--path` requests retain their requested
programs even if minimisation would omit them. Every current exclusion cell is
retained so its exact-row ratchet can detect a stale or changed exclusion.
`native --full --exhaustive` restores every baseline program and legal
configuration; `agree --native full --exhaustive` restores every mutation and
removes program and configuration thinning. These are the explicit release
checks. `--bless` requires `--exhaustive`.

Generate the cached coverage artifact without executing programs:

```sh
.buck2/bin/buck2 build //:native-selection --show-full-simple-output
```

A direct native CLI invocation without `HIRSEL_NATIVE_SELECTION` derives its
cover in its own scratch directory. Unfiltered Buck2 campaign shards receive
the generated artifact as an input. Its action key includes the compiler and
all corpus, package and library sources, so changing them regenerates the cover.
The whole `gate-native-full` target omits that dependency: explicit `--filter`
values select programs without loading or deriving a cover, while an unfiltered
whole-target run derives the same cover in scratch.

`native-select --replay-history` executes every reconstructable historical
source under the exhaustive matrix once and checks that all findings survive
every rotating slim configuration cover. It reports source snapshots it could
not execute. `--filter TEXT` or `--shard I/N` bounds this replay. `tools/native-plants.py --selection FILE --output DIR` starts
from a clean worktree and a frozen healthy selection, checks healthy controls,
then temporarily plants real AArch64 add lowering, system-allocator initial-count
and regions-off release omissions. It restores each source byte-for-byte and
requires identical full/slim finding sets for each plant. The script runs bounded Buck2 pool tests at lane priority with one CPU each,
and freezes the healthy cover as a declared source input. Add `--history-only`
to replay the protected historical set in pool shards, or `--plant NAME` to
run one fault. No native campaign lock is used.

Feature coverage, pairwise coverage and historical/mutation replay are
finite evidence, not a guarantee of identical findings for arbitrary bugs.
Higher-order interactions are covered only by seed rotation over time and
by the explicit exhaustive run.

Runtime failure token identities are resolved in the selected definition's
source snapshot, including dependency sources. JSON's `primary_external` marks
a site outside the case file; formatting that case leaves its dependency line
expectation unchanged.

`// expect: blocked` requires a CONC-8 Blocked observation with no language failure
or exit. Its stdout directives match the complete captured output prefix exactly,
including an empty prefix. Ordinary and checked interpretation must agree.
