# Function and constant scaling after the compiler cutover

The `69412fd9` cutover already removes the two historical out-of-memory failures.
This change adds their missing stress series and replaces protoc's four-field
batching with one helper per field (oneof writers still handle one whole oneof).
It changes no V, modes or ownership verdict, lattice, cache policy or flow state.

## Measurement protocol

Use the release CLI and matching std image from `//:hirsel-toolchain` at the
cutover, with persistent results and the resident daemon disabled. Keep each
historical protobuf/std snapshot's image separate from the current std image:
a mismatched image refreshes from source, changing the measured workload. The
original GAP 5 chunk fixtures are unchanged. Every command runs through
`tools/capped`, with `--timings=json` and `--mem-usage` for the direct probes.
`perf stat` counts cycles and instructions, including child threads. Instruction
counts, rather than separately collected wall times, determine the scaling fit.

The initial isolated campaign has ten seeded rounds at each of three sizes.
It omitted the shared CPU timing lock; its instruction counts remain evidence,
but none of its wall values establishes a latency ratio. The subsequent official
stress matrix uses the runner's per-round CPU30 timing lock and frozen inputs
and images. CPU30 avoids occupying the shared CPU31 campaign queue. No compiler
implementation differs between the before/after stress runs, so no compiler
speedup is attributed to this change.

Fit `ln(instructions) = intercept + exponent * ln(N)` to the three medians.
These are total command counts with fixed startup cost; they establish the
observed range, not an asymptotic theorem or an incremental-cost exponent.

| Initial isolated input | N | Median instructions, billions | Maximum peak RSS, MiB |
|---|---:|---:|---:|
| Inline constant | 1024 | 3.027 | 133.00 |
| Inline constant | 2048 | 5.082 | 149.88 |
| Inline constant | 4096 | 9.276 | 177.00 |
| Wide record, match arms | 4 | 2.710 | 129.00 |
| Wide record, match arms | 8 | 2.875 | 129.50 |
| Wide record, match arms | 16 | 3.223 | 129.75 |

The fitted exponents are **0.808** for the constant and **0.125** for the initial
record probe. The final record generator also includes `?` on each assignment,
so the official matrix exercises failure-edge snapshots like a protobuf decoder.
A compile-time caller forces independent V during `check`; an ordinary runtime
main would only exercise the front checks under the current lowering policy.
The record has 165 list fields and four helpers at every size. The Rust pair retains the field/arm/`?` structure and uses runtime `black_box`
consumers. Hirsel additionally evaluates the caller at compile time to include
V in `check`; these pairs are regression controls, not equal-work compiler
throughput claims.

## Why there is no new flow-state rewrite

The current validator already packs finite facts into persistent radix rows
(`hirsel-validate/src/bit_rows.rs` and `hirsel-types/src/rows.rs`). Ordered fact
collections use shared immutable snapshots with copy-on-write mutation
(`hirsel-validate/src/shared.rs`). Ownership value payloads also share unchanged
content (`hirsel-ownership/src/flow.rs`). The historical diagnosis of copying
whole dense states is not sufficient evidence for replacing those structures
again after this cutover.

This is existing prior art, not a new data structure: the packed rows cite
rustc's `rustc_index/src/bit_set.rs:16–30,70–94`; rustc's dataflow scratch and
entry-state structure is in `rustc_mir_dataflow/src/framework/mod.rs:298–378`
at reference revision `19833af7224424e8b53e288097d56b6af43d5bcf`. Hirsel's existing
persistent paths adapt it for edge-specific snapshots. No new deviation from
that design is introduced here.

A CPU profile of the 16-arm V probe still finds temporary-order BTreeSet work in
`Checker::write`, plus image decode and copies. Its short sample is attribution,
not evidence for another broad rewrite. Heaptrack only intercepts the system
allocator, so its small LLVM allocation report does **not** describe mimalloc's
Rust heap. A separate Massif page profile records the underlying mappings,
including mimalloc's 1 GiB virtual arena reservation and the 128 MiB compiler
stack; those mapping sizes are not resident usage. `--mem-usage` independently
reports retained image and engine products.

## Reproduction

```sh
.buck2/bin/buck2 build --target-platforms //tools/buck2:release \
  //:hirsel-toolchain --show-full-output
HIRSEL_DAEMON=off HIRSEL_RESULTS=none@1 tools/compiler-bench/run \
  --toolchain <bundle> --output stress.json --work <private-work-dir> \
  --groups stress --select 'stress/(inline-constant|record-flow)-' \
  --modes check --cpu 30 --repeat 10 --jobs 1
```

The 4096-element acceptance bound is **peak RSS**, not a promise that a 256 MiB
`RLIMIT_DATA` can run the compiler: the compiler thread reserves a 128 MiB stack
in addition to heap mappings. A separate 256M guard probe exits 71 at a reported
157 MiB resident peak. The ordinary 4 GiB capped run checks successfully below
256 MiB RSS; no limit or guard was raised or weakened.

## Validation and generated outputs

The benchmark tool tests change from 30 passing / 2 stale expectations to 32
passing: the cutover renamed std precheck to image and added timing intervals
and phases. The expectations and timing inventory now match the actual compiler.

At the unchanged compiler baseline: modes 13, V unit 34 and V integration 482
pass. Ownership has 34 passing and its existing corpus ratchet fails on five
MODE-2 cases; authority has 27 passing and its existing PROV-1 corpus ratchet
fails. Those exact existing failures are recorded in `tools/known-failures.tsv`;
no assertion or expected diagnostic was relaxed. The MODE/OWN agreement slice
checks 652 variants: 315 accepted, 337 rejected, zero unavailable, 124 executed;
it also checks 367 originals, with no discrepancy clusters.

The modified protoc generates the full conformance message and well-known
sources through its ordinary `hirsel run` command. The generated full-message
package passes a binary/JSON round-trip test (1554 V runs; 574.66 MiB peak RSS),
and the regenerated well-known types pass all five existing protobuf tests.
Generated sources are retained as validation artifacts, not hand-edited or
committed by this lane, following the host's generated-file rule.

## Original protobuf scaling proof

| Fields per helper | Median instructions, billions | Maximum peak RSS, MiB |
|---:|---:|---:|
| 4 | 44.141 | 324.93 |
| 8 | 45.632 | 305.70 |
| 16 | 55.198 | 297.60 |

Ten seeded rounds give a total-command exponent of **0.161** for fields
per helper. Total fields and generated message semantics are fixed across these
historical chunk fixtures. One helper per field separately checks at **458.02
MiB** with **62.056 billion** instructions; the initial source-refresh run also
completed below 1 GiB (835.87 MiB). The full-message acceptance bound therefore
holds with and without that historical std image. No fixture, checker or cap was
changed for this proof.

## Final stress matrices

Both official matrices finish **24/24 cells, 240/240 command samples** with no
caps or failures. Each includes Hirsel/Rust and cold/warm at all six sizes.
The same frozen release CLI and std image are used in both; these are
repeatability and regression-coverage checks, not an implementation speedup.

| Series / temperature | Before exponent | After exponent | After maximum RSS, MiB |
|---|---:|---:|---:|
| inline-constant / cold | 0.802 | 0.802 | 181.00 |
| inline-constant / warm | 0.799 | 0.799 | 177.00 |
| record-flow / cold | 0.125 | 0.125 | 136.50 |
| record-flow / warm | 0.110 | 0.110 | 134.50 |

All measured exponents are below linear over this range, hence within the
requested N log N envelope. The final record series retains `?` failure edges.
Peak RSS remains below 256 MiB for the constant and below 1 GiB for the record.
No wall-time ratio is inferred between these separately collected sessions.
