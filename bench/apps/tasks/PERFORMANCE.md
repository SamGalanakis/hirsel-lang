# Tasks performance follow-up

The tables and causal narrative below describe the historical 2026-10-20
session. The [2026-10-22 frozen scoreboard](RESULTS-app-perf-refresh-2.md)
reports current service, recording, RSS and compiler costs together, including
the recorded-list 4 GiB failure. Its ratios must not be combined with this table.

The separately landed ten-round refresh is commit `6d4e23ac`. The final
[results](RESULTS.md) compare the same candidate binary with recording disabled
and with `--hirsel-record` at its default level, against Rust's one-worker
runtime. Every operation has ten fresh processes; the bounded follow-up offers
16 requests/s and measures SSE fan-out at 1, 100 and 1000 subscribers.
Counters are normalized by completed requests before comparing participants.
The first refresh retains the wider capacity sweep in its commit.

## Final verdict

| Operation | Plain / Rust one-worker instructions | Recorded / plain instructions |
|---|---:|---:|
| Cold | 1.323 | 8.199 |
| Restart | 3.742 | 6.645 |
| Create | 0.981 | 19.298 |
| Update | 0.978 | 19.352 |
| List | 2.274 | 22.200 |
| Search | 1.062 | 1.172 |
| SSE 1 / 100 / 1000 | 0.928 / 1.063 / 1.067 | 13.957 / 1.565 / 1.135 |

Restart and list miss the 1.5x instruction goal. Seeded idle RSS is about
16.0 MiB against 7.2 MiB (2.2x), missing the 2x goal. Cold RSS meets it.
The recording baseline's plain create/update/list/search instruction ratios
were 1.546/1.544/2.711/2.191. The final rows improve each; restart improves from
3.907x. This comparison uses instructions only across sessions; wall times and
cycles are compared only within a session. The JSON files retain hashes and
rounds rather than attributing concurrent-main changes to this lane.

## Changes and evidence

Generic std changes short-circuit impossible substring matches, prequote known
JSON field/variant names in the derive macro, and append unchanged HTML spans.
The HTML formatting wrapper shares immutable input instead of deep-copying it;
its return region now follows its argument and it is `@noalloc`. This changes
retained-wrapper lifetime behavior; callers needing owned escaped text can use
`html.escape`. No application implementation was changed.

[Micro results](../../tasks-perf/README.md) retain ten-round before/after
instructions and matching output checksums. Reference implementations are Rust
`core/src/str/pattern.rs:989-1017`, Go `strings/replace.go:550-579` and
`encoding/json/encode.go:733-739,1180-1183`, pinned in
[NOTICES](../../../third-party/NOTICES.md). Hirsel emits prequoted names as
source string literals because derive expands Hirsel source; runtime field
values still use the regular JSON writer. HTML keeps Hirsel's existing five
replacement spellings and byte-preserving UTF-8 behavior.

Worker collection now trims retired slabs even without a region exit. The
heap idle default is 1 MiB, separately from the existing region-pool cap. On
Linux, retired 64 KiB slab pages receive `MADV_DONTNEED` before libc free, since
libc may retain the aligned allocation. Live slabs are never purged. This ports
mimalloc's collection/decommit structure (`theap.c:124-147`,
`prim/unix/prim.c:549-558`); Hirsel uses its existing slab retirement and libc
backing rather than mimalloc's allocator. Other platforms retain their existing
release behavior. The aggressive burst micro's kernel cost is reported beside
the app's RSS reduction, rather than presented as an instruction improvement.

## Remaining causes and minimal triggers

- Restart: start over `fixture(data, 10000)` in `bench`. `store.decode` parses a
  generic JSON DOM, allocates/hashes object keys (including duplicate checks),
  then derives a typed Board and destroys the DOM. Rust's serde visitor builds
  typed items directly. Profiles show `json_node`, `json_quoted`,
  `json_owned_text`, hash/map insertion and destruction. A generic typed
  streaming decoder would remove the intermediate representation.
- List: `GET /` over the same fixture. Rendering builds many temporary f-string
  fragments, validates finished UTF-8 builders, and copies the final HTML at
  `html_reply`'s explicit unique Bytes conversion. Profiles retain builder,
  `seq_grow`, copy and rendering symbols. The immutable HTML wrapper removes
  one copy source; general builder fusion/borrowed conversion remains open.
- Idle RSS: start over that fixture, then `GET /?q=Task+9999` and leave idle.
  Ordinary private heap dominates the resident mappings. Reclaimed slab pages
  reduce retention substantially, but live typed board storage, loaded runtime
  metadata, partially occupied slabs and libc-retained non-slab allocations
  remain. Heap profiles mix startup, list and 100 subscriptions and cannot
  provide an exact additive decomposition of the initial idle RSS.

## Recording-off scaffolding

Plain execution still calls `wants_issuer`/`Journaled::durable` at effect
boundaries (`host_waits.rs`, `io_real.rs:539-559`), goes through
`Journaled::before` and no-op journal dispatch (`journal.rs:835-862,913-930`),
and evaluates `capture_views` (`io_real.rs:759`). The general performer path
also retains owned completion handling. These checks are avoidable costs of a
run being recordable even with no tape destination. This lane reports them;
the concurrent I/O lane owns socket protocol and request-copy removal. The
plain mode avoids tape/header serialization and durable issuer bookkeeping.
Flat journal-symbol samples undercount checks inlined into ordinary host
functions, so their absence does not establish zero cost.

Recorded execution additionally formats and flushes every event before
completion delivery (`launch.rs:2241-2283`). Hex rendering currently formats
each byte independently (`hirsel-run-schema/src/lib.rs:399-400`); recorded
profiles identify formatting/hex/tape symbols. Recorded/plain ratios include
that default recording policy, and are distinct from plain/Rust parity.

## Reference counts and copy-on-write

The generated report separates increment slow paths, decrement/dispatch,
last-owner destruction, COW detach entries and explicit graph copies for every
profile and recording mode. These are disjoint flat self-symbol buckets, not
whole-program percentages. Finite count increments/decrements are emitted
inline (`hirsel-llvm/src/runtime.rs:394-425`) and remain charged to their
containing native functions. Exact attribution of those instructions needs
instruction tags or separately calibrated instrumentation and remains open.
Zero detach samples do not prove zero detaches. `seq_grow` mixes ordinary
capacity growth and possible detachment, so it is not wholly charged to COW.
Earlier independent-board retention was already removed on main; remaining
`copy_graph` samples include explicit unique conversions in rendering/selection
and must not be confused with COW or the old per-subscriber board retention.

## Validation

The final source passes 67 runtime unit tests and 295 integration tests (one
ignored), the standalone 117-check HTTP contract, tasks interpreted/native E2E,
the stream-retention test, tasks' 22 package tests in both backends, Datastar's
7 package tests in both backends and its API-reference check. The selected
substring/derive/JSON/HTML corpus run initially passed 38 of 39; the struct
variant fixture needed an explicit expected type. Its corrected JSON case and
the final HTML borrowed-wrapper case each pass the selected corpus runner.
Four benchmark-report tests pass.

The separate compiler-memory assertion for building the E2E client fails at
1537 and 1540 MiB against a strict 1536 MiB bound. The generated client and
functional E2E tests pass. A control using
the same frozen release compiler and matching original/final std prechecks
built successfully at 1532.7/1530.8 MiB, respectively (raw
[control](../../tasks-perf/results/st-app-compiler-memory.json)). It does not
reproduce a std-caused increase, but uses a different compiler build from the
debug test. The debug assertion remains unresolved; it is not classified as
unrelated or bypassed.

## Reclamation follow-up

The [matched allocator follow-up](../../../reports/rss-throughput-2.md) retains
the RSS savings but declines targeted libc page purging: create/list/update
still cost 11.9%/34.9%/11.0% more total instructions than the pre-RSS control.
The ten-round session completes 200 samples without failed cells. The maximum
2% instruction-regression goal remains NO-GO; production policy is unchanged.
