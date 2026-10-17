# Coverage and assumptions

- **FACT-1:** pure/boolean conditions, result, plain before snapshots, taken-parameter ensures, contract allocations, false requires/ensures (including an early return), entry ordering and failures during requires/ensures/before evaluation are covered. A failure evaluating an integer operation expects SEM-1, not FACT-1. The allocation witness expects MEM-5 because that rule defines @noalloc. The at and append examples are made complete; append uses the declared list.push signature.
- **FACT-2:** Percent's exact example has 0/100 valid and 101/255 invalid constructors. Distinct scalar/ordinary-record/nonplain bases, a plain record base and nested distinct bases are covered. Facted-place mutability is owned by OWN/SEM, not an invented new FACT restriction. Multi-field invariants on an ordinary record are not allowed; external opaque Range construction uses a public constructor with a FACT-1 contract. Its valid/invalid twins exercise that contract across a package boundary while keeping the representation hidden.
- **FACT-3:** known false contracts/facts still check and fail at runtime; a true-contract witness can run with the check retained or removed. The corpus never expects compile rejection solely because a prover sees a false runtime condition. Optimisation invariance requires the matrix in UNTESTABLE.md.
- Signature defaults follow the LANG-6 v20 defaults. Generic type facts' inherited bounds follow LANG-7/GEN-1. Diagnostic locations for false contracts cite their contract expression; invalid distinct values cite their construction site.

The external constructor twins select `facts@1` through ordinary manifests.
They construct the dependency's `Percent` at 100 and 101: the valid boundary
runs successfully, while the first invalid value fails FACT-2 at the client
construction site. The valid value is destructured through a client alias.
The representation/niche oracle in UNTESTABLE.md remains independent of these
source-level package checks.

The external opaque invariant twins select the same package and call its public
Range constructor. Ordered endpoints succeed; reversed endpoints fail FACT-1 at
the dependency constructor's declared precondition. The failure rule follows the
contract mechanism specified for multi-field invariants in FACT-2.
