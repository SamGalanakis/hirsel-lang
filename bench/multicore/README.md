# Multicore B1

One command builds the locked tokio references and native LLVM programs and runs
all workload cells in a seeded, ten-round, interleaved session at 1/2/4/8 cores:

```sh
bench/multicore/run
python3 tools/tests/multicore_bench.py
```

Requires Linux, the repository Buck2/native toolchain, Cargo, perf counters,
`taskset`, `/usr/bin/time`, and eight physical cores in the admission affinity.
The default worker grant is CPUs 31,30,29,28,27,26,25,24 (client CPU7 is excluded).
Use `--cpus` to select another eight distinct physical cores. The host's CPU31
measurement lease serializes sessions with the tasks-rust suite. Each measured
program is one `tools/capped` call with a width-matched CPU quota; perf inherits
all its threads, and reads cycles, instructions, task-clock and duration_time.
SMT siblings' busy shares and the actual capped scope's `throttled_usec` deltas
are archived for every sample. No counters or timings include a capped queue wait.

The archived [baseline tables](/workspace/code/hirsel-briefs/multicore/repo-evidence/bench/multicore/results/baseline.md) and compressed raw session
contain 810 successful checksum samples: 81 cells × ten rounds. Re-render with
`bench/multicore/run --report-only --output /workspace/code/hirsel-briefs/multicore/repo-evidence/bench/multicore/results/baseline.json.gz
--report /tmp/b1-baseline.md`. This archived run predates C1/W1/F1 and the landed
tasks-rust server, so its free-mode and headroom cells explicitly remain
dependency-owned. The `extern` tokio reference is built; the current native
comparison has eleven programs.

The instruction ratio is the verdict. Time ratios have meaning only inside this
one session. Process-lifetime measurements include startup and teardown; short
cells expose launch cost. Increase `--iterations` for steady-state work. Never
pool sessions, or present ten process durations as operation p99. Peak RSS is
`wait4` high-water RSS from GNU time, not a resident snapshot. Raw samples retain
perf multiplexing percentages, binary/source SHA256, revision, checksum, scope,
SMT load, diagnostics and recording size. A changed sealed input refuses reuse.

`manifest.json` is the workload and acceptance contract. Width does not change
the amount of work: eight logical lanes share the work at every width. Programs
print a deterministic checksum. Recording off/on/durable differ only in run
policy. Each sample gets a fresh, empty mount and a recorder outside that mount.
The runner checks exact stdout before admitting a sample. `generate` reproduces
the checked-in Hirsel sources. Arithmetic stays well below u64 overflow.

| Workload | Algorithm and matched tokio reference | Intended evidence |
|---|---|---|
| yield | eight children, n cancellation-check scheduling points each; tokio yields | switches/queue work and migration pressure |
| fork_join | n immediately awaited children, each returns i+1 | spawn fast paths |
| late_set | start a/b before constructing the set, wait for a's signal, insert their existing handles, await first, then start/insert c, drain all | late registration and completion ranks; FuturesUnordered of JoinHandles avoids extra wrapper tasks |
| atomic | eight children load one SeqCst scalar set to 7, n times | handle/atomic lookup |
| roots | eight readers clone and release a shared 64-element graph per iteration | root traffic and remote release |
| cancel_depth | eight children check through four nested scope ancestors | cancellation depth; tokio reference reads five uncanceled ancestor flags and consumes its cooperative budget |
| mutex | eight children increment one mutex-protected scalar | hot-object throughput |
| channel | unbounded ping-pong, n round trips, value i becomes i+1 | contention and recording |
| independent_files | eight tasks each open a distinct file in the SAME mount, retain it and write n one-byte requests | ConcurrentAdapter overlap and shared namespace epochs |
| namespace_mix | eight tasks create, rename and unlink their own names in one mount | run-wide namespace exclusion |
| allocation | eight children build/release a fresh 64-element dynamic list by incremental pushes each iteration | logical meter and allocator traffic |
| extern | eight children call a separate C translation unit's identity function n times | tokio foreign baseline; Hirsel trusted thread dispatch is R8-owned |

The synchronous Rust file operations deliberately run on the calling worker,
matching bounded Hirsel host calls. They never hold a resource lock across an
async wait. Namespace workload names are disjoint; all share the same physical
mount. File overlap must come from runtime request intervals; throughput alone
does not prove overlap. The sources exercise it today; the report leaves the
overlap metric missing until R6 supplies those intervals. Cancellation baselines
are matched *work*, not a claim that tokio exposes Hirsel's scope semantics.

## Acceptance and dependency ownership

`run` writes raw JSON, an analysis JSON and Markdown with baseline, floor and
per-row gap tables. It exits **2** for incomplete/dependency-owned/partial evidence,
**1** for an execution failure, **0** only for complete acceptance. A successful
baseline-only run is intentionally not F1 acceptance. The report lists every
headroom metric for each relevant workload and width; missing is `null`/`—`, never
zero. Thresholds, including R1's 5% width-1 noise allowance, are in the manifest.

Free mode must meet every floor independently: off throughput >=0.8 tokio,
recording >=0.9 off, operation p99 <=1.2 off, compressed bytes per contended event
<=4, and durable throughput >= cooperative durable at width 1. RSS, throttling,
flush CPU and flush cost per prepared request are reported. Correctness,
conformance, cross-engine replay and differential gates remain independent.

Today's cooperative runtime is measured at width 1 only. The suite does not
mislabel a one-thread run as an eight-core run. Free provider/placement/execution
and internal counters are dependency-owned by C1/W1/F1/R1–R8/K1. No runtime, ABI or
compiler change belongs to B1. Extern Hirsel wrappers with `threads` declarations
are R8-owned. All eleven headroom rows retain an unblocked optimization in design
§5; missing metrics are work for those lanes, not an accepted speed ceiling.

A later executor supplies a sealed inventory via `--participants inventory.json`:

```json
{
  "participants": [{
    "name": "free", "provider": "tasks.free@1", "placement": true,
    "widths": [1, 2, 4, 8], "modes": ["off", "record", "durable"],
    "commands": {"channel": ["/absolute/path/channel", "--hirsel-grants", "{grants}", "{iterations}"]}
  }],
  "sha256": {"/absolute/path/channel": "sha256-of-binary"}
}
```

All participants use explicit argument vectors (no shell). Substitutions are
`width`, `iterations`, `root`, `grants`, `recording`, `metrics`, `mode`. Hirsel
participants get `--hirsel-record` in recording modes. An adapter can set `env`
and `cwd`; it must emit the oracle checksum and consolidate runtime metrics into
one final schema-1 object at `{metrics}`. `placement.metrics` is a runtime JSONL
stream; the adapter owns converting that stream, not the benchmark runner.

Telemetry is `{ "schema": 1, "width": 8, "metrics": { ... } }`. Metric names are
exactly `headroom[].metrics` plus `latency_p99_ns`; all values are finite,
nonnegative numbers. Units are in names (`ns`, `per_op`, `per_s`); switches count
TaskCtx switch_out/switch_in pairs, refresh time counts Heap::bind's stamp-mismatch
branch. `speculation_pauses` is a total; additional `speculation_pauses_<cause>`
fields retain cause breakdown. `compressed_bytes_per_contended_event` counts
compressed scheduling data only, excluding headers, initial trees, unrelated Io
payloads and uncompressed buffers. `flush_ns_per_prepared_request` divides actual
flush cost by prepared requests, not by all semantic operations. Zero counts are
valid only when actually observed. No latency percentile is synthesized from
throughput. Metrics collection that changes overhead belongs in a separate
instrumented diagnostic inventory; it must not contaminate timing comparisons.

For R1, add two width-1 participants named `before` and `after` to one inventory
and run the same session. `compare-width1 session.json --before before --after
after` checks paired complete rounds and reports instruction/time ratios and a
paired bootstrap time interval. Both median ratios must be <=1.05. Use distinct
provider labels for these candidates, retaining `tasks.` prefix for Hirsel launch
flags. Cross-session comparisons are refused by construction.

## Tasks/axum baseline

The default `run` also invokes `server`. `server --baseline-root PATH` accepts a
landed checkout of the same repository; `--hirsel BINARY` adds the free server
participant. Until the baseline lands, it writes a dependency-owned server
result. `--skip-servers` selects a partial micro-only session.

Use the tasks-rust lane's `bench/apps/tasks/rust` application and open-loop `load`
binary once landed; B1 does not fork that application. The baseline is pinned to
tokio 1.52.0, axum 0.8.9 and the repository Rust toolchain, as are B1's tokio
counterparts. Server on CPU31, client on CPU7; the server at width N receives
CPU31 plus N-1 worker cores, with the same grant for both implementations. Its
single-thread comparison remains width 1. `TASKS_WORKERS` integration and the
server multicore launch adapter are coordinated with tasks-rust/C1/W1/F1 and are
listed dependency-owned until available. The existing tasks suite uses ten
seeded interleaved open-loop rounds and consumes full response bodies, including
queueing from scheduled arrival, preventing coordinated omission. Do not replace
it with a closed-loop client or compare saved sessions' wall times.
