# v20 semantics and remaining oracles

Conversion and integer division signatures, exact-division failure, float.to_i32
errors, the float.sum tree, scalar/grapheme element types, terminal string positions,
UTF-8 decoding and map replacement order are specified in section 22. New runtime
witnesses exercise those choices. Grapheme tables are pinned to Unicode 16.0.0.

Floating-point universal correctness and SipHash-1-3 remain arithmetic/hash model
oracles in UNTESTABLE.md; concrete source examples cannot prove them. Correctly
rounded transcendental APIs beyond the declared ledger require named library
signatures before adding tests. Flat bytes remain #1. Shape-driven generation is deferred from v1
(#5). All ratified provisional markers were removed.
