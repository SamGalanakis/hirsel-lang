# Exact sums: Neal xsum and CPython

`stdlib_exact_sum_vectors.rs` uses the shared `reference_vectors::run` runner
(pa-vector-harness): checked ordinary Hirsel, bounded source batches, no
interpreter step budget. All expected values are fixed IEEE words, independent
of the Hirsel implementation. No production execution limits change.

References were cloned before porting:

- Radford Neal, [xsum](https://github.com/radfordneal/xsum), revision
  `b766b3e104cdbd6769c8df033fb430700fce32dd`.
  [`xsum.h:59-95`](https://github.com/radfordneal/xsum/blob/b766b3e104cdbd6769c8df033fb430700fce32dd/xsum.h#L59)
  defines overlapping signed chunks, 32-bit radix and the 2047-term carry bound.
  [`xsum.c:180-459`](https://github.com/radfordneal/xsum/blob/b766b3e104cdbd6769c8df033fb430700fce32dd/xsum.c#L180)
  normalizes chunks and collapses redundant negative sign chunks;
  [`:852-995`](https://github.com/radfordneal/xsum/blob/b766b3e104cdbd6769c8df033fb430700fce32dd/xsum.c#L852)
  splits a finite significand between two chunks and defers carries.
  These are ported in `stdlib/std@1/superacc.hir`. MIT, Copyright 2015, 2018,
  2021, 2024 Radford M. Neal; see `LICENSE.xsum`.
- CPython, [python/cpython](https://github.com/python/cpython), revision
  `8b7d6c53eee2226b08bed761d5a1742b5adb8507`.
  [`Lib/test/test_math.py:726-745`](https://github.com/python/cpython/blob/8b7d6c53eee2226b08bed761d5a1742b5adb8507/Lib/test/test_math.py#L726)
  contains 13 fixed `testFsum` rows; `:747-752` appends the telescoping row.
  All 14 are included, including the two 1000-term series, 1024-term partials
  resize and 1001-term telescoping cases. PSF-2.0; see `LICENSE.cpython`.

## Imported tables

| Input file and upstream location | Rows before | Rows now | Assertions now |
| --- | ---: | ---: | ---: |
| `xsum-check.c:62` one_term | 0 | 41 | 164 |
| `xsum-check.c:110` two_term | 0 | 169 | 676 |
| `xsum-check.c:287` three_term | 0 | 18 | 72 |
| `xsum-check.c:311` ten_term | 0 | 14 | 56 |
| CPython `testFsum` fixed table and telescoping case | 0 | 14 | 28 |
| binary32 versions of all xsum rows + seeded oracle rows | 0 | 242 + 256 | 1992 |

`xsum.json` preserves every table row with its table name and zero-based index.
The original C initializer expressions were evaluated with a C99 compiler at
`-O0` into binary64 words (including each expression's intermediate rounding).
One-term expectations are the input, two-term expectations are correctly rounded
addition, and three-/ten-term expectations are the published fourth/eleventh
number. An independent Python integer sum, in units of the least subnormal,
checks every finite expected result. JSON preserves exact input/output words,
including denormals, overflow and special values.

`cpython-fsum.json` records the fixed table's original binary64 inputs and
published results. Comprehensions are expanded completely, with no shortened
series. The telescoping inputs use the original Python `1.7**i` expressions.

`exact-sums-f32.json` converts all 242 xsum input rows to binary32, and includes
256 random rows (Python `random.Random(20261020)`, lengths 1..24, uniform raw
32-bit input words). Expected results sum the signed integer significands at
exact binary exponents, retain 24 significant bits, and apply integer
round-to-nearest/ties-to-even, with subnormals, overflow, canonical NaNs and
Hirsel signed zeros. There is no binary64 intermediate rounding of the sum.

Each xsum and binary32 row runs in both orders and with both signs, following
`xsum-check.c:638,717,824,870`'s negated table tests. CPython rows run in both
orders. Counts are asserted on the host. Eight named shards per precision and
one CPython test cover every ordinal, including remainders. A further test
checks 35 combinations (70 assertions) of carry counts (2046/2047/2048/4095/8193) and bit shifts
(0/1/31/32/33/2044/2045) against independently computed sum words, followed by
full positive/negative cancellation with a surviving least-subnormal term.
The existing 256 rational binary64 vectors (512 checks) and binary32 corpus
oracles remain in place.

## Deviations and reasons

1. **68 signed chunks instead of 67.** Preserve the existing contract for every
   u64-sized Array: 2098 value bits plus 64 count bits need 2162 bits. A 68-chunk
   radix-2^32 representation covers 2176 bits. No change to public APIs or ABI.
2. **The binary point starts at the least subnormal bit**, using the existing
   precision-specific decomposition (`raw_exponent - 1`, subnormals at zero).
   Upstream starts one bit lower. This permits a single kernel for binary32
   and binary64 without converting binary32 sums through binary64.
3. **Final rounding is parameterized by precision.** Retain Hirsel's integer
   guard/sticky extraction and one ties-to-even rounding; normalize a negative
   magnitude with the same carry routine first. Upstream `xsum_small_round`
   (`xsum.c:1124-1466`) returns binary64 and has separate positive/negative
   branches. Hirsel needs direct binary32 rounding as well. No floating-point
   cast is used to locate the top bit.
4. **Special-value flags remain in f32/f64**, with Hirsel's canonical NaN for
   NaNs/opposing infinities, and negative zero only for a nonempty all-negative-
   zero input. Upstream chooses the greatest NaN payload and returns positive
   zero for exact zero. Corresponding expected zero/NaN words are adapted;
   every numeric term is preserved. CPython's overflow/opposing-infinity
   exceptions become infinity/canonical NaN, as the existing Hirsel API requires.
5. **Portable scalar carry/add loops** replace upstream SIMD, unrolling, debug
   printing and build-time optimization switches. Hirsel source has no SIMD
   intrinsics or preprocessor; the two-chunk add and carry cadence are preserved.
6. **Scope is the four requested sum tables and fixed CPython vectors.** xsum's
   dot-product, division, accumulator-merge and repeated/scaled campaigns target
   APIs Hirsel does not expose; they are not imported. CPython's dynamic
   FloatLike, invalid iterable/type and iterator-exception cases do not apply to
   `Array[f32/f64]`. Its unseeded randomized `msum` campaign is replaced by the
   existing seeded Fraction vectors and new seeded integer binary32 oracle.
7. **Fixed tables are represented as IEEE-word JSON**, rather than executable C
   and Python expressions, so the normal Rust harness runs without a C compiler
   or Python dependency. IDs, counts and published expectations are retained;
   the source batches use the shared harness rather than corpus step budgets.

The new suite adds 18 Rust tests and 3058 Hirsel assertions in total.
