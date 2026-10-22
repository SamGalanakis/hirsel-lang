# Tasks performance results

This is a historical report. Publish each new measurement in a separate
`RESULTS-<lane-or-session>.md` beside it; do not append rows here. Discover
reports with `rg --files -g 'RESULTS*.md'` (no shared report index to edit).

Microbenchmarks: baseline `b37afd182199`, ten seeded interleaved rounds on CPU31,
seed 20261020, matching output checksums. Hirsel workloads use N=20,000 and one
epoch; the failure-table example uses 10,000 sites and 100 loads. Instructions
include process startup and teardown. Raw sessions retain binary hashes, all
counters, schedules and SMT sibling samples in [results](results).

| Workload | Before instructions | After instructions | After / before |
|---|---:|---:|---:|
| ASCII case folding | 431,041,056 | 48,110,296 | 0.112 |
| ASCII case-insensitive search | 534,760,485 | 53,278,490 | 0.100 |
| ASCII NFKC normalization | 915,775,358 | 47,333,384 | 0.052 |
| Short decimal formatting | 11,979,900 | 10,951,398 | 0.914 |
| Derived record JSON (DOM → direct writer) | 117,268,655 | 71,941,692 | 0.613 |
| JSON object parsing | 109,743,242 | 109,632,312 | 0.999 |
| Failure-table load (10k sites × 100) | 1,470,399,435 | 339,102,665 | 0.231 |

JSON parsing is effectively unchanged (0.999×); its tokens keep independent storage.
Instruction counts are the verdict. Cross-session wall times
are not compared. The direct writer uses the same field order, escaping and
nesting limit as the DOM path. Parsed tokens retain independent storage so a later unique conversion
copies only the token. A shared-input experiment was rejected after the
10k-snapshot restart exceeded the 4 GiB cap (whole-backing-block slice copies). A native regression fixture bounds the physical copy bytes for 256 short tokens
in a padded document. Lazy failure tables retain
load-time validation and decode diagnostic strings once, when first needed.

The application session uses ten seeded interleaved rounds of old Hirsel,
new Hirsel and Rust current_thread on CPU31 (client CPU7). Each operation
uses a fresh process, a 10k-task snapshot, 1 req/s and one second of load;
cold uses an empty snapshot. Full counters, hashes and sibling load are in
[the raw session](results/tasks-session.json). The Hirsel baseline is the
tasks-rust lane’s measured binary (source revision `20c7208b9623`). These
measurements exclude the separate sse-memory lane’s deep-copy changes.

| Operation | Before / Rust | After / Rust (95% CI) | After / before |
|---|---:|---:|---:|
| cold | 3.934 | 2.410 [2.295, 2.423] | 0.613 |
| restart | 6.344 | 4.621 [4.610, 4.631] | 0.728 |
| search | 13.883 | 4.640 [4.630, 4.649] | 0.334 |
| create | 2.791 | 1.602 [1.600, 1.603] | 0.574 |
| update | 2.788 | 1.601 [1.600, 1.603] | 0.574 |

Validation on the final implementation: all 6,039 corpus cases passed (6,033
selected in-gate), 5,000 agreement variants passed with none skipped or flagged,
and native smoke plus all 113 selected test targets passed. The tasks suite
remains 22/22. Eight focused regression tests were added: two corpus cases,
two Unicode differential tests, one decimal differential test, two runtime
failure-table tests and one native JSON physical-copy regression. The corrected
server also passed 11 differential HTTP/HTML checks and four persistence-failure
checks against Rust. The verifier additionally passed Rust's SIGTERM and
storage-provider checks. No performance samples were excluded.

A follow-up session compares the isolated tasks-perf binary with the landed
combined revision `044644a0e7e2`, which includes sse-memory `402118f9` and other
intervening main changes. This is a combined-revision comparison, without
attributing the entire difference to one lane. It uses the same ten-round
seeded, interleaved protocol, CPU31/CPU7, 10k board and 1 req/s for one second.
The comparison driver selects only `list` and `search`; all 60 samples passed
and none were excluded. [Raw counters and hashes](results/combined-search-list-session.json)
retain the independent baseline for this session.

| Operation | Isolated / Rust | Combined / Rust (95% CI) | Combined / isolated |
|---|---:|---:|---:|
| list | 2.834 | 2.609 [2.566, 2.638] | 0.920 |
| search | 4.642 | 1.049 [1.043, 1.055] | 0.226 |
