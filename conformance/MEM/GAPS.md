# Memory coverage dispositions

Safe source observes values and diagnostics. Address stability, physical placement,
count fields and storage reclamation need instrumentation; passing source examples
cannot close those claims. See [UNTESTABLE.md](UNTESTABLE.md) and the
[evidence index](../coverage-ledger.md#memory).

- **Deliberately unspecified — MEM-3 / LANG-10:** logical frame-byte charges and
  normative structural scratch levels remain decisions in the
  [spec index](../../docs/spec/index.md#unspecified). The interpreter's current
  limits are implementation controls, not invented normative budgets.
- **Settled contract, separate current defects — MEM-6:** bounded-arena logical
  charges are normative in the [runtime table](../../crates/hirsel-rt/README.md#bounded-arenas).
  The old claim that all arena exhaustion boundaries awaited a table is stale.
  Exact-cap and cleanup fixtures must still be judged against their current
  checker outcomes; their inherited PROV-4 rows are retained until reproduced green.
- **Instrumented controls present — MEM-4/10:** runtime region tests cover
  unpublished relocation, publication, held-heap release, failure cleanup and the
  8 MiB idle-pool cap. These close the claim of absent controls, not every compiled
  placement path or cancellation schedule.
- **Resolved static coverage — MEM-5:** explicit, generic and field cleanup pairs
  test declared `noalloc_drop` effects. Universal instantiation and allocation-free
  cancellation/failure cleanup remain model/instrumentation obligations.
- **Bounded new control — MEM-8 / SEM-1:** checked SIMD addition has all-ten-width
  boundary and per-lane overflow/underflow coverage. This is not an exhaustive
  vector-operation or backend proof.
- **Shipped surface, open unsafe review — MEM-9:** scalar `unchecked_add`, foreign
  metadata, wrappers and raw-pointer APIs exist. Each declared operation needs its
  own contract vectors; universal unsafe aliasing and environmental crashes stay
  outside ordinary source determinism coverage.
- **Resolved feature history — MEM-12:** flat bytes and field-only `@ growable`
  validation ship. Physical pointer-free layout/copy-cost auditing remains distinct
  from wire-codec tests. Complete layout composition/native data ABI remains
  [unspecified](../../docs/spec/memory.md#mem-7-layout).
- **Partial model coverage — MEM-11/14:** exact root-count overflow and slot
  generation retirement have low-level controls. They do not establish weak-memory
  correctness or universal reclamation. The native corpus pending file contains
  executed obligations, not a list of missing model tests.
