# Substring prior-art realignment

The shared `@noalloc` kernel is adapted from Rust std revision
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
`library/core/src/str/pattern.rs:1362-1633,1735-1770` (MIT).
It retains both maximal-suffix orders, critical factorization, exact short
period versus long-period bound, short-period memory and the 64-bit byte
fingerprint. Split and replace prepare the needle once and reuse that plan.
Find, contains and split_once use the same kernel. Bytes replacement shares
it, including arbitrary invalid UTF-8 and slice-relative offsets.

References were cloned from rust-lang/rust and golang/go, then checked out
at the revisions pinned in NOTICES. All source citations below use those pins.

## Corpus vectors

Rust `library/alloctests/tests/str.rs`:

- `54-100`, `test_find_str`: all 18 fixed checks and all 253 scalar-aligned
  nonempty substrings of `Việt Namacbaabcaabaaba`; both first and last match
  offsets are checked (506 checks). The long loop is partitioned into eleven
  corpus programs to stay within the 1M-step execution budget.
- `245-264`, `test_starts_with` / `test_ends_with`: all 14 checks.
- `1484-1510`, `test_rsplit` / `test_rsplitn`: all three patterns each.
- `1524-1533`, `test_rsplit_once`: all eight checks, plus empty and Unicode.
- `1536-1540`, `test_split_whitespace`: the multilingual whitespace vector,
  plus Unicode whitespace and empty-input controls.
- `1566-1586`, `test_splitator`: all 14 rows.
- `1708-1730`, `test_rsplitn_char_iterator`: all four char/predicate rows.
- Existing split_once, contains, lines and replace corpus cases stay enabled.

Go `src/bytes/bytes_test.go:1770-1790`, revision
`ebe2feefc400f813aefdc8a036e66d2619da6649`: every ReplaceTests row.
The pinned table contains **19**, not the audit's 20. A twentieth row covers
invalid UTF-8 and NUL bytes. Original finite limits and expected outputs are
retained in comments beside their adapted checks.

Additional differential corpus tests compare both search directions and every
start offset (including EOF and beyond EOF) with a naive byte oracle over all
binary words through length three. Periodic adversarial cases exercise both
period branches, fingerprint collisions, repeated matches and slice boundaries.

## Deviations and reasons

1. Hirsel's `StrPos` ties offsets to a source: string wrappers convert the
   kernel's byte offsets using `string.position` / `string.byte_offset`.
   Bounds-checked Bytes indexing replaces Rust's unsafe slice accesses.
2. The kernel returns matches only; Rust's rejected-range / UTF-8 iterator
   machinery has no corresponding consumer in these Hirsel APIs.
3. Reverse operations run the same kernel over virtual reversed byte views,
   rather than carrying Rust's separate backward state and known-period
   factorization optimization. This keeps one shared implementation without
   allocating reversed copies; factorization remains linear in needle length.
4. Plans hold scalar preprocessing only. Each `find_from` starts fresh memory,
   as Rust resets memory after each nonoverlapping match; callers own cursors.
5. Hirsel split (including new reverse split) requires a nonempty delimiter;
   existing failure behavior is preserved. Find and split_once accept empty
   patterns. String replace inserts at UTF-8 scalar boundaries as before.
6. Hirsel bytes.replace rejects empty patterns and has no replacement limit.
   Go's empty-pattern rows therefore assert byte failure and check insertion
   through string.replace; finite-limit rows check replacement of all matches.
   Raw bytes still support invalid UTF-8. No new limited-replacement API is added.
7. Rust char and equality-predicate reverse split vectors use equivalent
   single-scalar String delimiters: Hirsel has no generic Pattern trait or
   double-ended split iterator. Returned lists keep the specified field order.
8. Reverse search/split and Unicode split_whitespace are added as ordinary
   source APIs to run the audit's previously unsupported vector families.
   Unicode splitting uses the existing Unicode16 White_Space property table.
9. Rust's architecture-specific memchr/SIMD dispatch is not ported: Hirsel
   exposes portable byte indexing, so this is the scalar Two-Way algorithm.
10. Rust allocation/capacity/pointer-identity checks are inapplicable to Hirsel's
   ownership API and are replaced by existing ownership/lifetime corpus twins.

## Validation

Targeted Buck2 runs cover the new corpus fixtures, established text/byte cases
and their ownership/lifetime twins, representative HTTP/TOML/YAML/URI/shlex
importers, and the new driver scaling regression. Representative new fixtures
also run through the native LLVM/Cranelift matrix. A driver test doubles both
haystack and needle over four sizes and checks interpreter step growth stays
below the quadratic ratio, including fingerprint collisions and left mismatches.

Fixtures keep Hirsel's explicit ownership rules: repeated split inputs are
fresh copies, expected lists copy a still-live source, and lists reversed in
place are explicitly unique. The complete substring and oracle vectors are
partitioned into bounded programs rather than increasing their step budget;
the driver scaling regression retains inputs through 4096 haystack bytes.

Validation results: 44/44 selected corpus cases passed, including all 23 new
fixtures and representative codec importers. The final reverse-only source
adjustment passed its selected corpus case again. Both driver tests passed:
the new linear interpreter-step regression and existing ordinary-source/native
split ownership check. All four representative new programs passed the LLVM
smoke configurations, real-Io record/replay, runner/executable byte comparison,
and Cranelift parity. The std precheck passed at the unchanged 2 GiB guard.

The reverse splitter keeps the zero-count case inside a conditional and returns
its list once: an early empty-list return exposes the already recorded RC
importer State::live slot panic. The remaining-count loop follows Rust
`core/src/str/iter.rs:937-967`; results and allocation semantics are unchanged.
