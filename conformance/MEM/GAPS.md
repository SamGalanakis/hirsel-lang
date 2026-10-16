# Remaining memory coverage

- MEM-3/6: exact scratch/frame-depth and arena exhaustion boundaries await #16's
  logical accounting table. Zero-capacity empty arena cases do not assume a charge.
- MEM-4/10: builder publication, relocation, immutable nested blocks, heap-child
  release and the 8 MiB idle-pool limit need instrumented model/native checks.
  Source tests check values, not addresses or count fields.
- MEM-5: explicit/generic/field cleanup effect pairs now exercise noalloc_drop;
  cross-definition effects must come from signatures.
- MEM-8: simd.splat/add are declared; zero-width rejection has a control. Full
  integer-lane boundaries require the native/SIMD interpreter operation suite.
- MEM-9: scalar unchecked_add's documented contract is explicit. Foreign wrappers
  and raw-memory APIs still need #6 and separately declared operation signatures.
- MEM-12: @ growable is field-only and validated by owner/type/position pairs.
  Exact flat bytes remain #1; no prototype format is silently normative.
- MEM-11/14: physical sharing, generation wrap and storage reclamation retain
  their model oracles. `conformance/m2-runtime.txt` lists the dynamic runtime dependencies.
