# One-body rebuild attribution — session 01a161c9

Frozen source: `8bedce64d25ba757e6bf5a14ebb10423703d180c` (current origin/main at
launch, not historical `9f161d96`). No compiler optimization, language rule,
TIR/LIR, ABI, existing benchmark or README changes. The harness is
[`incremental-profile-9`](incremental-profile-9); its fixture is
[`main.hir`](../tests/fixtures/incremental-profile-9/main.hir).

## Inputs, controls and oracle

`helper(4)` computes `x + 1`; the real body edit changes only `1` to `2`, keeping
`helper(x:u64)->u64`. Main executes the helper and prints `5\n` or `6\n` through
core stdout. An unexpected result fails. Controls: exact unchanged source,
`RECOMPILE_MARKER_0` to `_1` in the preceding comment, and changing the helper's
parameter/result from u64 to u32 (the literal caller remains valid). Each cell
has a new workspace and private result/native/object/runtime/temp namespace.
The exact base is copied before preparation and restored in `finally`; each
patch and edited source hash is retained. No shared cache is cleared.

Three independent modes: resident **check** with acknowledged dispatch to an
owned server; daemon-off fresh-process disk-warm **check**; daemon-off native
**build --debug** (Cranelift). They are not mixed into an incremental ratio.
Every edit is checked and executed by a cache-free fresh interpreter. Every
native output is executed and compared with a separate cache-free native build.
All 40 native output pairs in the accepted matrix also have identical SHA-256
binary bytes. No diagnostic is hidden by ignoring a failed build. V stays on;
no `--no-verify`, candidate fork, cache-policy rewrite or compiler rebuild occurs
inside measurement. The fixture selects no std dependency, but the normal CLI
still opens its sibling std image; that cost is deliberately retained.

Each matrix is ten seeded interleaved rounds (`20261022`), four edits × three
modes = **120 measured cells**. One job, ordinary CPU6, 4 GiB cap. CPU31 is never
used. `perf stat cycles,instructions` inherits compiler threads and descendants;
resident server counters use the existing enable/ack protocol and are added to
client counters. Every PMU counter reports 100% running coverage. No exclusive
wall timing lock was held; all wall figures below are attribution observations,
with **no wall ratios or speedup claims**. Host: Ryzen 9 5950X, 32 logical CPUs,
Linux 6.8.0-137, perf 6.8.12. SMT and other lanes remain potential wall noise.

Std objects: `none` (matches the old comparison); ordinary result/object reuse
remains `dir@1`, program shortcut cache is off, page cache is not flushed. Native
runtime and link inputs were prebuilt once; no Cargo dependency/runtime rebuild
occurs in a measured process tree. `strace -f -e trace=process` confirms exactly
one `ld.lld --threads=1` and two `llvm-objcopy` descendants for the real edit.
Full descendant argv and retained per-PID Callgrind artifacts are in the session.
`analyze-callgrind.py PROFILE_DIR` reproduces their instruction/allocation boundary
summary; its checksum is in `final-source-identities.json`.

## Instruction evidence

Accepted `matrix-final-10`: median retired instructions in millions, including
the client plus resident worker where applicable. These are costs, not an
optimization before/after comparison. Cycles and all raw samples are retained.

| Mode | Unchanged | Marker | Real body | Signature control |
|---|---:|---:|---:|---:|
| Resident check | 45.892 | 47.267 | 47.272 | 59.234 |
| Fresh process, warm disk check | 299.463 | 301.553 | 301.446 | 308.204 |
| Native debug build | 1273.464 | 1269.123 | 1269.054 | 1314.106 |

The slight native ordering is not evidence that editing speeds compilation up.
The useful finding is that unchanged, marker and body rebuilds all retain large
common native costs. Signature changes perform more semantic work.

| Counter (constant across the ten cells of each group) | Disk unchanged | Disk body | Disk signature | Native unchanged | Native body | Native signature |
|---|---:|---:|---:|---:|---:|---:|
| Checked definitions | 0 | 1 | 2 | 0 | 1 | 2 |
| Query recomputes | 6 | 9 | 12 | 6 | 10 | 14 |
| Store hits/misses | 2/0 | 1/1 | 0/2 | 12/0 | 10/2 | 8/4 |
| Lowered bodies / full V runs | 0/0 | 0/0 | 0/0 | 0/0 | 1/1 | 2/2 |
| Object hits/misses | — | — | — | 12/0 | 11/1 | 10/2 |

Marker matches body counters on this fixture: the comment belongs to the helper's
syntax region. Eleven native cached Values are still decoded/rebound (62,946
encoded bytes); an object hit does not mean zero import/validation/identity work.
Fresh-process invalidation counters are zero because these are new engines,
not evidence that a signature dependency did not change. The CLI's timing flag
bypasses resident dispatch, so resident query/phase counters are **unavailable**
here; acknowledged server execution and whole-server PMU counts are retained.
Dirty counts alone never establish semantic work.

## Attributed body-edit cost and residual

`profile-2` uses a separate restored/warmed namespace for **each** profiler and
asserts one object miss, preventing a previously cached body variant from
masquerading as its first edit. The first uninstrumented timing report partitions
249.384 ms as follows; these additive driver intervals are observations from one
profile, not accepted wall benchmarks:

| Interval | ms | Fraction |
|---|---:|---:|
| Link/toolchain/output | 180.803 | 72.50% |
| Std image open/decode | 21.465 | 8.61% |
| S0 load/declarations/check setup | 13.545 | 5.43% |
| Parallel2 lowering/V frontier | 11.619 | 4.66% |
| Native units | 9.176 | 3.68% |
| Specialization/native plan | 7.105 | 2.85% |
| Front preparation | 1.324 | 0.53% |
| Parallel front | 1.159 | 0.46% |
| GEN-5/S1/merge | 0.022 | 0.01% |
| Untimed/startup residual | 3.166 | 1.27% |

Phase CPU observations cross-cut these intervals and must not be added to them:
parse+lex 0.068 ms, check 0.163, modes 0.375, lower 0.613, V 10.367,
plan 6.544, import 6.268, codegen 1.220, store lookup 1.405. V's phase includes
receipt/validation bookkeeping; the count of full V runs is one.

Instruction sampling (`instructions`, 999 Hz, DWARF stacks) captured **235
samples, zero lost**, about 1.268 billion instructions. Sampling shares are
approximate and too coarse for small functions. Ranked causes and ownership:

| Rank | Attributed instruction share | Evidence / minimal trigger | Route and confidence |
|---|---:|---|---|
| 1 | 41.69% | 23.29% ld.lld, 18.40% two llvm-objcopy processes, despite 11/12 object hits; any helper body edit in this fixture | Unowned `native-link-repeat-9` brief via orchestrator; high for process attribution, medium for exact share |
| 2 | 13.61% | SHA-256 leaf; 8.52 percentage points have a `kit::Link::command` input-hash stack, 1.83 have V input-digest stacks | Link identity to `native-link-repeat-9`; V digest to `modes-flow-2`; medium, preserve content verification |
| 3 | 4.77% | 3.28% `StdImage::from_image` and 1.49% bincode varint decode leaves; normal fresh CLI still loads the std image on this std-free program | `app-perf-refresh-2` for application refresh/startup attribution; medium, decode leaf does not prove every decode is std |
| 4 | 39.93% residual | Remaining compiler/startup/kernel/allocator/import/backend leaves; no single causal attribution established | Concrete `native-remaining-attribution-9` brief; report as unaccounted, do not label it all parse, V or invalidation |

The shares form a leaf/process partition; SHA's two stack subshares are included
inside 13.61%, not added again. Interval link time includes hashing, tools and
copies; it is not synonymous with ld.lld CPU. Allocation/worker scope goes to
`mc`, with **no multicore scaling conclusion** from this one-job experiment.

## Allocations and coverage

Keep allocation observations separate from instruction costs. The pinned
Callgrind run records compiler function-entry calls to `mi_malloc_aligned`:
**307,591**, `mi_zalloc_aligned`: **265**, `mi_realloc_aligned`: **55,256**.
These are allocation/reallocation boundary calls under simulation, not requested
byte totals, distinct live objects, or a production allocation ratio. Per-PID
libc boundaries: ld.lld malloc/calloc/realloc **19,358/123/2,551**; first objcopy
**23,843/104/0**; second **23,870/104/0**. No allocator counts are inferred from RSS.

Heaptrack observes only **3,645** compiler interposed allocation calls (45
temporary); the linked mimalloc Rust allocations are outside its interception.
That count is not total compiler churn or a descendant aggregate. BPF setup
refused non-root use; its failed attempt is retained. `--mem-usage` independently
reports 9,874,855 held std-image bytes, net requested-heap checkpoints of
13,570,441 bytes after front completion and 14,753,021 after link; peak compiler
RSS is 155,344,896 bytes. Net held bytes are not cumulative allocations.
Callgrind's simulated Ir differs materially from PMU instructions (including
CPU-feature/loader behavior); it is retained per process and not pooled into
PMU ratios.

## Historical marker diagnostic

One build-only reproduction on the frozen current LangArena port (47 registered
tasks, versus the upstream 50-task array), no runtime campaign and no Rust
comparison build: cold preparation 41.587 s, marker singleton 5.223 s and
40,690,197,142 retired instructions. These seconds are standalone diagnostics,
not a cold/incremental or language speed ratio. Historical `9f161d96` had 45
Hirsel versus 50 Rust tasks and its 44.8/29.9 s and 21.0/4.5 s remain singletons.
Current marker reused 829 objects and rebuilt one, yet checked 46 definitions,
lowered nine, ran 54 full Vs and recomputed 884 queries. Its `import` phase alone
reported 2.052 s of work and native units 2.748 s of additive interval time.
That suite-shaped trigger belongs in follow-on attribution, not a claim that the
minimal fixture explains every second of the historical suite result.

## Reproduction and retained artifacts

Evidence root on this host:
`/workspace/code/hirsel-briefs/incremental-profile-9/session-01a161c9`.
`source.tar` and `source/` retain the frozen Hirsel tree; `upstream.tar` and
`upstream/` retain LangArena at `3c9f84ddaeec834c0631747cb789e8cfd602f993`.
`bundle/` retains the executable and std image; `native-inputs/` retains complete
LLVM, runtime, link-kit and Valgrind trees independent of the removed worktree.
`frozen-environment.json` maps those retained inputs. `inputs.json` in every
session enumerates SHA-256 of consumed tracked and untracked files, with a
post-consumer `frozen_verified=true` check. Original paths map by suffix to the
retained native-inputs copy; its hashes were independently frozen in the final
matrix. Existing `resident`, `corpus.py`, `measure.py` and the exact harness are
also hashed. Complete commands, policies, stdout/stderr, perf CSV/coverage,
source edit patches, source hashes and output hashes are in each result.json.

| Artifact | SHA-256 |
|---|---|
| Compiler | `f7ec897cad1d2c251d1964b3cc28b553ce655164de5bce048745fe900a47b4c2` |
| Std image | `9dd859b415f6e532f1f219f67377b77be645a58f4fef9d902c51067322107172` |
| Hirsel source archive | `6e304be29f9aa272a7c608e7934f7617eb05348596859e99699a2a3471961a77` |
| LangArena source archive | `61599dd599f5689d6e8968fb4b883fa57e2f56864558610aa0213b6afedc8a9f` |

From this repository with an already prepared frozen toolchain and a new WORK:

```sh
W=/workspace/code/hirsel-briefs/incremental-profile-9/session-01a161c9
tools/capped --timeout 900 taskset -c 6 tools/compiler-bench/incremental-profile-9 matrix \
  --cli "$W/bundle/hirsel" --env "$W/frozen-environment.json" --work "$WORK" \
  --rounds 10 --seed 20261022 --freeze "$W/bundle" --freeze "$W/native-inputs" \
  --freeze "$W/source" --freeze "$W/upstream"
# Use a distinct new WORK with `profile` to collect stacks, live bytes,
# heaptrack, descendant exec argv and pinned per-process Callgrind output.
# `legacy --legacy-root "$W/source" --cpu 5` runs only the two build commands.
```

Do not run a compiler build while these consumers are active. Freeze all actual
native paths passed in the environment; check the hashes after consumers exit.
The driver timing schema and cache policies are described in docs/building.md.
For a new machine, prepare the exact scoped CLI/native targets once through
Buck2, outside measurement; the retained build/target-discovery logs identify
all labels and release configuration. Long commands are wrapped in owned logs
with unconditional `EXIT` markers. This report contains no credentials or full
ambient environment dump.

Attempts retained separately: three launch-fixture probes (the first two fail:
invalid entry contract, then std-free f-string); one successful one-round smoke;
original `matrix-10` (120 passing cells, frozen); `profile-1` (later profilers hit
an already-cached edit, excluded); corrected `profile-2`; one `legacy-1`;
`matrix-final-10` (120 passing cells, frozen). The second matrix's changed
obligation was persistent consumed-input identity: move native inputs out of
soon-to-be-removed buck-out into retained copies and filter unrelated ambient
credentials/session variables. It is not repeated sampling for confidence.
No third matrix or second legacy campaign ran. Exact earlier harness versions
are retained. Two Buck launcher permission failures preceded the executable
mode/cache-key correction. Final scoped Buck target: **7 unit tests passed**;
no intended test filter selected zero. No broad corpus campaign ran.
First landing was refused by inherited main E0063 in checked-shadow test fixture
`Body.scoped_parameters` (1087-second build attempt). The already-tested repair
landed on main as `ce3a275c`; normal rebase/retry reuses it, with no duplicate
fixture repair or repeated diagnostic campaign. The original red log is retained.

## Implementation-ready follow-ons

* **native-link-repeat-9** (currently unowned; ask orchestrator to assign): use
  this fixture's unchanged/body cells and retained process.trace. Separate
  runtime/object content hashing (`native/kit.rs:181,270`) from ld.lld and debug
  splitting (`native/link.rs:1761`). Keep content identity and fresh-cache
  byte/diagnostic parity. Compare debug with a matched release control before
  choosing a change; release has no two debug-copy processes. Acceptance:
  ten seeded instruction rounds and allocation boundaries, no validation bypass.
* **modes-flow-2**: V input-digest stacks (1.83 sampled points), one full V on
  minimal body edit versus 54 on the suite marker; inspect receipt/input
  reconstruction before treating dirty counts as repeated semantic checking.
  Acceptance: exact one-body output and cache-free diagnostic parity, count
  physical V work and receipt validation separately.
* **app-perf-refresh-2**: fresh-process image load on the std-free fixture,
  9.875 MB retained image, and final resident/disk instruction cells. Preserve
  normal daemon compatibility and selected-std semantics. Any startup change
  needs a separate frozen before/after experiment; none is implemented here.
* **mc**: retain the one-job process-tree and per-process allocation evidence
  as a baseline. This lane measures no scaling or lock-contention cause. Do not
  infer worker speedup from phase work time; any further jobs comparison is its
  own bounded instruction/affinity experiment.
* **native-remaining-attribution-9** (unowned): partition the residual 39.93%
  and the suite marker's native import/validation work. Minimal trigger is the
  same helper edit; suite trigger is only the retained marker patch. Add
  attribution at existing read/decode/native-plan seams before proposing an
  optimization. Resident phase/query counter coverage is also unresolved: the
  public timing flag intentionally takes the fresh-process path.

## Prior art and deviations

Rustc dev guide at `aa181c58c5ad008b67fa8df22b3e610cb9c81dc8`:
`src/profiling/with-perf.md:18-48` supplies DWARF sampling and build-before-profile
procedure; `src/queries/incremental-compilation-in-detail.md:16-105` supplies the
dependency-change versus actual-result-change diagnostic distinction. Retained
reference notices are MIT/Apache-2.0; LangArena's retained source is MIT,
Copyright 2026 Konstantin Makarchev. No external implementation is vendored into
this change. Semantic adaptations: keep incremental caches enabled; use an
instruction event at 999 Hz rather than the guide's generic 99 Hz to collect a
bounded short compiler profile; retain resident and disk modes separately;
execute the changed function rather than treating a dirty query as proof.
The existing local resident helper's enable/ack PMU protocol is reused.
