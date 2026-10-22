# Tasks runtime/std microbenchmarks

Build each `.hir` with `hirsel build --release ... -o <binary>`, with the
matching toolchain's runtime archives, LLVM and link kit (as in
`bench/micro/build`). The sidecar selects the std source. Build the baseline
against std from the baseline revision; build the candidate against current std.
Use `json_record_dom.hir` for the baseline and `json_record.hir` for the direct
writer. Both print the same wrapping checksum. The other programs use identical
source except `search`, whose baseline expression is
`string.contains(string.casefold(s), string.casefold("benchmark"))`.

Each binary takes N and epochs after
`--hirsel-grants bench/micro/grants.json`. Suggested N is 20,000, epochs 1.
The decimal workload covers the short-integer path repeatedly. Unicode folding,
search, normalization and JSON parsing include real call sites, not substitute
implementations. `json_record` compares a derived DOM/map with a derived writer.

`crates/hirsel-rt/examples/failure_table.rs` calls the real runtime loader without
requesting diagnostic text. Build `//:hirsel-rt-example-failure_table` on both
revisions with `--target-platforms //tools/buck2:release`; arguments are site
count and rounds (10,000 and 100). It prints those inputs as its checksum.

Put old and new commands in a participants JSON as described in
[the micro harness](../micro/README.md), then run:

```sh
bench/micro/run participants.json --reference before --seed 20261020 --output session.json
```

For the five application rows, `tasks-compare` reuses the tasks comparison
lane's server, seeded 10k board, open-loop load generator and counter protocol:

```sh
bench/tasks-perf/tasks-compare --harness bench/apps/tasks \
  --before /tmp/tasks-before --after /tmp/tasks-after --output /tmp/tasks-session
```

It runs ten seeded interleaved rounds of old Hirsel, new Hirsel and the locked
Rust current-thread executable. Servers use CPU31, clients CPU7. The CPU31
measurement lock covers the session, and raw samples retain CPU15 busy share,
all hardware counters and binary hashes. Every cell has a fresh server. Search,
create and update use 1 req/s for one second; cold uses an empty board and
restart uses 10k tasks. The instruction ratio is the verdict; cross-session
wall times are never compared. Missing samples fail the run.

The st-app follow-up adds `contains` (short-tag containment misses), `html`
(mixed UTF-8 and HTML replacements), and `html_plain` (ordinary application
text). Both HTML workloads go through the real formatting wrapper.
`hirsel-rt`'s `idle_pool` example exercises repeated 12k-block bursts separated
by worker collection; it makes idle retention and reclamation costs visible.
Its instruction count includes kernel page fault and deallocation work.

Ten-round st-app comparisons (total instructions per invocation; 20k items for
std workloads, ten 12k-block bursts for idle reclamation). Raw counters, binary
hashes, paired ratios and output checksums are in `results/st-app-*.json`.
Each row compares identical input/output within its own interleaved session.

| Workload | Before | After | After/before |
|---|---:|---:|---:|
| contains | 42,728,834 | 23,936,551 | 0.560 |
| derived JSON keys | 68,726,484 | 57,457,062 | 0.836 |
| HTML mixed | 100,520,120 | 90,402,199 | 0.899 |
| HTML plain | 96,005,890 | 73,948,754 | 0.770 |
| idle slab reclamation | 49,179,322 | 280,852,754 | 5.711 |

Idle reclamation deliberately trades repeated allocation throughput for bounded
retention. User instructions rise 1.137x; most extra instructions are kernel
page reclamation/fault work. The separate ten-round seeded app comparison
reduces idle RSS from 29.17 to 15.94 MiB; restart instructions fall from 376.83M
to 360.05M. This still misses the 2x RSS target against 7.25 MiB Rust.

`slice_copy.hir` retains 10,000 independent 64-byte String copies viewed from
one 1 MiB String. On an unfixed runtime it requires roughly 10 GiB; always
run that baseline under an address-space limit. Run the interpreter benchmark
with `hirsel run bench/tasks-perf/slice_copy.hir --max-steps unbounded`; its
10k workload exceeds the corpus step budget, so the permanent 10k allocation
assertion lives in the runtime integration tests. The fixed runtime requests
80 bytes per interval (64 data bytes plus its 16-byte sequence header).
[The measurements](/workspace/code/hirsel-briefs/tasks-perf/repo-evidence/bench/tasks-perf/results/slice-copy.json) distinguish process RSS from
runtime copy storage: the interpreter includes compiler state and boxed input
construction, and already materializes String/Bytes views when it creates them.

The [restart comparison](/workspace/code/hirsel-briefs/tasks-perf/repo-evidence/bench/tasks-perf/results/slice-copy-restart.json) contains ten seeded,
interleaved rounds of before, after and Rust (30 complete samples). Median
instructions are 319.843M before and 319.959M after (1.00036x); median idle RSS
is 15.441 MiB before and 15.451 MiB after (1.00063x). Both Hirsel binaries use
the same compiler/runtime and differ only in DOM JSON input sharing. Current
tasks uses the direct typed reader, so this is a neutral result.
