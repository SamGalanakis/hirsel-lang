# Binary recording and lazy replay

Ordinary tapes move from text v7 to binary v8; durable journals move from v1
to v2. Both engines share the same explicit payload codec and CRC32C frame
envelope. The event set, Prepared protocol, completion delivery barriers,
ordinary flush points and durable synchronization points are unchanged.

Frames carry length, CRC32C, sequence, kind and raw payload. A reusable frame
buffer replaces event formatting and hexadecimal expansion. CRC32C streams
over the encoded spans in place, using SSE4.2 when available and slicing by
eight otherwise. Recording off skips absent-journal dispatch, issuer capture,
capture views and event/completion clones. Installed custom journals retain
their dynamic contracts.

Binary replay retains the original buffered allocation and indexes validated
frame ranges. It decodes only the current and previous events. The adapter
still returns owned completions; durable recovery retains the semantic events
and preparations needed for reporting in-doubt work. These boundaries are the
remaining copies, rather than a second owned ordinary event corpus.

Text rendering remains available for inspection and legacy v6/v7 imports.
Owning observation tests regenerate ordinary and durable fixtures through the
real writers; their semantic golden JSON excludes only the encoding-specific
digest and byte count from fixed comparisons.

## Prior art

Inspected upstream sources and ports:

| Source | Revision and location | Structure used |
| --- | --- | --- |
| rr | `50d54f2f541d79b508044fefdb2574fe2ec746f7`, `src/TraceStream.cc:408-557` | Length-delimited binary events over a buffered stream. |
| SQLite | `049891534ddad2583a33750c05433fcce3648b9c`, `src/wal.c:47-68,990-1022` | Append-only frame header, payload and checksum validation. |
| crc32c | `c92fcff3c76aed4f1083894f243981c0df278f7f`, `src/lib.rs:49-70`, `src/sw.rs:42-70`, `src/hw_x86_64.rs:49-90` | Hardware dispatch and slicing-by-eight fallback; published and incremental vectors from `tests/simple.rs:31-65`. |
| bincode 1.3.3 | `c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68` | Fixed little-endian scalars, explicit variant tags and length-prefixed raw byte spans. |

Deviations and reasons:

- Compression is omitted because ordinary completions must flush individually.
- SQLite page identifiers, salts and rolling checksums are omitted because the
  log contains ordered run events. Per-frame sequence and CRC32C remain.
- Bounded unaligned loads replace crc32c's alignment prefix and parallel-lane
  tables, keeping the implementation small while retaining its test vectors.
- Payload field order and variant tags are declared explicitly instead of
  relying on serde derives, whose JSON attributes can omit or rename data.

## Validation

Before the extra binary corruption/replay integration test, the focused
schema/runtime run passed 6 schema tests and 303 runtime integration tests
(one ignored). The final relevant campaign passed **620 tests, zero failures,
13 ignored**, across 23 binaries/shards: runtime unit/integration, run-schema
unit/integration, all 16 CLI shards, stream replay, interpreter and driver.
This includes exact cross-engine binary event identity, both native backends'
recordings replayed by the interpreter, durable recovery and byte-exact raw
non-UTF-8 payloads. The additional test rejects every truncation and every
single-byte corruption of a complete ordinary tape.

The selected RUN/replay corpus validated and passed **21/21 cases**, with no
unimplemented cases. Generated build files and benchmark outputs are excluded
from the change.

## Measurement protocol

Instruction counts are the verdict. Each campaign uses ten seeded interleaved
rounds, server CPU31, client CPU7 and the shared timing lock. Tasks create,
update and list use four requests/second for three seconds per cell. Profiles
are separate from the counter samples. Before/after campaigns are separate
sessions, so no wall-time speedup across them is claimed.

The baseline tasks session (`2026-10-21T01:16:34Z`, seed 20261020) completed
60 samples without errors. Recorded/plain total instructions per request were
18.7961 for create, 18.8209 for update and 22.9550 for list. Recorded create's
flat self profile was dominated by formatting (format_inner 14.12%, fmt write
11.03%, LowerHex 6.78%, pad_integral 6.05%), malloc 9.04%, memmove 8.28% and
hex 6.82%. This directly identifies the removed per-byte text path.


The final tasks session (`2026-10-21T01:45:20Z`) completed 60 samples with
zero errors. Each value below is the median instructions/request in its cell;
ratios divide those medians within the same session.

| Operation | Plain | Recorded | Recorded/plain before | Recorded/plain after |
| --- | ---: | ---: | ---: | ---: |
| create | 47,202,416 | 50,394,901 | 18.7961 | **1.0676** |
| update | 47,118,419 | 50,315,801 | 18.8209 | **1.0679** |
| list | 535,072,886 | 636,000,476 | 22.9550 | **1.1886** |

Create and update meet the 1.5 target. Final recorded create has 440 profile
samples: the leading self costs are application bytes/text append, memory
copying and JSON writing. The old per-byte hex/LowerHex formatting stack is
absent. Remaining recording work includes raw payload copies into the reusable
buffer, CRC32C, owned Request/Completion capture and the required completion
flushes; the profile is not a percentage decomposition of the instruction gap.

The net session (`c5749db6-65fa-4995-8fad-272792e6dfd9`) interleaved frozen
st-io before, final after, both recorded variants and Rust: ten rounds, 50
samples, seed 20261020. Every sample completed 1024 requests with the expected
stdout digest. The minimal trigger is `bench/micro/net/net-only.hir`: accept
one connection, read 35-byte requests, write one fixed response for each, and
print the completed count. The client offers 2048 requests/second.

| Net participant | Total instructions / Rust | User instructions / Rust |
| --- | ---: | ---: |
| before plain | 1.017 | 2.131 |
| before recorded | 3.517 | 176.310 |
| after plain | 1.039 | 1.897 |
| after recorded | 1.642 | 26.645 |

Recorded/plain net ratios are **1.5805 total**, 14.0472 user, versus 3.4578 total and 82.7549 user before. The total instruction ratio meets the 3 target; the user-only ratio remains higher because recording uses the owned request/completion path, payload encoding/checksumming and per-completion writes rather than the plain borrowed socket path. Kernel socket work dominates the total denominator. User-only and total ratios must not be conflated. No claim is made that user-only net overhead is below 3.

Raw samples, binary hashes, client latency/checksum evidence and before/after profiles are preserved in the lane measurement artifacts. Benchmark-generated result pages are not committed.
