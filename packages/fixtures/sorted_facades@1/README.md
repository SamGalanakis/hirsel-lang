# Dictionary-bound facade pilot

Decision: keep this as fixture support in `packages/fixtures/sorted_facades@1`,
not public std. The survey found this repetition in fixtures and no default-order
argument sites in std, packages, examples or benchmarks. The current compiler
also prevents a complete facade migration. No compiler or language change is included.

## Current status

The measurements and blockers below are historical, recorded at the baseline
`b37afd18`. The brand-region-fixes lane has since fixed blockers 2 and 3:

- A receiving annotation written with the brand its value carries keeps it, whether
  it is spelled directly, through a transparent alias or with a module-qualified
  definition path (PROV-2 in [brands.md](../../../docs/spec/brands.md)). The four
  facade probes now accept as `PROV-2-facade-{alias-binding,concrete-result-annotation,generic-result,package-result}-ok`.
- Checked execution bounds a resource that a native operation moves out of a `mut`
  argument by the caller's content, not by the wrapper's access to it. The repro is
  now the passing corpus case
  [DROP-2-facade-resource-remove-run.hir](../../../conformance/DROP/DROP-2-facade-resource-remove-run.hir).

The remaining calls have not been migrated again since then.

## Measured migration

Baseline: `b37afd182199b86fd85331a7edacfc4f3590ffa1` (the launch `origin/main`).
The fixed survey ledger is recorded in [ledger.json](ledger.json); it contains
the same 87 argument positions surveyed at `e50d5e6eaaeb498378b636cdf7887fb332286f54`.

| Operation | Survey sites | Migrated | Blocked |
| --- | ---: | ---: | ---: |
| empty | 21 | 0 | 21 |
| insert | 32 | 32 | 0 |
| len | 13 | 13 | 0 |
| get | 11 | 11 | 0 |
| remove | 10 | 9 | 1 |
| Total | 87 | 65 | 22 |

Across all 15 ledger files (including the three entirely unchanged files):

- Source tokens: **3,716 → 3,586**, saving **130 (3.50%)**.
- Physical source lines: **500 → 493**, saving **7 (1.40%)**.
- All 13 `len` witness operands are removed.
- The original witness bindings and raw receiving annotations remain.

These counts exclude new facade bodies, manifests, tests, scripts and this report.
No type-annotation savings are claimed. Formatting accounts for the line savings;
the token saving is exactly `by` and its comma at each migrated call.

Reproduce from the repository root:

```sh
python3 packages/fixtures/sorted_facades@1/measure.py
```

The script asserts the per-file call counts and restores the removed operands
before comparing complete token sequences. Thus every existing `mut`, `take`,
brand, relation, authority operand and test assertion survives the migration.
Comments and whitespace are excluded from tokens; strings and suffixed integer
literals each count as one token, and compound punctuation uses longest matching.

| Fixture | Sites migrated / surveyed | Lines before → after |
| --- | ---: | ---: |
| `conformance/CT/CT-2-sortedmap-constant-ok.hir` | 0 / 1 | 7 → 7 |
| `conformance/DROP/DROP-2-sorted-map-replaced-value.hir` | 3 / 4 | 26 → 26 |
| `conformance/DROP/DROP-2-std-sorted-map-remove-run.hir` | 4 / 6 | 35 → 35 |
| `conformance/GEN/GEN-1-sorted-map-copyable-key.hir` | 0 / 1 | 21 → 21 |
| `conformance/GEN/GEN-1-sortedmap-runtime-equality-bad.hir` | 1 / 2 | 16 → 16 |
| `conformance/MEM/MEM-6-sorted-map-arena-run.hir` | 19 / 25 | 147 → 147 |
| `conformance/MEM/MEM-6-std-sorted-map-arena-get-run.hir` | 3 / 4 | 33 → 33 |
| `conformance/MODE/MODE-2-std-container-empty-run.hir` | 0 / 2 | 25 → 25 |
| `conformance/OWN/OWN-4-heap-capacity-sorted-closure-run.hir` | 2 / 3 | 17 → 16 |
| `conformance/SEM/SEM-5-sorted-map-projected-and-consuming-run.hir` | 2 / 3 | 31 → 31 |
| `conformance/SEM/SEM-5-sorted-map-resource-modes-run.hir` | 1 / 2 | 35 → 35 |
| `conformance/SEM/SEM-5-std-sorted-map-access-run.hir` | 17 / 18 | 51 → 45 |
| `conformance/SEM/SEM-5-std-sorted-map-owned-cycle-run.hir` | 2 / 3 | 26 → 26 |
| `conformance/SEM/SEM-5-std-sorted-map-string-get-run.hir` | 9 / 10 | 18 → 18 |
| `native-tests/regressions/history/native-heap-heap-d15facecd1.hir` | 2 / 3 | 12 → 12 |

## API and wrapper cost

The language reserves builtin type names even inside modules: the requested
`u32_maps.Map[V]` declaration rejects with LANG-7. The pilot uses the transparent
`u32_maps.OrderedMap[V]` alias instead, plus its `u64_maps` companion because
the ledger uses both widths. It does not introduce a fresh dictionary witness.

Each facade is **24 physical lines**: one alias, five explicit wrappers,
one purpose comment, attributes and blank lines (48 lines for both). The source
facade bodies alone therefore exceed the seven lines saved in callers by 41
lines, before accounting for manifest and assertion overhead. These fixtures
already need package manifests to import the pilot; those costs are separate
from the source-only comparison above.

`insert` retains `mut m`, taken key/value, and `@preserves_unique(m)`;
`get` retains `V: copyable` and `Option[V] in m`; `remove` retains a unique
mutable map and unique result; `len` retains `@noalloc`. Every wrapper declares
`uses {}`. The core ledger’s `@unretained(by)` is not source syntax; the facade
has no borrowed dictionary parameter because it names the global default.

The headers can write those obligations, but the following evidence transport
and execution limitations prevent claiming all obligations work end to end.

## Blockers and durable probes

1. **Reserved alias name (LANG-7).**
   [LANG-7-facade-map-reserved-bad.hir](../../../conformance/LANG/LANG-7-facade-map-reserved-bad.hir)
   is the minimal requested spelling. All callers use `OrderedMap` where supported.
2. **Result/receiving-annotation brand loss (PROV-2): 21 empty calls blocked at the baseline; fixed.**
   At the baseline a concrete result kept its brand only without a receiving
   annotation; writing the same branded type, a local alias, a generic result or
   the package result dropped it. The probes are now the passing
   [PROV-2-facade-generic-result-ok.hir](../../../conformance/PROV/PROV-2-facade-generic-result-ok.hir),
   [PROV-2-facade-package-result-ok.hir](../../../conformance/PROV/PROV-2-facade-package-result-ok.hir),
   `concrete-result-annotation-ok` and `alias-binding-ok`, next to the
   `concrete-result-ok` twin. The raw `SortedMap[K,V] of by` declarations and the
   21 builtin empty calls were not migrated again. A zero-argument source header
   also has no caller arena parameter to name in its result relation; the pilot
   makes no claim of replacing the builtin's expected-destination placement rule.
3. **Resource result region: one remove call blocked at the baseline; fixed.**
   The original resource removal in `DROP-2-std-sorted-map-remove-run.hir` stays
   raw. At the baseline, wrapping it returned normally but checked interpretation
   trapped `region use after scope exit (MODE-1/MEM-10)` at the caller match. The
   checker was sound and the checked interpreter was wrong. The repro now passes
   in every engine as
   [DROP-2-facade-resource-remove-run.hir](../../../conformance/DROP/DROP-2-facade-resource-remove-run.hir).
4. **Initial slow check: no persistent stall reproduced.**
   The original small trigger is the durable `package-result-ok` corpus case (formerly `-bad`)
   linked above. An initial check took over two minutes, then completed; warm
   probes took about 0.5–0.8 seconds. The host rules already identify slow std
   prechecking (~118 seconds). This is a recorded observation, not a speed
   benchmark or an assertion that a new checker deadlock exists.

## Semantic checks

The cross-package `PROV-1-facade-alias-package-ok` case passes the named alias
to a raw `SortedMap[u32,String] of u32.order` formal and passes a raw map back
through the alias formal. Raw and facade default/descending twins both accept
the default and reject the descending dictionary with PROV-1. Alias equality
in these formal positions does not imply receiving annotations preserve evidence.

Raw/facade positive and negative twins cover `@noalloc` len versus allocating
insert, live get-result loans, unique remove inputs, and insertion of shared
versus explicitly unique counted values. `uses {}` headers check with no
capability grants. Existing resource drop order, projected/consuming iteration,
heap access and exact bounded-arena budgets run under ordinary and checked
interpretation; their original assertions are unchanged.

Focused baseline: **14 / 14** original conformance files pass. After migration:
**41 / 41** focused corpus cases pass (14 originals plus 27 new assertions).
The fifteenth ledger file is a historical native regression and is selected
separately for both native backends by the gate. Gate/landing receipts provide
the final validation result; this report does not assert a runtime speed benefit.
