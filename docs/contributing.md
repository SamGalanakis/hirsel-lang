# Contributing to Hirsel

This page covers building, testing and landing changes to the compiler,
runtime, libraries and documentation. For writing Hirsel programs, start
with the [tour](tour.md), [building and running programs](building.md) and
[concurrency guide](concurrent-agents.md).

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
- `proofs/rc/` is the Lean model of the reference-counting protocol,
  `proofs/ordered/` the TLA+ model of ordered multi-core execution, and
  `proofs/free/` the loom and GenMC models of the free-mode protocol
  (`crates/hirsel-free`); `proofs/free/check.sh` runs them.
- `bench/native/` is the native benchmark harness and its programs.
- `tools/` holds the Buck2 build (`tools/buck2/`), the pinned LLVM toolchain
  (`tools/native-toolchain/`) and gate scripts.

## Toolchains

Rust is pinned in `rust-toolchain.toml` (1.99.0) and installed by rustup.
The native backend needs LLVM 23.1.4 (`tools/native-toolchain/llvm-23.lock`);
`.cargo/config.toml` points `LLVM_SYS_231_PREFIX` at `tools/buck2/toolchains/llvm-files`
unless the environment already sets it. `tools/native-toolchain/README.md`
installs the pinned tree. Native tests carry pinned qemu, an AArch64 cross toolchain and Valgrind as
Buck2 inputs; only the Lean checks, the TLA+ model (Java 11 or later; `check.sh` fetches
the pinned `tla2tools.jar`) and the free-mode models need a host installation
(`proofs/free/check.sh` builds its pinned GenMC once into the cache; it needs cmake, LLVM
19-22 development files and libffi).

The build caps itself for a shared host: `.cargo/config.toml` limits Cargo
to 4 jobs and sets `RUST_TEST_THREADS` and `HIRSEL_JOBS` (in-process worker
threads) to 4. Export them explicitly when a wrapper might override them:

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
```

## Compiler std products

Buck2 test targets that run std programs get `HIRSEL_PRODUCTS` from
`//:hirsel-std-products`, which rebuilds whenever std or the compiler changes.
The ordinary `hirsel precompute <manifest> -o <directory>` command publishes
std through the same lazy typed content store used for user query products.
Test tools use the optimized `//tools/buck2:checked` profile, with debug
assertions and integer overflow checks enabled.

Precomputation publishes bounded query products and releases completed bodies.
Its memory cap remains `HIRSEL_MEMORY_LIMIT=2G` (`PRODUCT_MEMORY_LIMIT` in
`tools/buck2/defs.bzl`). A build that reaches the bound is a memory regression
to find, not a bound to raise. Provider bytes supply products; verifier-owned
local receipts remain a separate authority.

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
`python3 tools/buck2/conformance.py` runs its shards on the pool. Every corpus
file must pass its expected outcome unless `conformance/pending.txt` lists it
with a reason (`conformance/<path>.hir # <reason>`, sorted) or it carries an
open `// spec: #N` comment. A pending file that passes, or a pending path that
names no file, fails its shard, so the list only shrinks. The executable files
outside those exclusions are the native gate's baseline.

## Growing inventories: no hand-maintained totals

Do not pin the size of a growing primitive, ledger, registry, provider, test or
API inventory in an assertion, array type or fixture footer. Check the identities
instead: compare inventories in both directions, reject duplicate rows, and
verify every declaration's contract or implementation. Keep fixed counts only
when they express a format, a bounded domain or a test's explicit input.

Use inferred arrays or slices for extensible lists. A checked-in inventory has
one item per line, sorted by identity, with no trailing total. Generate derived
inventories with the owning tool and verify them with `--check` or an exact drift
test; regenerate them after a merge instead of hand-merging their contents.
`tools/bless.sh core-review native-inventory` regenerates the core review and
native operation tables; the registry and native coverage tests check their
identities and contracts. Only main's runs write `docs/api.md`; the full gate's
drift tests compare it with every public core and std export on main. Sorted
lists such as these merge without conflicts when contributors add or remove different
entries (the `hirsel` merge driver, `tools/buck2/merging.py`). The conformance gate checks
each sorted, unique `path # reason` line of `conformance/pending.txt`; the full-gate counter
checks every compiled test identity executes exactly once. Neither needs a total
updated when another contributor adds an item.

## Memory limits

Every hirsel binary and test harness caps its own memory before doing any work
(`crates/hirsel-guard`): a soft data limit of `HIRSEL_MEMORY_LIMIT` (bytes or a
`K`, `M`, `G` or `T` count; default `4G`), which programs they launch inherit;
only an AddressSanitizer executable is launched with it lifted
(`hirsel_guard::sanitized`), since its shadow mapping needs terabytes of address
space and the run's cgroup or pool reservation bounds it anyway. A
run that reaches it stops at once with `hirsel: memory limit 4G reached
(HIRSEL_MEMORY_LIMIT); ...` and exit status 71. The native benchmark suite
(`bench/native/suite.py`) sets the same limit on itself, so every build and
benchmark it launches inherits it. Under Buck2 each test's limit is
its measured grant (twice its `memory_kb` in `tools/buck2/test_sizes.bzl`); a test
that needs more gets a re-measured request, not a variable.

Every runner that spawns worker threads or processes (the agreement campaign,
`native`, the corpus-wide tests) takes its worker count from `HIRSEL_JOBS`,
default 2; under Buck2 the test launcher sets it to the action's CPU reservation.
No runner defaults to the machine's core count, and every worker allocates under
the same process limit.

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
shard, a campaign) go through their Buck2 targets, which run on the pool
within their measured reservations.

## Buck2

Hirsel builds and tests with Buck2. Cargo manifests and `Cargo.lock` stay
the source of truth: the BUCK files and the other build inputs
`tools/buck2/sync.py` derives are gitignored, and no change commits them
(`tools/buck2/README.md`, Generated files). `prepare.py`, `tools/land`,
`tools/main-runs` and `tools/bless.sh` regenerate them when the tree changed; before
a bare `.buck2/bin/buck2` after an edit or a rebase, run
`python3 tools/buck2/sync.py --if-stale`. The toolchains (Rust, the pinned LLVM, Buck2 and
its prelude) come in hermetically; nothing is installed system-wide.

```sh
python3 tools/buck2/prepare.py                  # once per checkout
.buck2/bin/buck2 build //:hirsel-cli-bin-hirsel
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
tools/land                                      # land it on main
```

Buck2 runs on a remote-execution pool whose endpoint, credentials and
platform properties live only in the gitignored `.buckconfig.local`, written
by a host hook (`tools/buck2/README.md`); without one `prepare.py` fails, and
`prepare.py --local` runs every action on this host instead. `-c hirsel.tests=host` explicitly opts into running tests on the invoking
host; native tools remain pinned inputs in either mode. More target names and the filtered native commands are in
[tools/buck2/README.md](../tools/buck2/README.md).

## Fast loop

While iterating, run the one test that shows the change, by name, then the
same test through the heaviest crate that reads it, and run the tests the
change can break once at the end:

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
38 s against 114 s over twelve paired edits). Leave `-c hirsel.full=true` to
main's runs.

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
checks only matched files and their pending lines. Every match must meet its
expected outcome.

## Testing and landing

For deterministic full-app native Pending/AfterWrite tests, use the public
[scripted fixture recipe](native-test-fixtures.md) with the same Args, files and
mount rights on each engine.

Run `.buck2/bin/buck2 kill` in your worktree when you finish and
before any wait longer than a few minutes: idle daemons retain substantial
memory on the shared host.

Run the tests your change can break, once, with Buck2: the test targets of
the crates you changed, changed and added corpus cases with their twins
(the same name up to the outcome word), and importers of the std or package
modules you changed. Chosen corpus cases, their agreement variants and
native programs run through the selected targets (`;` separates slices, `|` the paths of one slice):

```sh
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it
.buck2/bin/buck2 test -c hirsel.conformance_paths='<case>|<case>' //:gate-conformance-selected-shard-1
.buck2/bin/buck2 test -c hirsel.agree_paths='<case>|<case>' //:gate-agree-selected-shard-1
python3 tools/buck2/native.py <program> <program>  # cost-bounded shards, both backends
```

Each conformance slice validates every `|`-separated prefix against the whole
corpus before partitioning. A typo beside a valid case fails the slice before
case compilation. Overlapping prefixes run each case once. With an explicit
runner `--shard`, a globally valid empty partition reports zero selected and
executed cases and retains its count records; a missing report is an error.

The checked heap executor (smoke's `heap+inject` configuration) has no model of runners,
so a corpus case with a `// runners:` header fails there; judge it by the other
configurations. Land checks only what breaks everyone; main's fast run tests main every
2 h and the full suite runs by hand before milestones.

### Known failures

Failures main is known to have are listed in `tools/known-failures.tsv` (one
tab-separated row each: test id, reason, owner, date). An id is a test
target, a binary and case (`hirsel-rt-test-it::replay::kill_order`) or a
corpus case path. A main run reports no red the list explains. If you
encounter a failure your change did not cause, add a row and push that row
straight to main. Remove the row when fixing the failure.

### Landing

`tools/land` (in the background with a log, never under `timeout`) fetches,
rebases the worktree's commits onto origin/main, regenerates the build files
when stale (`sync.py --if-stale`), builds the workspace without the std
std image (a test that takes minutes), runs Clippy and checks formatting, all
through Buck2 at pool priority 75, above focused tests at 50
(`tools/buck2/platforms.bzl`), and pushes. A push rejected because main moved
goes back to the rebase; a red check refuses the push and prints its first
error; a conflict stops land with the conflicted paths. Land runs no tests.
After its rebase it runs the rebased tree's own `tools/land`, so a landed fix
to it holds at once. Each push appends a row (`landed`, `branch`, `base`,
`at`) to `hirsel-landings.jsonl` in the clone's common git directory, from
which a main run identifies a commit's contributor.

No change commits a generated file, so no rebase conflicts on one: land
refuses a change that touches `docs/api.md` or a gitignored build file, with
the commands that drop it. The files contributors commonly add to merge with
`tools/buck2/merging.py`, the `hirsel` merge driver `.gitattributes` names:
rows both sides append to `third-party/NOTICES.md` are both kept, package
manifests (`hirsel.json`) merge their sources and dependencies structurally,
and the sorted ratchet and expectation lists keep the entries either side
adds and drop those either side removes. Resolve any other conflict by hand.

### Main's runs

`tools/main-runs --tick --tier fast|full` runs main's suite in one of two
tiers at pool priority 40. The fast tier (the default) runs prepare,
`sync.py --check`, the tool tests, the workspace build, Clippy, fmt and the
workspace tests, GenMC models and the fresh std compiler instruction budget
(about 12 min). The full tier runs those and then the release
gate: the `full_gate_` tests in debug and release, the whole conformance
campaign and ratchet, the unrestricted source/IR and native agreement
campaigns, the instruction budgets, native smoke, release tests, the native
matrix, every `tools/check-native.sh` check and both hash-seed sweeps. It
takes hours and runs only by hand, before milestones
(`tools/main-runs --tick --tier full`, in the background with a log).
`--dry-run` lists a tier's stages.

A systemd user timer (`hirsel-main-runs.timer`, every 2 h; host config, not
in the repo; it needs no session) ticks the fast tier through
`hirsel-main-runs.service` (Nice 10, at most 8 GiB and 4 CPUs), which runs the
copy in the runner's worktree (`$HIRSEL_MAIN_RUNS/worktrees/main-runs`, by
default under `~/.local/state/hirsel/main-runs`). One lock admits one run: a
fast tick skips while any run is going, and a full tick waits for it. A tick
skips when main has not moved since the last run that covered its tier (a
full run covers fast); otherwise it checks out origin/main there and runs
that commit's copy, so a fix to the runner holds from the next run. A run
executes its tier's stages except those `tools/main-runs-skip.tsv` (stage,
reason, owner, date) names.

Both exception tables have exactly four nonempty tab-separated fields, unique
identities and a real `YYYY-MM-DD` date. Blank lines and lines starting with
`#` are ignored. Skip identities must name a defined stage (including full
stages during a fast run). A malformed row, duplicate, missing table or
unknown stage stops execution as a configuration failure, with its filename,
line and reason; that failed attempt does not cover the SHA for future ticks.
An owner not yet assigned is written explicitly as `unassigned`.

The report and the existing `runs.jsonl` history add `exception_status`
version 1 alongside the unchanged code-red routing fields. It records table
SHA-256 digests, the run SHA/tier, planned and unselected stages, skipped
rows, selected commands (including flags and queried target inventory), and
all decoded target/case observations with stage, exact identity, log path
and TestRun execution provenance. Observed failure, pass, skip, omission,
infrastructure and unusable evidence remain distinct. Every known identity
has a disposition even when it is unobserved; absence is not proof that a
case was missing or outside the selected tier. Parent evidence is context
for an unobserved case, never an invented child result.

A recovery candidate requires a fresh exact pass and complete libtest
execution evidence. Cached output retains its digest/timestamps and does
not count as fresh execution or recovery; absent or ambiguous provenance
stays unknown. A target-wide row requires its entire expected population:
an unfiltered, nonempty, complete target execution, or all shards proven by
the owning workspace query, with no ignored or filtered-out children.
A passing child or partial shard selection is insufficient. Runners without
this completeness evidence keep their observed scope and unknown recovery.
Reason text is explanatory, not a failure signature. Candidates are reported
for their observed stage; rows are never automatically deleted and no full
run is started to resolve them. This is an exception census, not a test pass
percentage. The structure follows the strict pending-entry checks in
`crates/hirsel-conformance/src/lib.rs`; the reasoned-exclusion accounting
motivation is Koru's `challenges/016_the_honest_board.md:131–155`.

Each run writes one report, headed with its tier (`$HIRSEL_MAIN_RUNS/reports/<time>-<sha>.md`, the
newest copied to `latest.md`): every red the known failures do not explain,
with its likely culprit (of the commits since the last run of its stage, the one that
changed the most files under the red's crate or among its failing corpus
cases) and that commit's contributor from the landings log. Fix failures
attributed to your changes. After the run it regenerates `docs/api.md` with the built CLI
and after a full run ratchets `tools/perf-counts.tsv` down to what `gate-perf-counts` measured
beyond the 1% noise, pushing each change straight to main.
`tools/perf-counts --bless` measures on the pool and rewrites the table: the
reviewed step after an intended change of counts.

Both tiers also run `tools/perf-counts --fresh-std` on the host. It builds the
release compiler and checks std at jobs 1, with daemon, products and result
reuse disabled, under `tools/capped` with 2 GiB and 60 CPU seconds. Whole-process
`perf stat -e instructions:u` must stay within 3% of `tools/perf-fresh-std.json`;
kernel-inclusive instructions are printed as context. Missing or multiplexed
counters and failed checks fail the stage. Update this baseline manually in
the commit that intentionally changes it, with a one-line `reason`. Neither
blessing nor the automatic Cachegrind ratchet updates it.

Changes to native code also run the filtered native matrix for the programs
they add or touch on the pool. Focus runs book one CPU at priority 50:

```sh
.buck2/bin/buck2 test -c hirsel.full=true -c hirsel.tests=focus //:gate-native-full -- --test-arg=--filter --test-arg=<program>
```

A change to LLVM emission must carry the matching Cranelift change.

For changes to `crates/hirsel-llvm`, `hirsel-cranelift`, `hirsel-lir`, `hirsel-rc`
or the runtime, run the programs they affect through the LLVM native smoke
matrix and Cranelift parity (`tools/buck2/native.py`, above).

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

### Native coverage

Native and scaling targets catch defects that the interpreted corpus cannot.
Main's full suite runs them by hand before milestones; the fast tier runs every 2 h. Run the parts your change can break:
the `gate-std-native` shard holding a std module you changed
(`--std-compile --filter <module>` names one module),
`gate-native-bugs` for lowering, backend or runtime changes, and the
checker-scaling shards for front-end changes.

`gate-std-native-shard-<i>` (`native --std-compile`, four module shards)
builds every public std@1 function through the native stage validators and
both Cranelift and LLVM at O0, without linking or running anything. Generic
functions get monomorphic wrappers at representative types (`u64`, size 4),
and the compile-time generators of `std.derive` compile their generated bodies
for a representative record. One process over all of std (114 modules, 930
public functions, 4604 units) took 931 seconds and 3.0 GiB on the dev
compiler, about 160 seconds of it the first module's source audit, which each
shard pays once; the four shards took at most 333 seconds each. With their std
workarounds reverted, `http2.drain_frames` failed its import validator
(CleanupOrder) and `hpack.lookup` its ready validator (MovedPart) before
2c871f85 fixed both.

`gate-native-corpus-shard-<i>` (`native --corpus`, 32 shards) runs every
runnable corpus case (run, fail, blocked and test headers; about 2,560)
natively and compares the full interpreter observation: stdout, exit status,
failure rule and site, with headers and `.stdout` sidecars as further oracles.
Every case runs on Cranelift; a 1/16 slice that rotates with the run's seed, and
every case with a pending row, also run on LLVM O0. A case whose header names
`runners:` passes them, its `fault_seed:` and the launcher's callee facts to the
executable, whose callee runs execute in its process as the interpreter's do.
`native-tests/regressions/corpus-exclusions.tsv` names the cases never launched
natively (MEM-12), and `corpus-pending.tsv` the failures still attempted on
every run, each with its exact reason; an unlisted failure, a changed reason
and a pending row that now agrees all fail. At 16 shards a shard took 838 to
1503 seconds.

`gate-native-bugs` (`native --bugs`) runs a generator seeded from GAP 4--8:
literal matches on destructured and borrowed scrutinees, let-else payloads,
`replace`/`take` inside tuple, record and variant construction, const `Inline`
reads into aggregates, `?` in arms, and loops writing fields of a `mut`,
`unique` or nested collection. It enumerates all 18 place x operation x context
combinations (`generated/bug-0` to `-17`), interpreter against LLVM O0 and
Cranelift. Known failures are rows of `native-tests/regressions/bug-pending.tsv`,
judged like `corpus-pending.tsv`; a new failure prints a line-minimized program.

`gate-checker-scaling-shard-<i>` (`tools/checker-scaling.py`, one family per
shard) checks generated programs at N=10, 40 and 160: `match` arms assigning
record fields through `?`, record field count, nested loops with `?`, closure
captures, long `if`/`else if` chains and deep nesting. Instructions are
Cachegrind Ir under the pinned Valgrind, deterministic as in `gate-perf-counts`;
peak RSS comes from a separate uninstrumented run, and wall time is printed
only for diagnosis. Over an empty-program baseline (0.59 billion Ir), the
marginal cost may grow at most twice N log2(N) between consecutive sizes, plus
16 MiB RSS noise. Each cell has a 2 GiB data cap and a 45-second limit (240
under Cachegrind); reaching either is a finding. The match/try/field family
was red for GAP 5 (from N=40 to 160 its marginal Ir grew 18-fold against an
allowed 11-fold) until 19aa5a01 made that check linear; `EXPECTED` lists any
family expected to fail, with its owner, and an unexpected pass fails until its
row is removed. The first shard takes about 70 seconds and the others about 40
each.

Tests that execute the full native matrix or take about a minute or longer
carry `#[ignore = "full gate"]` and a `full_gate_` name prefix. Keep their
inputs and assertions intact; add a separate bounded smoke test when useful.
Cases that call `full_matrix` only for discovery, and the drift tests of
`docs/api.md` (current on main alone, since only main's runs write it), are
skipped by name outside the full suite (`FULL_GATE_SKIPS` in
`tools/buck2/sync.py`).

`tools/main-stall-alarm`
appends to `alarms.log` in `$HIRSEL_MAIN_RUNS` when main gets no commit for
40 minutes while contributor worktrees are moving; it can run from a
repository maintenance loop.

A PROG-3 corpus case may use `// expect: refuse entry` with an accepted or run
`twin:`. The source still checks normally; the conformance driver then calls the
production entry admission validator and requires an invalid-entry refusal. This
keeps ordinary functions returning other types legal without treating a launch
refusal as a language failure.
