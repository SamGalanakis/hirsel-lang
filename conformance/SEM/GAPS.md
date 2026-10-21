# Semantics coverage dispositions

Current rules are [SEM-1–5](../../docs/spec/semantics.md), the
[core contracts](../../docs/spec/core-library.md) and
[CT-3/5](../../docs/spec/compile-time.md). The old v20 section numbers and
issue numbers are historical, not current feature requirements.

- **Resolved capability history:** integer conversions/division, float conversion
  errors and the adjacent-pair sum tree, scalar/grapheme types, terminal positions,
  UTF-8 and replacement order have executable controls. Flat bytes and shape-driven
  generation are shipped; they are no longer missing features.
- **Open oracle coverage:** universal float rounding, reduction graphs across
  optimizations, Unicode construction invariants and SipHash-1-3 for every seed
  require the independent models in [UNTESTABLE.md](UNTESTABLE.md). Finite source
  examples do not prove universal arithmetic or physical bucket placement.
- **Contract boundary:** add transcendental controls against the named operation's
  declared contract; an undeclared correctly-rounded API is not an implementation
  defect. Private flat layout is distinct from the canonical serialization codec.

The [evidence index](../coverage-ledger.md#semantics) records exact controls,
execution results, owners and residual scope for every former entry.
