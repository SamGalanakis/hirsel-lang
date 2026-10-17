# Destruction properties requiring another oracle

| Rule | Part | Oracle |
| --- | --- | --- |
| DROP-1 | All copyable values have no visible destruction; exact final-holder memory freeing | Type-system/RC model and exhaustive type construction; concrete plain/counted copies have run tests. |
| DROP-2 | Every possible exit/schedule obeys strict lifetimes | Frame-table/ownership model and differential runs with injected errors, failure and cancellation. `.hir` covers local/field/sequence order, replacement, scope end, moves, `?`, `return`, `break` and `continue`; FAIL-3 covers failure. |
| DROP-3 | Shielding and absence of cancellation points under every Io implementation | Cancellation-aware deterministic Io trace oracle, including cancellation during close/output and discarding close errors. One unconfigured program cannot force every cancellation interleaving. |
| DROP-3 | Only stored capabilities are available, across all possible resource implementations | Capability-summary proof/API audit. Stored Stdout is a concrete accepted test; missing ambient authority is enforced jointly by CAP-1/2. |
| DROP-4 | Abort semantics of the complete library file/flush/close APIs under all injected errors | Deterministic filesystem transaction model. The AtomicFile/save spec example and minimal explicit-commit/implicit-abort traces are included. |
