# Single-thread network cost

This is a historical report. New measurement campaigns, raw sessions and
reproduction scripts belong in `/workspace/code/hirsel-briefs/<lane>/`.
Keep only a short result row or archive link in an existing results page.

The plain `net-only.hir` loop meets the target: **1.926× Rust user
instructions**, down from 17.239× in the same interleaved session. Total
instructions are 1.017× Rust. The actual runtime ABI regression test performs
1024 ready read/write pairs with **zero allocations** after setup.

Recording remains expensive: **175.394× Rust user instructions**, or 3.457×
total instructions. Plain calls now borrow live buffers and bypass owned
`Request`/`Completion` construction, payload clones, and absent-journal dispatch.
Recorded calls retain owned bytes for replay.

## Per-change measurements

All rows below were measured together, ten seeded interleaved rounds on
2026-10-20, server CPU 31, open-loop client CPU 7, 1024 requests at 2048/s.
Seed: 20261020; session: `d7ede339-dae7-4cb1-95b7-b40c387a03f6`.
Each entry is the harness's instruction ratio against the same Rust participant.
All 210 samples completed and produced the expected `1024` checksum.

| Cumulative change / participant | Plain user | Recorded user | Plain total | Recorded total |
| --- | ---: | ---: | ---: | ---: |
| Before | 17.239 | 178.908 | 1.269 | 3.521 |
| Borrow read/write buffers; reuse existing capacity | 10.083 | 176.419 | 1.161 | 3.472 |
| Skip idle root scheduler polls and plain wait journal events | 8.827 | 177.178 | 1.153 | 3.490 |
| Wait inline for an exclusive sole-root socket | 4.891 | 175.525 | 1.032 | 3.458 |
| Share capability/stream validation | 4.085 | 177.087 | 1.058 | 3.475 |
| Compact live handles/results; defer owned protocol | 3.266 | 175.079 | 1.050 | 3.467 |
| Syscall-first exclusive provider, without pending tables | 2.593 | 175.509 | 0.995 | 3.465 |
| Cache validated grants until the next generic authority operation | 2.117 | 175.842 | 1.029 | 3.465 |
| Inline fixed read wrapper; expose write-only storage | 2.088 | 176.101 | 1.035 | 3.484 |
| Reuse sole-root proof; bypass absent-journal dispatch | **1.926** | **175.394** | **1.017** | **3.457** |

The write-only declaration by itself was neutral in a separate experiment.
Inlining helped, but generated code still contains buffer initialization:
this result does not claim that all `memset` work disappeared. The source keeps
the 4096-byte initializer inside the loop and preserves the unwritten suffix.

Heaptrack measured 12,078 allocation calls before, 3,933 after buffer borrowing,
and 218 after the final changes, each over 1024 requests including startup.
The last run has 96.31 KB peak tracked allocations and 59 temporary allocations.
The 218 calls are fixed setup/teardown costs, not a claim of zero allocations
for the whole process. The ABI test isolates steady-state read/write and counts
zero. Direct reads initialize only the received prefix; unique ordinary lists
reuse sufficient existing spare capacity. Shared/tracked values use the normal
protocol, and validation precedes any buffer growth.

## Tasks create and list

A separate single interleaved session measured before, after, and Rust together:
ten rounds, seed 20261020, CPU 31/7, one open-loop request/s for one second per
cell, with 10,000 seeded tasks. Ratios use medians; every cell completed without
an application error.

| Operation | Before user / Rust | After user / Rust | Before total / Rust | After total / Rust |
| --- | ---: | ---: | ---: | ---: |
| Create | 1.633 | 1.672 | 1.471 | 1.503 |
| List | 2.550 | 2.541 | 2.717 | 2.422 |

Create regressed 2.4% in user instructions (67.57M to 69.21M); list user
instructions improved only 0.35%, while total instructions improved 10.8%.
One large before/create outlier remains in the raw samples; no sample was
excluded. These are frozen application binaries, before the later st-app
application changes. The after runtime also incorporates intervening main
changes, so the application deltas cannot be attributed entirely to socket work.

List remains above 2×. Minimal trigger: `GET /` against the tasks server seeded
with 10,000 items. Its user-instruction profile is dominated by allocation and
immutable board copying plus HTML/text building: `alloc::grow` 10.45%,
`copy_graph` 5.70%, runtime allocation 5.37%, `html.escape_into` 3.43%,
`unique_copy_value` 3.01%, `text.append` 2.93%, `clone_block` 2.20%, and
`decimal.append` 1.94%. That profile had about 26,000 samples, 1,746 lost, and
limited unwind accuracy; percentages identify causes rather than exact totals.
The st-app lane owns the application/RC/COW follow-up.

Recorded net also remains above 2×. Minimal trigger: the same `net-only.hir`
with `--record` (native spelling `--hirsel-record PATH`). Its profile shows
formatting/hex conversion under `run_schema::hex -> launch::render_tape ->
TapeJournal::event`, plus payload encoding, hashing and journal allocations.
Formatting and hexadecimal formatting account for 8.46% and 7.43% of samples.
Tape rendering is the remaining cause; recording-off was the performance target.

## Reproduction and evidence

Use the commands in [README.md](README.md) with frozen binaries for every
cumulative stage. `compare --record` adds a recorded participant for each stage;
it checks output checksums and shuts down the finite client cooperatively on
failure. `bench/tasks-perf/tasks-compare --operations create,list` measures the
application comparison. Instruction counts are the verdict; cycles and total
instructions are retained in the raw data.

The net source is frozen from `http-serial-path/net-only.hir` and committed here.
The independent Rust reference is `bench/micro/http/server.rs`: Rust recognizes
a fixed HTTP head while Hirsel counts its fixed 35-byte length. Response headers
differ, but both send equal application bodies. This measures the socket floor;
it includes no Hirsel parser, connection transfer, or application work.

Raw evidence is retained outside the repository at
`/workspace/code/hirsel-briefs/st-io/`:

- `verdict-counts/counts.json`: all stage samples, binary hashes, cycles and ratios.
  Final `net-noidle` uses matching compiler/runtime ABI 29.
- `alloc-before/`, `alloc-buffers/`, `alloc-noidle/`: heaptrack captures and summaries.
- `tasks-verdict/samples.json`: all application samples and ratios. Binary SHA256:
  before `5c971c8e9d04947a42b89b206d42bf3c2ad12e2753df996559c0738e59e8ad1e`,
  after `720e73f194120ec19549b4f2e932e02edd2f84be3264fc0fb9414e62f38c2e73`,
  Rust `216d01c9b3efca7acc7ff2e628f2ffa9edb291997809ad33c91432c5be8288b2`.
- `tasks-profile-final2/` and `profile-record/`: remaining-cost profiles.

Earlier `live-counts` (mismatched compiler/runtime ABI) and `matched-counts`
(client lifecycle interference) are invalid and excluded from this report.

## Prior art and semantic constraints

The implementation follows these cloned upstream revisions:

- Rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
  `library/std/src/sys/net/connection/socket/unix.rs:285-303,320-322`:
  receive into uninitialized spare capacity and advance only the written prefix;
  `:180-229`: syscall-first readiness, deadline accounting and interrupted waits.
- Tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
  `tokio/src/runtime/io/registration.rs:225-247`: retry after WouldBlock;
  `tokio/src/runtime/scheduler/current_thread/mod.rs:818-879`: root/driver scheduling.
- mio `a88e5e119172e4fb1638926faf65a4fe889e05b1`,
  `src/sys/unix/selector/epoll.rs:129-145`: edge-triggered readiness interests.

Deviations follow Hirsel's semantics: stackful fibers replace futures;
capability, cancellation and grant checks remain; recording retains owned
buffers; shared/tagged readiness, virtual clocks, pending peers, scope deadlines,
external wake sources, TLS and blocking-pool callbacks use the normal scheduler.
Inline polling is confined to an exclusive sole native root. Generic authority
operations invalidate the cached grants, including close. Free-mode yield events
and diagnostic root points remain observable. `single_worker_fast_path` is the
shared seam for the multicore lane to disable shortcuts for wide/migration runs;
registered stop/wake descriptors already decline inline socket polling.
There is no new exported runtime ABI or TIR/LIR construct.

## Validation

The final merge validation passed 83 runtime unit tests and 303 runtime
integration tests (one ignored). The LLVM suite passed 32 (13 ignored).
This lane adds 12 runtime unit regressions, plus a corpus regression for the
unwritten read suffix; its agreement run passed one original and three variants.

The HTTP campaign validated all 63 selected unique cases after fixing missing
twin metadata and rerunning that pair. HTTP agreement passed 41 originals and
86 variants. All eight focused application cases passed: tasks package tests,
interpreted and LLVM end-to-end, bounded-memory LLVM client build, event streams,
httpget, fetchjson connection reuse, and real Unix HTTP record/replay.
Infrastructure-disconnected runs were retried; the interpreted tasks retry
actually ran and passed its test.

The stdlib HTTP suite passed 24 and retained one tracked failure:
`stdlib_http::http2_adapter_scripts_record_and_replay` expects `net.connect`
for a peer-goaway script that now listens. It is already listed in
`tools/known-failures.tsv` under std-web-2 and is outside these socket changes.
