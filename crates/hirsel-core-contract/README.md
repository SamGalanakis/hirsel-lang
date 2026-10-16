# hirsel-core-contract

The operation manifest schema shared by the native and reference engines.
Data only: no algorithms, no dispatch.

Allowed dependencies: hirsel-syntax (for `DefPath` and `ContentHash`). Never an
implementation crate, the driver or a tool.

Invariants:

- The driver registry (`BuiltinRegistry::operation_inventory`) is the sole
  producer of operation identities. A `Manifest` is built from those identities
  with `Manifest::skeleton`, which gives every row the conservative
  unclassified disposition. Rows can be replaced, never added or removed, so no
  registry operation can be absent; `check_coverage` compares row identities
  against an inventory in both directions.
- Every row admits "unavailable": an operation's call ownership is `None` until
  classified, and every engine disposition can be `Disposition::Unavailable`
  with a named reason. No schema is inferred from an implementation.
- Effect dimensions are independent booleans where `true` includes unknown, as
  in the registry's `OperationEffects`; a row never claims purity it did not
  prove.
- Manifest digests are computed by `hirsel-codec`'s snapshot encoding under the
  `hirsel-lir` snapshot domain, which includes the interface version.

A row's `failure: Option<RuleCode>` declares a deterministic native primitive's
status-1 rule. The caller records it at the call anchor before cleanup; numeric
codes remain driver-assigned. None permits success or an already-recorded
propagated failure. This field participates in the canonical manifest digest.
