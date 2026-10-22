# Fixed-domain bit sets

`std@1/bitset` stores membership in `[0, domain_len)` in dense `u64`
words. `empty(domain_len)` returns a unique, opaque `BitSet` with every
word zero. A zero-length domain is valid. The domain never grows.

```hirsel
let mut selected = bitset.empty(65)
let previous = bitset.insert(mut selected, 64) // Ok(false)
let again = bitset.insert(mut selected, 64)    // Ok(true)
let snapshot = bitset.indices(selected)      // owned [64u64]
bitset.clear(mut selected)
// snapshot still contains 64
```

| Operation | Result | Cost |
|---|---|---|
| `empty(n)` | unique empty set with domain length `n` | O(ceil(n/64)) |
| `domain_len(set)` | `u64` domain length | O(1) |
| `contains(set, index)` | `Result[bool, BitSetError]` | O(1) |
| `insert(mut set, index)` | prior membership, in `Result` | O(1) |
| `remove(mut set, index)` | prior membership, in `Result` | O(1) |
| `clear(mut set)` | unit | O(ceil(n/64)) |
| `count(set)` | `u64` member count | O(ceil(n/64) + members) |
| `union(mut destination, source)` | `Result[(), BitSetError]` | O(ceil(n/64)) |
| `intersection(mut destination, source)` | `Result[(), BitSetError]` | O(ceil(n/64)) |
| `difference(mut destination, source)` | `Result[(), BitSetError]` | O(ceil(n/64)) |
| `indices(set)` | unique ascending `List[u64]` | O(ceil(n/64) + members) |

`IndexOutOfBounds(index, domain_len)` rejects every index at or above the
domain length, including `u64` maximum. Both mutations return the membership
**before** the operation: removing an absent member returns `Ok(false)`;
removing a present member returns `Ok(true)`. Invalid indices return before
reading a word or mutating the set.

Binary operations mutate the destination and borrow the source. They require
equal **domain lengths**, even when the two lengths would occupy the same
number of words. `DomainMismatch(destination_len, source_len)` returns before
any writes. Difference computes destination minus source. Empty-set algebra
with two zero-length domains succeeds. All operations are deterministic.

The representation is opaque. Word count uses `n / 64` plus one for a nonzero
remainder, avoiding an overflowing `n + 63`. Every mutation that can set or
combine bits masks unused final-word bits. The mask runs only for a nonzero
remainder; empty sets and exact word multiples never shift by 64. Clear writes
only zeros. Count and iteration therefore never expose padding bits.

Storage is O(ceil(n/64)) words plus fixed record/list overhead, with list
capacity subject to normal allocation policy. This is dense storage; sparse
membership does not reduce its domain allocation. Huge allocations may fail
through the normal resource policy. `indices` allocates O(members) additional
storage and returns an owned snapshot with no loan of the set. It remains
valid after mutation or destruction of the original set. Scalar counting and
iteration remove one low set bit per step, bounded by 64 steps per word.

## Provenance and adaptations

The implementation ports the layout, quotient/remainder indexing, word algebra
and tail mask from rust-lang/rust commit
`19833af7224424e8b53e288097d56b6af43d5bcf`,
`compiler/rustc_index/src/bit_set.rs:73-180,243-258,320-336,1594-1644`,
under MIT (Rust), retained in `third-party/NOTICES.md`.
The brief's `243-258` pin covers removal and sorted iteration; the actual
last-word mask at this revision is at `1617-1623`.

Adaptations for Hirsel's contract:

- A `u64` domain and opaque unique record replace a `usize` domain and typed
  Rust index marker. Checked errors replace panics/assertions.
- Insert reports prior membership, whereas Rust reports whether it changed
  the set. Remove already reports prior membership in the Rust reference.
- Binary operations return checked unit results rather than a changed flag.
- An owned ascending snapshot replaces a borrowed iterator. Existing portable
  trailing-zero arithmetic and bounded scalar counting replace Rust intrinsics.
- Only the fixed-domain surface is ported; growable, chunked, complement and
  allocation-specialized variants are outside this module.

The corpus fixture ports the empty/membership/union and sparse iteration vectors
from `bit_set/tests.rs:8-87`. Lane-private boundary tests cover domains
0/1/63/64/65/127/128/129, dense and sparse tails, invalid maximum indices,
unchanged destinations on mismatches, and snapshot lifetime. A separate
Boolean-list oracle checks bounded deterministic mutation and algebra traces,
count, ascending uniqueness and domain bounds. The same fixtures execute on H,
LLVM and Cranelift; the longer traces use the Rust harness's explicit step
budget instead of raising the corpus's 1M-step budget.
