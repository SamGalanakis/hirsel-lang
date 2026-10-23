# Destruction coverage dispositions

[DROP-2/3](../../docs/spec/failure.md#drop-2-resources) define strict cleanup,
unit-only nonpropagating finalizers, bug failure during drop, declared `@noalloc`
and structural `noalloc_drop`. These are settled rules, not v20 proposals.

- **Resolved fixture-support history:** cross-module `forget` has package-backed
  accepted/rejected controls. External representation restrictions use current
  package fixtures; source declaration names need no dotted-path extension.
- **Instrumented controls present:** runtime tests cover shielded finalization,
  failure chains, fields after failed drop and process termination for a failing
  unsafe-FFI finalizer. Foreign metadata support is shipped.
- **Open oracle scope:** allocation instrumentation across every failure and
  cancellation exit, and scheduler/Io interleaving coverage, remain obligations.
  The M2 scheduler is implemented; a missing *particular oracle* must not be
  described as an absent scheduler. The cancellation/drop source control and
  low-level shielding controls are finite witnesses.

- **Finite synchronous model:** `failure_cleanup_21` composes three-owner takes
  with both branch arms, joins, first-iteration break, return, `?` and division
  failure. It checks exact finalizer identities/order and zero live storage in H,
  reached H faults, and full public observations across interpreter, H, LLVM and
  Cranelift. Native arbitrary-site fault injection is not exposed by the test
  adapters; these controls do not establish cancellation or universal coverage.

See [UNTESTABLE.md](UNTESTABLE.md) for universal obligations and the
[evidence index](../coverage-ledger.md#destruction-and-failure) for exact executed
controls, current failures, owners and minimal commands.
