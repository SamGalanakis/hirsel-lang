# Service reclamation: measured NO-GO

The targeted libc-reuse policy is declined. It preserves the RSS reduction but
still costs 11.9% more create instructions, 34.9% more list instructions, 11.0%
more update instructions and 4.0% more SSE1 instructions than the matched
pre-RSS allocator. It does not establish a material request improvement over
the landed direct-mapping policy. The required maximum 2% regression is **not
met**. Production reclamation remains unchanged; this change adds regression
coverage around its threshold and alignment boundary.

## Frozen comparison

The compiler, std and tasks application are from
`0a0d9aed835ce1808f18342a139835935187df05` (the full identity is also in the
retained input manifest). The application is emitted once as three objects;
all Hirsel participants link those identical objects and the same link kit.
Only the runtime archive varies:

- Pre-RSS: current runtime with the `a5a14ba9` direct-mapping policy and its new
  host-wait idle-cap enforcement disabled. All other current runtime changes
  remain present.
- Landed RSS: unmodified runtime at the frozen source revision.
- Trial: libc allocation/free plus `MADV_DONTNEED` on whole interior pages of
  freed large allocations, retaining allocator link words and partial boundary
  pages. It keeps the existing 128 KiB/alignment selection and idle-cap guard.
- Rust: unchanged one-worker service, rebuilt with its locked dependencies and
  `rustc 1.99.0 (b940084d7 2026-09-28)` alongside the open-loop client and launcher.

Production executable SHA256:

| Participant | SHA256 |
|---|---|
| Pre-RSS | `4fea55a48f0165ccf8802ba6ad6709bb4884e2349eb6b1e52e0d3cb7d030a29f` |
| Landed RSS | `df633eed101424a3bf2480241ea96d7c638adab59d8503419ad1975af5ad7e47` |
| Declined trial | `aac299c1738be2b47393e19c0c29ef2c83c0a4ab428da5153067bdd436aa3460` |
| Rust | `281d633929ca926900757720b330e60b33b293cb0e4d687f1506e19e5dc0a481` |

Session `2026-10-21T19:09:39Z`: ten fresh-process rounds, seed 20261021,
seeded operation/participant interleaving, server CPU31/client CPU7, locks in
ascending CPU order. Create/list/update offer 16 requests/s for three seconds;
SSE has one subscription. All **200 samples** complete with zero failed cells.
Each participant completes 480 requests per mutation/list cell, ten SSE patches
and ten cold lifetimes. Cold counters include launch, readiness and shutdown;
load cells count server request work, including persistence and descendants,
between the client's synchronized start and completion. Client validation and
server cleanup remain outside load counters, as in the existing comparator.

Ratios are ratios of within-session medians, normalized by actual completed
requests. Intervals use paired-round bootstrap (95%, 5000 draws). They describe
session jitter, not host drift. The SMT sibling was busy (median shares roughly
25–84% by cell); cycle/time ratios are reported without a capacity/parity claim.

| Cell | Landed / pre total instructions | Trial / pre total instructions [95% CI] | Trial / pre user instructions | Trial / pre cycles |
|---|---:|---:|---:|---:|
| Cold | 0.963 | 0.961 [0.772, 1.004] | 1.000 | 0.934 |
| Create | 1.128 | 1.119 [1.112, 1.225] | 1.000 | 1.508 |
| List | 1.368 | 1.349 [1.311, 1.442] | 1.000 | 1.985 |
| Update | 1.110 | 1.110 [1.066, 1.151] | 1.000 | 1.318 |
| SSE1 | 1.051 | 1.040 [1.036, 1.047] | 1.000 | 1.064 |

Trial / landed total instructions are 0.992 create [0.974, 1.054], 0.986 list
[0.968, 1.050], 1.000 update [0.993, 1.023], and 0.990 SSE1 [0.987, 0.997].
The small SSE improvement does not repair the request regression.

| Cell | Pre / landed / trial completed req/s | Pre / landed / trial / Rust load RSS (MiB) |
|---|---|---|
| Create | 15.995 / 15.993 / 15.993 | 22.68 / 18.80 / 19.02 / 11.59 |
| List | 15.993 / 15.990 / 15.968 | 49.99 / 17.87 / 17.87 / 39.53 |
| Update | 15.996 / 15.994 / 15.992 | 22.88 / 18.67 / 18.64 / 11.62 |
| SSE1 | 96.605 / 89.289 / 99.481 | 21.50 / 17.53 / 17.45 / 11.22 |

SSE rate is the reciprocal of one patch's latency, not sustained capacity.
Meeting the offered mutation/list rate likewise does not establish capacity.
Rust's list load RSS is a separate post-load snapshot; it must not replace its
closed-single-request RSS below.

## Reclamation and attribution

CPU30 diagnostics preserve smaps and smaps_rollup at each boundary. A seeded
readiness request, three lists and one create finish at 50.95 MiB with pre-RSS,
17.43 MiB with landed RSS and 17.59 MiB with the trial. The second full list
exposes the pre-RSS allocator-history retention. Disabling direct mapping while
keeping idle slab enforcement ends at 50.57 MiB: the slab guard alone does not
preserve the win.

Across that diagnostic sequence, landed RSS makes 423 mmap, 47 munmap and 140
madvise calls; pre-RSS makes 410/34/140; the trial makes 410/34/158. Thus it
removes the extra map/unmap calls, while total instruction overhead remains.
Unchanged user counts and changed kernel counts support allocation/reclamation
cost as the cause; syscall counts alone do not decompose that cost. Traces
include temporary render mappings of about 15.36 and 30.73 million bytes.
Keeping these warm would exceed the existing 1 MiB idle slab budget; this lane
does not increase it or change libc's global thresholds.

Prior art is glibc `04e750e75b73957cf1c791535a3f4319534a52fc`,
`malloc/malloc.c:2254-2285,4395-4412` (the landed mmap/munmap mechanism) and
`:4772-4810` (free-chunk whole-page purge). The trial applies the latter only to
runtime-owned backing before System free, rather than traversing libc bins.
The only retained implementation is the regression test; no libc policy, heap
ABI, live-object representation or service lifecycle change is adopted.

The trial's separate measurement companion passes all four existing 1 MiB
idle-slab checks. Seeded readiness/list both retain 50,029 live blocks and
4,131,264 logical live bytes, with zero region backing. After create it has
50,032 live blocks/4,131,328 bytes. Host arena reserved bytes are not RSS:
after list glibc reports 15,654,912 arena bytes, 5,091,392 allocated and
10,563,520 free. After create those are 18,391,040/5,092,928/13,298,112.
The companion/preload is diagnostic only and contributes no timing samples.

A separate production snapshot of the landed policy reports cold/readiness/
closed-list/create RSS 8.62/15.94/15.98/17.30 MiB versus Rust
3.88/7.29/8.19/11.64. Cold executable text is 5.18 versus 1.11 MiB: report this
file-backed residual separately from allocator retention. The cold ratio is
2.22, closed-list 1.95 and create 1.49; no Rust RSS parity gate passes here.

## Reproduction and validation

Retained evidence is under `hirsel-briefs/rss-throughput-2`: `candidate/session.json`,
`summary-final.json`, binary/archive/object hashes, source manifests, exact
link commands, control/trial patches, raw perf CSV, smaps and allocator snapshots.
`session.py` uses the existing tasks benchmark's sampler and comparator without
changing their semantics. `check-instructions.py` exits 1 on the trial, retaining
every slower cell.

The minimal remaining trigger is the benchmark's 10,000-task fixture followed
by a single `POST /tasks` (or `POST /tasks/1`); full-list retention additionally
needs two `GET /` requests to exercise libc's allocator history. Repeat with
frozen paired executables using the existing benchmark:

```sh
bench/apps/tasks/bench --skip-build --samples-only \
  --binary /absolute/candidate --baseline-binary /absolute/pre-rss \
  --kinds hirsel,rust-one-worker --only cold,create,list,update,sse \
  --rates 16 --seconds 3 --rounds 10 --seed 20261021
```

The existing CLI also measures SSE100/1000; the retained lane session selects
SSE1 explicitly. No result for restart/search or other fan-outs is inferred.
The new allocator test executes eight bursts over four extents around 128 KiB
and five alignments (8 through 8192), collects between bursts and preserves a
live 512 KiB neighbor. It adds one test; the unchanged mincore and idle-cap
validators remain independent.

Earlier failures remain in the ledger: historical reproduction completed 149
samples plus one failed update request; the initial current-toolchain session
completed 149 plus an SSE mutation500 and output JSONDecodeError during shared
ENOSPC. Neither is reported as a clean all-cell result. An initial purge trial
failed the existing mincore test (one extra interior resident page), then passed
after correcting page rounding. Native setup attempts include a mixed-source
upload-size failure, CLI argument failures and a missing materialized link kit;
those are zero-test evidence, not passes.

Scoped native validation executes three successful controls under smoke's four
configurations/replay/Cranelift/runner checks and under the Cranelift comparison:
`MEM-11-nested-root-release-run`, `MEM-7-soa-reserve-refs-run`, and
`MEM-12-flat-bytes-field-run`. The fourth selected case,
`MEM-6-exhausted-callback-cleanup-run`, fails before native execution with the
known `PROV-4` checker diagnostic, already recorded in `tools/known-failures.tsv`.
It is not excluded or weakened by this change. The service verifier passes 23
checks. Runtime unit tests pass 309 with 1 ignored; descriptors pass 12.

Final allocator integration selection passes 32 tests (31 before), including
the new threshold burst control and the unchanged residency/idle-cap cases.
The separate release selection passes 29 tests. Formatting and diff whitespace
checks pass. The production allocator source is
byte-identical to the frozen main input after declining the trial.
