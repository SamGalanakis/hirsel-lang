# Tasks performance results

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
| JSON object parsing | Re-measurement pending after restoring independent token storage | — | — |
| Failure-table load (10k sites × 100) | 1,470,399,435 | 339,102,665 | 0.231 |

Instruction counts are the verdict. Cross-session wall times
are not compared. The direct writer uses the same field order, escaping and
nesting limit as the DOM path. Parsed tokens retain independent storage so a later unique conversion
copies only the token. A shared-input experiment was rejected after the
10k-snapshot restart exceeded the 4 GiB cap (whole-backing-block slice copies). A native regression fixture bounds the physical copy bytes for 256 short tokens
in a padded document. Lazy failure tables retain
load-time validation and decode diagnostic strings once, when first needed.

Application measurements and gate results are recorded below when complete.
