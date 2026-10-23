Relative-view fixtures use seed `20261026-text-view-bounds-26` as reproduction
metadata. The 96 histories are four texts × four nested-view depths × six
operations. Each valid history exhausts byte intervals, scalar intervals,
byte indexing within every valid window, scalar boundaries (including terminal
positions), and invalid position offsets through helper returns. The other five
histories exercise reversed byte slices, len+1, u64::MAX, invalid byte indexing,
and reversed branded positions. They expect SEM-3 or MEM-14 at the operation's
source site, not merely agreement between engines. Physical owner counts are
outside this test's contract.

`oracle.tsv` was produced by a separate Rust slice/UTF-8 oracle after validating
the pinned fixed vectors. Its raw byte hex and adapted Hirsel expectations are
identical for valid intervals; invalid intervals map to Hirsel failure rules.
Rust has no Hirsel brand semantics: source-only wrong-slice/numeric/forged-brand
rejections and an independent V adversary are separate tests.

Prior art (reopened with whole-file hashes verified):

- rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
  `library/core/src/str/traits.rs:260-284` (safe checked interval),
  `library/alloctests/tests/str.rs:774-791` (boundary/continuation shape),
  `library/core/src/str/validations.rs:146-219` and
  `library/alloctests/tests/str.rs:878-903` (oracle sanity vectors).
- golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
  `src/unicode/utf8/utf8.go:108-128` and `utf8_test.go:31-107`:
  U+0080/U+0800/U+10000 and truncated-prefix shapes. Go's replacement/FullRune
  policy is not imported; Hirsel string.position requires a scalar boundary.

Existing utf8_construction_model_16 construction/position and stale-brand
controls and native text ABI tests remain unchanged. Novel coverage here is
relative indexing of every nested byte interval and helper-return composition,
with each of interpreter, checked H, LLVM and Cranelift checked independently
against the same oracle. See third-party/NOTICES.md for upstream licensing.
