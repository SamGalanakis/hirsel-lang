# Borrowed serializer intervals

A serializer's checked `bytes.slice` result can borrow its stable String owner's
existing loan until one synchronous `bytes.append` returns. The runtime slice
producer previously incremented its backing block; the caller released that
owning temporary on its normal or failure cleanup path. There was no leak at a
source line with no attributed release.

The importer proves the entire temporary move graph, exactly one borrowed append
consumer, a stable `string.as_bytes` source, and an existing source loan. Raw
Bytes, retained, stored, taken and two-mutable-argument consumers keep the owning
producer. The new internal `SliceView` checks `start <= end <= length`, preserves
the source owner, origin and loan, and acquires no credit. C checks its type,
loan, backing and failure rule independently; H executes the interval check and
audits every injected failure. Both native backends emit the same checked view.
No public signature, source ownership mode or runtime ABI changes. The internal
LIR format becomes version 48 and its validator version 14; existing
instruction snapshot tags are retained and SliceView is appended.

Generated JSON serialization also copied its borrowed record's `List[String]`
tag field for iteration. A separate stable-field proof resolves that temporary
back to the borrowed parameter. It permits borrowed length/read operations and
synchronous byte writers with one mutable `List[u8]`, primitive borrowed peers,
no taken inputs and no owning result. An explicit `Share` remains at the former
copy point so bounded arena sharing and charge order stay exact.

The escaping algorithms and vectors are unchanged. Prior art is serde_json
`20572a6d`, `src/ser.rs:2079–2133`, and Goose `8511ae34`,
`stdlib/json.goose:11–58`: retain borrowed text and emit unescaped runs to a
mutable writer. Hirsel keeps checked bounds and its own ownership/UTF-8 rules;
it imports neither Rust unsafe conversions nor Goose storage assumptions.

## Recount

Only create, update and full-list operations were recreated from the frozen
10,000-task census fixture. Settled before/after snapshots are subtracted at
every source site, then each increment and decrement is reconciled separately
against the operation totals. The baseline reproduces the archived totals.
Diagnostic counters and production instruction samples use separate binaries.

| Operation | Finite increments before → after | Finite decrements before → after | Removed + / − |
|---|---:|---:|---:|
| Create | 50,137 → 133 | 50,222 → 218 | 50,004 / 50,004 |
| Update | 50,036 → 37 | 50,131 → 132 | 49,999 / 49,999 |
| Full list | 110,101 → 10,037 | 180,198 → 80,134 | 100,064 / 100,064 |

The exact targeted source events have zero residual finite credits:

| Source scope and site | Operation | Removed increments | Removed decrements |
|---|---|---:|---:|
| `json.write_text_into`, line 410 Call / Release | Create | 40,003 | 40,003 |
| Generated `item_json_into`, schema 1 Copy / 28 Release | Create | 10,001 | 10,001 |
| `json.write_text_into`, line 410 Call / Release | Update | 39,999 | 39,999 |
| Generated `item_json_into`, schema 1 Copy / 28 Release | Update | 10,000 | 10,000 |
| `html.escape_into`, line 55 Call / Release | Full list | 70,023 | 70,023 |
| `view.js_string_into`, line 103 Call / Release | Full list | 20,000 | 20,000 |

The list total also includes 40 pairs at `html.escape_into:48`, one at
`json.write_text_into:410`, and 10,000 copied-tag-field pairs in
`view.edit_form:152` Copy / `183` Release. Generated schema positions are not
physical lines in `model.hir`. Uncounted attempts are separate: create stays
9 + / 260,146 −, update stays 9 + / 260,127 −; list falls from
70,045 + / 810,390 − to 8 + / 740,353 −. Every operation still performs four
independent Shared-root atomics.

Allocation/free counts, allocated/copied bytes and COW detach counts are identical.
There are zero explicit arena allocations and zero region reservations in both
variants. Storage is unchanged:

| Operation | Allocations | Allocated capacity bytes | Settled live bytes | Peak live bytes |
|---|---:|---:|---:|---:|
| Create | 139 | 5,220,128 | 4,131,472 | 9,621,824 |
| Update | 139 | 5,219,648 | 4,131,216 | 9,621,824 |
| Full list | 70,137 | 52,370,448 | 4,131,264 | 54,313,024 |

All response bytes and persisted snapshot hashes match. Recording a create
operation saves revision 2 and writes the response; each variant replays its own
tape with matching output and exit status 0. Diagnostic executable size falls
from 13,187,680 to 13,180,680 bytes; production size falls from 10,518,928 to
10,516,248 bytes. These are count-traffic changes, not allocation/RSS savings.

## Production comparison

| Operation | Instructions/request before → after (median) | Median paired instruction ratio [95% CI] | Median paired cycles ratio [95% CI] |
|---|---:|---:|---:|
| Create | 60,655,540 → 57,371,916 | 0.9458 [0.9445, 0.9466] | 0.8732 [0.8500, 0.9904] |
| Update | 60,682,194 → 57,425,008 | 0.9462 [0.9459, 1.5428] | 0.9770 [0.9001, 1.7147] |
| List | 241,518,443 → 234,061,054 | 0.9637 [0.9588, 0.9695] | 0.9745 [0.8753, 1.0697] |

All 60 samples completed three requests with zero errors. All three paired
instruction medians decrease. Update has two high-cost candidate rounds and
create one high-cost baseline round; retain them in the reported confidence
intervals. Update's interval crosses 1.0, so this session does not establish a
statistically clear update improvement. Cycles are noisy and supply no general
latency claim. Idle/load RSS medians remain approximately 16/18 MiB for writes
and 16/16 MiB for list; no RSS improvement is claimed.

Production binary SHA-256 certificates:

- Before: `e0ac9fdfd829569b6bbc6da914fa114994c05ec7279f7cdd593d61001744ead3`.
- After: `e4e0cb570b8c0d42cd8c15880d8a19d3b0edef51128c07a896c79cd725e15d4e`.

Ten seeded interleaved rounds, seed 20261021, unchanged production source, LLVM
Speed, whole-program partitioning, regions enabled, `nice -n 10`, server CPU31,
open-loop client CPU7, 1 request/second for 3 seconds per sample, under the
existing CPU timing locks. Instructions and cycles come from `perf stat` and are
normalized by completed requests. Ratios pair the candidate with the baseline
within each round; confidence intervals resample those pairs. The intervals
reflect within-session jitter, not cross-host drift.

## Validation and conservative limits

Ten focused encoding tests passed, plus 46 RC import/root/view regressions,
14 LIR schema tests, three JSON importer tests, three tasks integration tests
and 11/11 selected conformance programs. The tasks package test executes 22
checks each through the interpreter and LLVM; both HTTP end-to-end variants
exercise first run, shutdown and restart. Setup failures (argument forwarding,
missing/stale materializations, disk exhaustion and a missing pool cache
directory) are excluded from executed counts. Only failed controls were retried.
The source-field compatibility control was already green before this change;
the missed generated-tag Copy and measured site deltas establish that part's
optimization witness. The production comparison initially reached zero samples
with a stale single-slot harness; the complete session uses the branch's bounded
concurrency harness with the same CPU locks and frozen client/server sources.

The narrow fixture checks one SliceView and no temporary slice Copy/Dup/Release
on both supported targets. Alias controls compare exact output after append to
copied backing storage, retained results and two mutable arguments; a nested
Shared root keeps its cleanup anchor. Invalid bounds fail with SEM-3 before the
append effect. LLVM and Cranelift agree with the interpreter; H audits injected
failure cleanup. Forged owned/plain holdings, a wrong failure rule and an expired
source loan are rejected by C. A generated serializer keeps the original bounded
arena threshold: 40 bytes fits and 39 returns OutOfSpace. Explicit take/mut and
noalloc acceptance/rejection twins retain their source diagnostics.

Unproved call shapes stay counted. Minimal examples are
`fn keep(data: Bytes) -> Bytes in data { bytes.slice(data, 0, bytes.len(data)) }`
and a `sink(bytes.slice(data, 0, bytes.len(data)), mut a, mut b)` call. The former
retains ownership beyond the call; the latter needs a broader no-retention and
alias proof. A raw Bytes source and storing/taking consumers likewise keep the
existing path. None blocks removal of the measured target pairs.
