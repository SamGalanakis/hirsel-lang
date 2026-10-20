# M0b: cost of ordered speculation and the task view

M0 is landed at `a8a54651` (instrumentation `4f177bb3`). Its task app
projections are only about 1.02x at light load and 1.04x under backlog at
four workers. Refinements remain part of the first ordered executor, enabled
by `reorder: auto`; those results are not a go/no-go gate. Free mode remains
first-class, including its independent tokio and replay gates.

This report adds measured switch/refresh/handoff costs, a bounded speculation
cost model, and a placement model over B1's real native workloads. The full
4/8-core tables are in [tables.md](tables.md), the inputs and unrounded results
in [costs.json](costs.json) and [results.json](results.json). A projection is
not measured multicore Hirsel throughput: W1/E1 do not exist on this base.

## Measured primitives

One session, ten seeded interleaved rounds, CPUs 31..28 / 31..24 (distinct
physical cores), all threads pinned; perf `cycles,instructions` inherits the
workers. Every raw sample retains perf running percentages, the busy share of
each SMT sibling, checksum, iterations, source and executable provenance. All
perf samples ran at least 95%. CPU31's timing lease serializes this session
with B1/tasks-rust. Absolute times below identify this session only; paired
instruction ratios are the verdict. Time ratios must not be compared with
M0's session. Startup is included in counters and amortized over two million
round trips per worker. Stack pages and the view are warmed before timing.

| Primitive | 4 cores ns/unit | Instructions/unit | 8 cores ns/unit | Instructions/unit |
|---|---:|---:|---:|---:|
| Bare fiber round trip | 45.15 | 76.96 | 50.22 | 77.04 |
| Bind/unbind, stable stamp | 49.92 | 111.08 | 60.39 | 111.20 |
| Bind/unbind, refresh every switch | 99.80 | 147.79 | 107.98 | 148.06 |
| Release/acquire baton, per transfer | 105.97 | 126.83 | 178.95 | 351.50 |

`switch.S` copies the x86-64 runtime's callee-saved register/control-word
switch and bootstrap. Each worker owns one stack and view. Stable and refresh
both transfer idle/fuel words, compare the worker/epoch stamp and switch twice
per round trip; refresh additionally forces a stamp change and copies 64 page
pointers per switch. This models the refresh branch, not a real task migrating
between OS threads. The C compiler retains the memory effects through an
opaque memory operand. `cost.c` is a standalone probe, not runtime code.

Forced-refresh/stable paired medians are **1.985x / 1.820x time**, and
**1.330x / 1.331x instructions**, at 4/8 cores. Time ratio ranges are wide
(1.225..3.323 / 1.063..4.312); the raw session retains that host noise. The
baton is a nonblocking scheduler-role cache-line handoff lower bound, not a
measurement of a parked/woken worker or a real scheduler-role implementation.
It does not substantiate the design's unverified 1–5 microsecond estimate.
The simulator charges one measured baton transfer per dispatch; real queues,
parking, NUMA and object locks may cost more.

## View selection

The default is recorded by `SELECTED_VIEW` and `ACTIVE_VIEW` in
`crates/hirsel-rt/src/multicore_policy.rs`. A 100% forced-refresh test gives
cost per refresh, not its frequency on a workload. The orchestrator's follow-up
requires B1 placement-model refresh frequency before selecting an indirection.
All eleven B1 traces are archived in [bench-traces.json.gz](bench-traces.json.gz).
At both widths, modeled migration/dispatch ranges from **0.0077%** (late set)
to **2.1277%** (allocation and roots). Yield is 0.0372%, channel 0.0249%,
independent files 0.6944%; see the full table. Widths agree because these
recorded spawn/readiness DAGs expose little additional parallelism.
**Select InlineEpochCopy**, keeping the existing generated-code layout.
The one-time cost of a full refresh does not justify an extra load on every
inline allocation at these frequencies.

For an added-cost target of 10%, linear interpolation of this session's paired
refresh/stable time ratios gives break-even refresh fractions of **10.15% at
4 cores / 12.20% at 8 cores**. The instruction ratios give about 30.3% at both
widths. These are cost-model break-even points, not precise host-independent
thresholds; the orchestrator's binding selection rule uses 10% modeled
frequency. Untraced page changes could raise total refresh frequency; W1/B1
must check that live telemetry before migration policy changes. Stable-worker
placement and epoch-only refresh remain necessary.

`mc-migration.py` preserves the recorded readiness DAG and retirement order.
It dispatches oldest-ready first, preferring the task's last worker and
stealing only when that worker is busy. Known keep entries (notably MutexTake)
are folded into their turn before placement. The denominator is turn dispatches (view-bind attempts), including resumption
after an idle worker receives a ping-pong message. Counting only changes in
the last task on a worker would greatly undercount ping-pong switches. First
binding is included in the denominator but excluded as a migration. Counts of
worker task changes are retained separately for audit.
Page/epoch changes are untraced, so the migration rate excludes those
refreshes. Unknown runtime entries conservatively split a turn. These are
model frequencies under one explicit locality policy, not live worker data.

## Refinement gain and limits

The cost model has eight logical tasks, four turns each, with an 8x-long first
head. Siblings start already queued. Each turn ends in one explicit D, E or H
operation. Strict commit, D run-ahead, and D+E early commit use the same work,
queue, handoff and 200 ns retirement cost. Logging is a stated 20 ns assumption,
not a measured runtime log cost. An early commit also pays 200 ns for its
scheduler-role action; retirement still pays its own bookkeeping cost. This
keeps the extra role cost visible. Change these assumptions in `analyze.py`.

The queue model enforces rule 1 using each predecessor's retirement time
against head-start time: one task can commit early within at most one queued
turn per head. D logs can span turns, bounded to 32 pending ops per task.
Only explicitly object-only E terminal ops can pass. Conflict checks include
all unresolved predecessors' possible read/write sets; real type closure,
held-child and poll footprints must be supplied by E1. A send's publication is
recorded only at its own retirement; the model retains a pending set before
then. Its tiny controls test these boundaries, not E0's semantic proof.

Distinct sibling objects pass the long head; a common object refuses all E
commits. For 50 ns turns, early commit can lose to strict commit because of
extra role work. For 100 us independent turns, gains flatten around 1.17x at
four workers and 1.10x at eight. This is the rule-1 ceiling in this fixed
queue, not a claim about all applications. Nesting a scope can expose more
fan-out; no unbounded bypass is assumed. D chains have more headroom, as the
full table shows. Head-of-line retirement and the serial role still limit
both refinements.

M0's archived logs contain no object identities, type closures or captured
footprints. The report replays their strict model at 4/8 cores, but reports
refined gains as unknown instead of inferring safety from operation names.
Likewise the B1 op-timing logs do not prove E eligibility. E1 owns measuring
actual refined throughput, canonical-trace equality and its positive long-head
benchmark when the executor exists; this report's synthetic scenarios do not
replace that gate.

## Committed auto policy

- Minimum segment: **2,000 ns**, rounding up eight 8-core baton transfers
  (8 x 178.95 ns). Shorter windows take strict commit.
- At least **25%** of bounded eligibility probes must pass. This covers D
  loggability or E's complete four-rule check, never just object disjointness.
- Measured saved compute must exceed extra role/log work by **10%**. A losing
  window switches back to strict commit even when the segment threshold passes.
- **64-turn** windows and periodic bounded probes allow a shape to recover from
  strict fallback. No permanent off switch based on one cold/noisy sample.
- **32** run-ahead ops per task; rule 1 remains an independent bound for E.
- Manager growth requires rising ready delay and scheduler-role busy share
  **below 80%**, as specified by §7. It still needs quotas, grants, permits,
  and the differential gate; production width stays 1 before that gate.

These defaults are named runtime constants with checked integer-boundary
helpers. `auto` stays the configured default; the helpers choose only placement
strategy, never semantics. The simulator's auto column rejects short,
conflicting and losing shapes. `AUTO_MIN_PASS_PERCENT` is a conservative
initial probe threshold, not an empirically universal optimum. E1/K1 own
feeding live samples and scheduling probes; those executors are not present
on this measurement lane's base. No ABI, tape, TIR/LIR or encoding is changed.

## Reproduction and validation

```sh
python3 reports/mc-profile-2-2026-10-20/measure.py
python3 reports/mc-profile-2-2026-10-20/collect-bench.py
python3 reports/mc-profile-2-2026-10-20/analyze.py
python3 tools/tests/mc_refinements.py
python3 tools/tests/mc_migration.py
```

The collector uses M0's sealed measurement helper/archive in
`/tmp/hirsel-mc-profile/pinned`; the archive's SHA256 is retained with the B1
traces, alongside exact source hashes. Rebuild them with M0's `build.py` first
if absent. The collector archives std at M0's source revision `8c1675a7a502`
to match the sealed precheck; workload sources are the unchanged B1 programs.
The default 1,000 B1 iterations and eight logical lanes are unchanged;
no trace time is compared across collection sessions. Reanalysis only needs
committed files. The external final design and an uncommitted toolchain are
not needed to reproduce the tables from archived traces.

The focused runtime policy run passed 2 tests. The new Python controls pass
13 tests (7 refinement, 4 migration and 2 evidence controls), alongside M0's
6 existing simulator controls. `analyze.py --check` verifies the committed
tables without changing them, using the Rust policy constants as its source.
Every collected B1 program's checksum agrees with its manifest at n=1000.
The repository gate and landing receipt provide the final broad-check evidence.
