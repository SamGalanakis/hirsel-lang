# Independent finite decimal vectors

`part-*.json` contains 166 fixed IDs (131 arithmetic rows, one minimized regression) in 25 bounded runtime
partitions. Seed: 2026102506. Each ID preserves its canonical input SHA256,
expected-output SHA256, upstream raw answer (where selected), Python raw answer,
adapted representation and exact output bytes. Expectations are host literals;
only input lexemes are passed through `decimal.parse`.

Oracles: CPython `_pydecimal.py` at
`8b7d6c53eee2226b08bed761d5a1742b5adb8507`, context precision 160; rust_decimal at
`b23fb077e301e77cb0ac36cc1ac4df1fba6c45d5`, default features disabled, `std` enabled.
The six modes map explicitly to ROUND_HALF_EVEN, ROUND_HALF_UP, ROUND_DOWN,
ROUND_UP, ROUND_FLOOR and ROUND_CEILING, and rust_decimal's corresponding strategies.

Inputs have at most 24 coefficient digits, scales -6..12 and requested places
-4..8. Aligned sums have at most 43 digits, products at most 48, and requested
quotient coefficients at most 56: 160-digit contexts cannot round exact arithmetic.
Every division/rescale has an exact rational quotient/remainder/denominator
certificate, independently checking the final rounded integer. This also certifies
that rust_decimal's division followed by rounding did not double round on the 104
selected corroborating cases. Rust's native scale is retained separately; only
numeric results within its 96-bit/28-scale domain corroborate the unbounded API.

Fixed upstream vectors: rust_decimal bankers rounding goldens in
`tests/decimal_tests/rounding.rs:73-90`; IBM `rounding.decTest` IDs radx100..102 and
rmux116..117. IBM's precision-5 answers are retained, but exact add/multiply use the
specified coefficient/scale representation. Exponents are decoded into coefficient
and scale outside Hirsel; the public parser remains strict. Oracle sanity also used
the cited first fourteen `tests/generated/Add_000_001.csv` rows, outside this bounded
matrix. See `manifest.json` for each deliberate semantic adaptation and its spec
paragraph. Existing IBM decTest row identities and skips are unchanged.

Each partition uses the real std package graph and public source/check/export/
launch adapters. Reference, checked H, LLVM and Cranelift each compare exact output
and outcome against the same independent fixture. The observation projection is
native observation schema 5: output, exit, failures, blocked/budget/signal and roots;
no resource effects are required by these pure operations beyond printing.
Zero divisor and both i32 scale-overflow directions run as individual SEM-1 policy
controls with zero/tiny coefficients. A syntax-error Result is an observed Invalid
value, distinct from source/V rejection, H trap, timeout or tooling failure.

Attribution: rust_decimal Copyright (c) 2016 Paul Mason, MIT. CPython Copyright
(c) 2001 Python Software Foundation, PSF-2.0. IBM General Decimal Arithmetic
Copyright (c) IBM Corporation, 1981, 2008. All rights reserved. These experimental
vectors are offered as-is, may contain errors and are not exhaustive; matching
them is no guarantee of compliance with a standard or specification. Full notices
and licenses are in `third-party/NOTICES.md` and
`native-tests/library/vectors/LICENSE.cpython`.
