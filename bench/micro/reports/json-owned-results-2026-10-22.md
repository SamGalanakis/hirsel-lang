# Owned typed JSON results, October 22, 2026

Derived byte decoders move the completed result after validation. The recursive
Tree no longer makes a second graph, and Date-bearing tasks snapshots retain the
same ownership proof. The borrowed Reader and token-view interfaces are unchanged.

The ten-round matched session reduces restart instructions by **26.94%** and typed
decode instructions by **38.42%**. Restart remains **2.4209× Rust**, above the
original 1.5× goal: **NO-GO** for that performance gate. Typed decode is 2.5392× Rust.
These are instruction-count results. Wall-clock timing is a follow-up.

[All 60 samples, executable identities and counters](json-owned-results-2026-10-22.json)
come from one seeded interleaved session (20261022). No sample was excluded;
every process exited successfully and every perf event ran 100% of its measured
interval. Raw stdout, stderr, perf CSV, status, launch markers and sibling snapshots
were retained separately. The typed outputs have one identical SHA-256 across
Rust, before and after. All 30 restart cells load the same seeded 10,000-task
snapshot and serve the same first request, including startup and shutdown.

The live host rule of October 22 limits exclusive timing-lock waits to ten minutes.
The earlier lock wait produced zero samples and was stopped. This instruction-only
session took no exclusive timing lock and ran alongside other work, with native
processes on CPU31 and client/coordinator on CPU7. Cycles and task-clock remain in
the raw counters for audit; their ratios are not acceptance evidence. The normal
4 GiB launcher cap and participant CPU budgets were unchanged.

Both Hirsel before/after executables use the same release compiler and runtime.
The compiler SHA-256 is `0f14bc0e869fba099fa2c07ad0ef9acb4ec06463587b395089d63b35b146b6fc`. Accepted source is baseline `2b0e271fc5e7`
plus this lane's Plain-proof change; the before std is the frozen baseline and
the after std replaces the final `unique decoded` with `take decoded`.
The unrelated later main changes used for landing are outside this measurement.
The reference is the unchanged serde-derived typed Rust micro and tasks one-worker
participant. Compiler startup is outside these native runtime samples; document
construction, ownership, key parsing, validation, runtime startup and cleanup are
inside the relevant participant counters.

| Workload | Participant | Total instructions | Total / Rust [95% paired-round CI] | User instructions | User / Rust |
|---|---|---:|---|---:|---:|
| restart | before | 320,469,024 | 3.3136 [3.3109, 3.3183] | 272,991,990 | 3.2916 |
| restart | after | 234,127,988 | 2.4209 [2.4172, 2.4266] | 201,134,918 | 2.4252 |
| typed | before | 4,387,630,217 | 4.1234 [4.1197, 4.1294] | 4,048,030,018 | 3.8667 |
| typed | after | 2,701,974,756 | 2.5392 [2.5381, 2.5407] | 2,601,788,562 | 2.4852 |

The typed input is the retained N=20,000 workload: construct one document and decode
it ten times. The restart trigger is the retained single request over a seeded
10,000-item task snapshot. Restart still exceeds its 1.5× Rust target; this change closes the
final-copy cause, not the remaining allocation/scanning/startup work. No wall-time
or broader application performance gate is claimed.

## Ownership evidence

The historical bare Tree move now passes on baseline main; it did not justify a
compiler rewrite. Adding checked opaque `time.Date` reproduces the MODE-1 loss.
The counted-part overlap walk treated a hidden representation as unknown despite
its checked public `plain` bound. Honoring that existing bound preserves the proof
without treating borrowed input as global. Non-plain opaque representations remain
conservative, and a genuine Reader token escape still rejects at MODE-1.

Separate measurement builds count physical operations, outside the timing session:

| Fixture | Allocations before → after | Deep copies | Copied blocks | Copied bytes | Live bytes at exit |
|---|---:|---:|---:|---:|---:|
| Leaf/Branch Tree, three owned strings | 120 → 115 | 1 → 0 | 5 → 0 | 384 → 0 | 0 → 0 |
| One-item task Board | 25 → 19 | 1 → 0 | 6 → 0 | 640 → 0 | 0 → 0 |

The native regression compares the derived byte entry against an explicit owned
reader factory, requires zero graph copies and equal allocation counts, and checks
that early errors after partial construction leave no live allocation. Nested
records, variants, lists, escaped and unescaped strings, checked opaque values,
UTF-8, trailing input and error precedence are covered by the retained tests.

## Validation

- 115 selected conformance programs pass, including both new owned/borrowed twins
  and the retained 293-input UTF-8 differential corpus. The old validators and
  dynamic JSON expected results are unchanged.
- Four JSON stdlib Rust tests pass, including the native allocation/copy/cleanup
  regression and independent Python number/fuzz oracles.
- Eleven selected decoding cases pass native smoke and Cranelift parity; smoke
  executes 121 image runs, plus its Cranelift executables, with no executable
  fallback. All 25 task tests pass interpreted and LLVM.
- Type analysis: 10/10; modes: 13/13; validator: 37/37. Ownership remains 34 passing
  and one existing corpus-ratchet failure, identical to baseline and already in
  `tools/known-failures.tsv`; it is not a green suite claim.

The deliberate failing copy regression (one executed test), opaque-Plain regression
(one executed test), setup errors and final passing runs are preserved separately.
A CAS upload identity-size failure proves neither compilation nor execution. A
fixture-only missing list type was fixed before the final four-test stdlib run.
The complete 10,680-file source inventory was frozen and checked unchanged through
the scoped compiler/correctness checks; the later test-only type annotation has its
own inventory. Rebase registry conflicts preserve both lanes' entries.

## Prior art and deviations

serde-rs/json `8c535a260ce561eaab4111aafc29038eb9f77ff3`, `src/de.rs:2624-2630`
uses `DeserializeOwned` for owned reader results; `src/read.rs:521-566` distinguishes
borrowed unescaped tokens from scratch/owned strings. The retained record/variant
vectors are `tests/test.rs:1252-1475`.

Hirsel keeps its explicit loan relations and MODE-1 proof instead of adding Rust's
trait/lifetime machinery. Its existing String-error boundary, duplicate-key policy,
number spellings and full-document error precedence are preserved. The Plain-bound
repair is proof transport for hidden representations, not a new Reader contract or
serialization format. No TIR/LIR construct or runtime ABI changed.
