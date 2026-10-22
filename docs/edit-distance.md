# Bounded scalar edit distance

`std@1/edit_distance.levenshtein(a, b, max_cells, max_scalars)` returns
`Result[u64, edit_distance.Error]`. Insertions, deletions and substitutions of
Unicode scalar values cost one each. There is no transposition, normalization
or grapheme segmentation: `"é"` versus `"é"` has distance 2; `"👩‍💻"`
versus `""` has distance 3. Inputs remain available after either result.

Limits are inclusive. For scalar lengths `m` and `n`:

1. Count the first input, then the second against the remaining scalar budget.
   Return `Error(ScalarLimit())` as soon as `m+n > max_scalars` is known,
   without storing decoded scalars. This takes at most `max_scalars+1`
   decoded scalars on refusal (the expression need not be representable).
2. If either input is empty, return the other length, using zero DP cells and
   no scalar buffers or DP row. Thus two empty strings succeed with both
   budgets zero. One nonempty input still needs its scalar budget.
3. For nonempty inputs, check `m > max_cells / n`. Return
   `Error(CellLimit())` if true, before allocating any buffers or DP row.
   No unchecked `m*n` or `m+n` calculation is used for these checks.
4. Decode both inputs into scalar buffers and compute the exact result.

Scalar refusal always precedes cell refusal. Equality still needs the full
cell budget: `levenshtein("a", "a", 0, 2)` refuses with `CellLimit()`.
`levenshtein("kitten", "sitting", 42, 13)` returns `Ok(3)`; budgets
`(41,13)` and `(42,12)` yield `CellLimit()` and `ScalarLimit()` respectively.
Failure never represents an approximate distance or an edit-distance cutoff.

Time is O(m+n+m*n). `max_cells` limits only the DP product, **not all CPU
work**: counting, decoding and row initialization also take work. Storage is
O(m+n) decoded scalars plus exactly `min(m,n)` DP words (u64), apart from
container overhead and allocator capacity rounding. `max_scalars` bounds
decoded element counts, not allocator bytes or the caller's existing UTF-8
storage. Host allocation failure remains subject to normal language/runtime
resource handling; the two error arms describe these caller budgets only.

The implementation structurally reserves each scalar buffer to its counted
length, pushes exactly that many scalars, and reserves/pushes exactly the
shorter length for the single DP row. No matrix, extra row or per-cell
allocation is used. Both nonempty lengths satisfy `m+n <= u64::MAX` after
preflight. Each DP candidate is at most `max(m,n)+1 <= m+n`, which also
proves the recurrence's increments cannot overflow.

## Ranked suggestions

Tie ordering belongs to the caller. This example orders equal distances by
the original candidate index, making the result deterministic. Each result
contains that index and its distance, so the caller can retrieve candidate text. A refused
candidate stops the operation with the same explicit error.

```hirsel
pub type Suggestion {
  distance: u64,
  order: u64,
}

@noalloc
fn compare(a: Suggestion, b: Suggestion) -> i32 uses {} {
  if a.distance < b.distance {
    return -1
  }
  if a.distance > b.distance {
    return 1
  }
  if a.order < b.order {
    return -1
  }
  if a.order > b.order {
    return 1
  }
  0
}

pub fn suggestions(query: String, candidates: List[String]) -> Result[
  List[Suggestion],
  std@1/edit_distance.Error
] uses {} {
  let mut ranked: unique List[Suggestion] = []
  let mut order: u64 = 0
  for candidate in candidates {
    let distance = std@1/edit_distance.levenshtein(query, candidate, 4096, 128)?
    list.push(mut ranked, Suggestion(distance: distance, order: order))
    order += 1
  }
  list.sort(mut ranked, compare)
  Ok(take ranked)
}
```

For query `"cat"` and candidates `["bat", "cut", "cat"]`, the order is
`["cat", "bat", "cut"]`, with distances `[0,1,1]`. Budgets apply per call;
the caller separately bounds the number of candidates and result storage.

## Provenance and verification

Ported from [strsim-rs](https://github.com/dguo/strsim-rs) at
`dacc84c0dc61eff0ee0ff66962bcf2e17018ad26`, `src/lib.rs:229–266` (one-row
recurrence and scalar wrapper) and `src/lib.rs:985–1026` (fixed vectors).
The pin was rechecked directly against the opened reference clone.

Deviations: explicit scalar/cell preflight and typed refusal implement the
bounded API; scalar buffers permit indexed Hirsel list access; swapping the
row dimension selects the shorter input; u64 distances replace Rust usize.
The recurrence and exact scalar costs are retained. No language, IR or runtime
ABI change is involved.

The lane's Rust tests compare against an independent full-matrix oracle.
The exhaustive corpus fixture contains all 169 ordered pairs of strings of
length at most two over `[a, é, 💚]`; Rust verifies every literal expectation
against that oracle, and the same fixture runs on H, LLVM and Cranelift.
Fixed vectors, inclusive budgets, refusal precedence, large-input early
refusal and strongly asymmetric inputs cover the remaining boundaries.

### Upstream MIT license

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
