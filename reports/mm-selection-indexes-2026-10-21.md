# Render task selections as board positions

The renderer now filters into `List[u64]` and projects rows from the board held
under the existing immutable borrow. `model.select` still returns an independent
`unique List[Item]`; its implementation, all_tags, counts, persistence, response
ownership and the Rust reference are unchanged. No compiler or language change.

Prior art: `bench/apps/tasks/rust/src/model.rs:168–193` filters into `Vec<&Item>`
and stably sorts borrowed rows. Hirsel uses checked ephemeral positions instead
of references. The existing `list.sort` (`stdlib/std@1/list.hir:41–45`) is unstable,
so the indexed comparator breaks equal keys by original position using relational
comparisons, without subtraction/conversion overflow. Manual order does not sort.
The older owning selector's stable-sort comment is inaccurate for sufficiently
large ties; it remains unchanged as requested. Small old/new equivalence cases
and a separate 40-row manual-position oracle cover both obligations.

## Validation

Parent: `781806f7`. Current supported generated target:
`//:hirsel-cli-test-examples-shard-1`, focus filter `tasks_`.

- Package tests increased from 22 to 25: **25 interpreted + 25 LLVM passed**,
  covering model, view, app, httpd and the remaining package modules. Added ID
  and byte comparisons, Unicode/casefold, duplicate tags, all orders, ties,
  absent dates/results/selections, mutations between requests and owned-result
  independence. The larger tie test crosses quicksort partitions.
- Interpreted HTTP e2e passed. Three native importers initially failed before
  emission because the pool has no HOME/XDG_CACHE_HOME for default object caching.
  Retrying only those three with documented `--env
  XDG_CACHE_HOME=/tmp/mm-selection-indexes-cache` passed: LLVM e2e, bounded client
  build, and the 200-stream retained-board regression. **5/5 importer tests pass**;
  the initial setup red remains recorded in `tools/known-failures.tsv`.
- Frozen 10k fixture: **12 responses per participant** equal across before,
  after and unchanged Rust, including every sort, search/missing/detail, update,
  delete, and full-list SSE. The SSE control mutates the board after reading only
  the event header, then verifies that the pending old body and next new body
  retain their own bytes through normal asynchronous transport completion.
- No conformance run: no compiler behavior was changed.

## Measurements

Evidence: `/workspace/tmp/mm-selection-indexes/`; source/binary hashes in
`certificate.json`, source snapshots in `source/{before,after}`, production build
logs/environment, diagnostic source hashes and the exact archived profiler probe
are retained. Baseline app sources are exported from the parent revision. Both
production binaries use the same frozen compiler/runtime, regions enabled,
`--release`, and fresh object emission (`HIRSEL_OBJECTS=none@1`,
`HIRSEL_STD_OBJECTS=none`). No census rerun: only candidate list/search/SSE cells
were recreated, compared to the archived original operation records under
`/workspace/tmp/mm-measure/captures/`.

The first cached candidate binary aborted in `channel_open` before rendering;
its initial paired attempt is invalid and preserved as `paired-invalid-cached`,
with a GDB backtrace. An initial equivalence harness assertion incorrectly expected
DELETE status 200 instead of 204; that setup red is preserved separately. Re-emitting the identical source without cached objects
serves correctly. Both participants were then emitted fresh for the valid run.
This cached-object failure is separate from selection and remains reported
upstream; the specific cache defect has not been diagnosed here. Compiler/runtime/stdlib inputs were unchanged throughout the initial
Buck builds; app/test-only edits during those builds are explicitly qualified in
`toolchain-build-inputs.json`. Production measurement sources were frozen before
emission; later formatting changes only whitespace, with final hashes retained.

| Full-list operation | Archived owning selection | Indexed rendering |
|---|---:|---:|
| Nested Item-copy allocations | 50,000 | **0** |
| Item-copy capacity bytes | 2,560,000 | **0** |
| Selection growth allocations | 13 | **12** |
| Selection growth capacity traffic | 3,063,728 | **162,880** |
| Final selection capacity | 1,531,920 | **81,424** |
| Comparator invocations (manual order) | 0 | 0 |
| Whole-request allocations | 70,137 | 20,132 |
| Whole-request capacity traffic | 52,370,448 | 46,909,376 |
| Settled logical live bytes before/after | 4,131,264 | **4,131,264** |
| Logical peak live bytes | 54,313,024 | **50,302,432** |

Selection itself eliminates 50,001 allocation events net; it still needs index
storage. Whole-request totals include other sites and are not all credited to
this representation change. Capacity traffic is neither RSS nor copied bytes.
Search selects one row, so its comparator also performs zero comparisons.

Rendering retains the borrowed board graph, one transient index buffer, and the
output builder. Row projection creates no Item owners. Index storage is released
before HTTP/SSE sending; the response remains independently owned until sending
completes. Diagnostic settled backing is 6,546,336 bytes (4,980,736 slab,
1,565,600 large), including 1,048,576 idle slab bytes. While the completed response
is pending, observed backing is 37,337,720 bytes and logical live storage
34,858,000 bytes; this is an idle-boundary observation, not exact peak backing.
The unchanged retained service graph has 50,029 logical blocks. Regions reserve
zero bytes in these captures. No reduction in independent board ownership or
blanket RC saving is claimed; drop sites belong to later operations.

The full-list response is **16,739,685 bytes**, SHA-256
`5cd11ff929157ca6da64090936e57baa76c2e41f58dceb3a4b1b0ca6950257b9`,
identical to the archived response. Search's 4,643-byte response also matches.

Ten seeded interleaved rounds (`20261021`), `nice -n 10`, server CPU31, open-loop
client CPU7, existing CPU7 then CPU31 timing locks; `perf stat` cycles and
instructions, one completed request per measured cell. Raw samples, perf files,
paired round ratios and bootstrap intervals are in `paired/samples.json`.
The `paired_ratio` field is the ratio of the two within-session group medians
(`bench.ratio`); ten individual paired-round ratios are retained separately.
Intervals use paired-round resampling and describe within-session jitter.

| Production cell | Before median instructions | After median instructions | After/before | 95% paired interval |
|---|---:|---:|---:|---:|
| Full list | 241,023,074 | 198,585,480 | **0.82393** | 0.81684–0.82772 |
| Search control | 25,910,942.5 | 25,943,715.5 | **1.00126** | 1.00013–1.00287 |

Full-list median cycles are 177,182,435.5 → 151,754,081. Production binary size is
9,079,752 → 9,070,072 bytes (−9,680); ELF text is 7,490,495 → 7,478,495 bytes.
The instruction gate passes with **17.6% fewer full-list instructions**. Search
has a small 0.13% instruction overhead, with identical filtering/output. No
latency speedup is inferred from allocation counts or from different sessions.
