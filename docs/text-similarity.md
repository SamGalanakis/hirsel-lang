# Bounded text similarity

`std@1/text_similarity.jaro(a, b, max_cells, max_scalars)` and
`jaro_winkler(a, b, max_cells, max_scalars)` return `Result[f64, Error]`.
`Error` has one arm, `TooLarge()`. Both functions are pure and preserve inputs
and their aliases on success and refusal.

Lengths and matching positions count Unicode scalar values, not UTF-8 bytes
or grapheme clusters. No normalization or case folding occurs. Thus `é` and
`e\u{301}` have no matching scalars; the combining mark counts separately.
Both empty inputs score exactly 1; exactly one empty input scores exactly 0.
Identical singleton scalars score exactly 1, different singletons exactly 0.

Each input independently obeys `max_scalars` (unlike edit_distance's combined
cap). Counts stop at the first scalar over the cap, without allocating scalar
storage. Counts are checked before empty fast paths, so an empty partner does
not bypass the other input's scalar cap. Empty inputs use zero matching cells
and require no scratch allocation.

For nonempty inputs of lengths n and m, `max_cells` must cover n*m, including
identical inputs. A division check refuses an overflowing product as well as a
product above the budget. Zero cells therefore refuse every nonempty pair.
Before allocating, preflight also checks n+m and the scratch extents. There
are two char lists (4 bytes per scalar), and a combined bool list (n+m flags,
1 byte each). Each native sequence has a 16-byte prefix; each extent must fit
the runtime's `isize::MAX / 2` ceiling on its 64-bit targets. The aggregate
5*(n+m)+48 byte arithmetic must fit u64. These are representability checks,
not a promise of sufficient host memory. All budget and representability
refusals return `TooLarge`, leaving the inputs unchanged. Scratch storage is
O(n+m); worst-case matching work is O(n*m), with O(n+m) counting, decoding,
and transposition work. `max_cells` bounds matching comparisons, not all CPU
work or allocations. No performance improvement is claimed.

Matching follows strsim's ordered, one-to-one greedy algorithm. The radius is
`max(n,m)/2` with a saturating subtraction of 1. Each left scalar takes the
first equal, unused right scalar within its window. Ordered matched sequences
are compared, and their mismatch count is divided by 2 **as an integer**,
including odd counts. The result is calculated in this order:

```
(matches/n + matches/m + (matches-transpositions)/matches) / 3
```

No matches score 0. Winkler adds `0.1 * prefix * (1-score)` only when the
calculated score is strictly greater than 0.7; prefix counts at most four
initial equal scalars. Floating operations retain the reference order.
Compatibility with this greedy reference is the contract; no universal
mathematical symmetry guarantee is made for repeated-character tie matching.

The ordinary std package manifest registers the module. No generated API
index or existing edit_distance, CLI suggestion, or benchmark API is changed.

## Prior art and adaptations

Ported from [strsim](https://github.com/rapidfuzz/strsim-rs) at commit
`dacc84c0dc61eff0ee0ff66962bcf2e17018ad26`, `src/lib.rs:90-219`.
The audited file SHA-256 is
`dbbee90724d9ed59185d0882c46bfbcaa6ffff4755d29663ee8acc9c542057c1`.
The scalar goldens come from `src/lib.rs:824-984`, including both directions
where upstream declares them, long prefixes, Unicode, and integer transpositions.
The generic integer no-match vector is represented by unequal scalar pairs.
All literal scores retain upstream tolerances (1e-5 for the last long-prefix
case); exact upstream scores retain zero tolerance.

Hirsel adaptations are allocation-free bounded counting, typed `TooLarge`
refusal before allocation, and bounded decoded char lists for indexed scalar
access. A single bool list replaces the two borrowed flag slices, preserving
the reference's combined allocation. Starting each candidate walk at its lower
bound skips only candidates the reference rejects. Saturating radius/bounds
are expressed with explicit integer branches. Strings are borrowed through
ordinary Hirsel parameters, with no mutable inputs. The Winkler boost is a
private helper so exact threshold values can be tested directly. There are
no scoring or normalization deviations.

`jaro_similarity_10` runs the vectors, budget/alias checks, long UTF-8 inputs,
and 1,922 frozen scores for all 31 strings of at most four scalars over `{a,é}`
on the interpreter, H, LLVM and Cranelift. A separate Rust oracle removes
candidates from a pool and sorts match positions instead of using flags or
the port's transposition cursor. It verifies the frozen scores independently.
Additional cases cover three (odd) ordered mismatches, combining sequences,
emoji, NUL, and prefix capping. Direct private-helper checks cover synthetic
n*m/scratch overflow without allocating impossible inputs, exact/one-below
budget boundaries, and the floating values immediately below, at, and above
0.7. Fixtures select this checkout's std through their sidecar manifests.

## strsim license

The MIT License (MIT)

Copyright (c) 2015 Danny Guo
Copyright (c) 2016 Titus Wormer <tituswormer@gmail.com>
Copyright (c) 2018 Akash Kurdekar

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
