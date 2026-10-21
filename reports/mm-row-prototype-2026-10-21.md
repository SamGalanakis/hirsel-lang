# Isolated row-text prototype: keep ordinary Items

**Verdict: NO-GO for migration.** The experiment uses one immutable MEM-12
`TextFields(title, description, tags)` owner per ordinary indexed Row, with
id/status/priority/due outside the flat owner. The tasks application, Item,
Board, JSON, compiler rules, TIR/LIR and runtime ABI are unchanged.

The implementation is a test-only package in
`crates/hirsel-driver/tests/fixtures/row-text.hir.json`. It imports the actual
application model and store encoder. Its consuming helpers in
`row-text-parts.hir` belong only to that package. The adapter builds from owned
Items or directly, borrows indexed rows and walks variable tags, replaces one
text owner, changes metadata, removes by stable id, and explicitly materializes
independent Items, titles and deep snapshots. Positions are never stable ids.
A shallow copy has no promised row-only detach cost.

`own_text` is a **workaround**: copying a borrowed String in an ordinary helper
passes all validators. Direct `unique row.text.title` passes source checking
and the interpreter but fails native Import validation with `Holding(ValueId1)`.
The 20-line `row-text-materialize.hir` preserves that separate compiler
limitation; the helper and ordinary-record twins execute correctly. The
`import-unique-projection` lane owns its repair. No validator was relaxed.

## Correctness and ownership

Frozen compiler/std/runtime base: `a4551d85152cc3d81494f39babf27f6f09dd58c6`.
The Rust driver harness is registered as `tests/it/row_text.rs`; Buck2 also
carries the actual model/store inputs. Seven added tests were executed across
focused runs; the last retention revision re-executed its two affected tests.

- C derives the complete Row at **40 bytes**, alignment 8, flat-owner pointer
  at offset 32. The reduced shape also checks a 16-byte indexed carrier.
- Source validation, C at every native stage, H, LLVM diagnostic/production,
  and Cranelift diagnostic controls pass for the working adapter. The small
  shape and detached-title controls include H fault campaigns.
- Product oracles compare the existing model and complete encoded JSON after
  first/repeated update, metadata update, 1-to-200-byte title, 4,000-byte
  description, eight tags, Unicode, missing/repeated/early deletion, ids and
  order. Explicit deep and encoded snapshots survive source mutation/drop;
  both release orders execute. A live title relation rejects the overlapping
  write with MODE-1; its ended-loan twin accepts.
- Exact10k owned and direct construction each reproduce **1,437,828 bytes**,
  SHA-256 `de7fb9101257206be69df18c46cfaf60ed02b95d986ea642837bcb63d6e2e4a8`,
  under both native backends. This runs from Rust, never a raised interpreter
  budget. The independent frozen fixture audit passes **10,098 assertions**.
- Bounded H retention twin: **898 allocations / 898 frees**, zero final live
  heap bytes, peak 58 blocks / 5,072 capacity bytes. It uses structural Item
  equality and a shorter description; full stress JSON executes natively.
- H cost twins with 3 and 17 independently owned rows show zero additional
  allocations/dups for one borrowed read, scalar update or early removal.
  Replacement adds exactly **two allocations**, independent of unrelated row
  count: one payload plus construction temporary. Peak increases by one
  block / 48 capacity bytes. All controls finish with zero live heap bytes.
- Six selected MEM-12 conformance cases freshly pass. Two selected executable
  MEM-12 controls pass LLVM smoke (22 launches) and Cranelift parity (2 cases).
  MEM-12-flat-list-builder-run was never executed natively by hand.

## Matched inclusive workload controls

Ten seeded interleaved rounds (`20261021`) per workload, three participants,
150 fresh samples. One frozen LLVM O3 production binary selects ordinary,
flat-from-owned and flat-direct paths. CPU31 runs the processes; CPU7 drives
samples, holding existing CPU7 then CPU31 timing locks. Total/user instructions
and cycles are retained separately. Every sample verifies the same actual
output and includes process startup, setup, operation, cleanup and descendants.
No historical selection/destination improvement is credited.

These are inclusive workloads, **not isolated operation windows**. Build and
drop cells construct/drop ten boards (build also reads them). Read, update and
materialization cells build once, perform 100 operations, then drop. Therefore
direct-path advantages cannot establish separate build/read/drop benefits or
exclude update/materialization regressions after subtracting setup.

| Workload | From-owned / ordinary instructions (95% paired interval) | Direct / ordinary instructions (95% paired interval) |
|---|---:|---:|
| Build + read + drop | 1.146 [1.122, 1.169] | 0.341 [0.334, 0.346] |
| Read with setup/drop | 1.191 [1.190, 1.229] | 0.777 [0.776, 0.778] |
| Replace text with setup/drop | 1.161 [1.159, 1.163] | 0.434 [0.432, 0.435] |
| Drop with construction | 1.156 [1.156, 1.156] | 0.335 [0.335, 0.336] |
| Materialize with setup/drop | 1.157 [1.156, 1.158] | 0.434 [0.434, 0.435] |

The from-owned path fails every instruction-benefit/no-regression requirement
outside paired uncertainty. The direct seam is promising only as an isolated
construction result. No application migration is authorized by this experiment.

## Storage accounts and unmet gates

Separate measurement archives, never timing archives, report these complete
10k build/read/drop diagnostic events. Repeating a borrowed read adds **zero**
allocations or copied bytes. A sparse update adds one ordinary allocation or
three flat allocations and only one extra copied flat block / 64 copied bytes;
it does not reconstruct unrelated rows. Copied bytes count runtime copy hooks,
not every constructor's byte write.

| Diagnostic account | Ordinary | From-owned | Direct |
|---|---:|---:|---:|
| Allocation events (all subsequently freed) | 80,004 | 100,005 | 40,004 |
| Copied blocks | 50,000 | 60,000 | 0 |
| Runtime copied bytes | 1,860,000 | 2,820,000 | 0 |
| Logical peak payload capacity bytes | 2,960,176 | 3,360,160 | 1,360,176 |
| Final live payload capacity bytes | 0 | 0 | 0 |

The from-owned peak includes input/output/construction overlap; it is not a
steady-state saving. Flat framing and tag-list builder temporaries remain real
costs. These capacities exclude eight-byte owner headers and are not backing,
reserved memory or RSS. The fixture has 418,894 UTF-8 text bytes; subtracting
that from the historical board census would not measure recoverable memory.

**Backing/reserved/RSS reduction remains unproven.** The retained sampling
attempt used a harness launch without a stdin grant, so its intended hold read
returned immediately. `/proc` saw a zombie and no idle heap snapshot existed.
The failed attempt and corrected inclusive session retain this as unavailable;
no settled live-block/header/backing/reserved/RSS number is fabricated. Exact
header/backing accounting, isolated operation windows and retained RSS must be
resolved before any favorable adoption decision. The numerical gate already
fails for owned conversion, so ordinary owners remain the fallback.

## Provenance and remaining work

Prior art is pinned to Goose `8511ae34bdc9aabf77baaab80b4d659ea87835e2`,
`test/codegen/print_variable_size_elements.goose:1-10` and
`docs/goose_spec.md:304-477`; Rust
`a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`library/alloc/src/vec/mod.rs:2307-2312,2347-2378`. The nested walk and ordered
`[a,b,c] -> [a,c]` vectors execute in the small source control. Hirsel's
independent counted text owners, immutable replacement and loan rejection are
deliberate deviations from Goose's pooled storage; ordinary metadata and public
Items follow the decision. No new pool, codec, allocator or lifetime semantics.

Durable external evidence:
`/workspace/code/hirsel-briefs/memory-moonshot-2026-10-21/prototypes/mm-row-prototype-4-evidence/`.
It preserves source/compiler/std/runtime/harness/fixture hashes, actual outputs,
allocation traces, snapshots/retention observations, ten-round samples, all
failed attempts and the direct-projection minimal repro. Historical source and
fixture exports were recovered from the prior durable design report, without
recreating or claiming a new historical service measurement.

Done: isolated adapter, product/lifetime/cost probes and bounded comparison.
Declined: production migration, chunks, whole-board flattening, SSO, Shared,
compiler/runtime changes and claims of established backing/RSS savings. Open
compiler repro: `row-text-materialize.hir`, owned by import-unique-projection.
