# Packed lists

`List[Packed[T]]` stores each element's active variant extent. Access stays
sequential (MEM-13): these operations introduce neither indexing nor element
slots, and cannot change the active tag of an existing element.

`list.truncate(mut xs, n)` leaves `xs` unchanged when `n >= list.len(xs)`.
Otherwise it walks the retained prefix to measure its bytes, reserves a new
sequential destination, walks the same prefix to append its elements, and
replaces the whole list. Other aliases retain their original values. This is
O(n) work with two prefix walks; a nonempty retained prefix needs one new outer
backing allocation, even when the old list is unique. Truncating to zero uses
static empty storage. Shrinking does not promise to retain the original spare
capacity. Packed elements are flat and cannot contain resource finalizers.

`list.filled(n, take seed)` copies a copyable seed into a fresh sequential list.
Zero elements allocate no backing. Nonempty construction reserves using the
minimum encoded variant width, then appends each active encoding with amortized
storage growth. It does not promise one allocation for variable-width Packed
elements. Seed construction and ownership costs must be counted separately.
Checked list capacity overflow raises SEM-1 before invoking the allocator.

With regions disabled and an inline seed, H, LLVM and Cranelift agree:

| Operation | Outer allocations |
| --- | ---: |
| Fill zero elements | 0 |
| Fill one or 64 Big values in Small(u8) / Big(u64, u64) | 2 |
| Truncate to the current length or a larger length | 0 additional |
| Truncate to a nonempty proper prefix, unique or aliased | 1 additional |
| Truncate to zero | 0 additional |

The original 64-Big construction took three allocations. Static empty storage
removes the initial header allocation; minimum-width reservation and later
active-width growth remain. An empty variant can lower the reservation's width
and add further growths. These are physical allocator counters, not timing
claims; no one-allocation fill guarantee is made for Packed storage.

The implementation adapts Rust's reserve-then-fill and prefix-before-retirement
structure (`rust-lang/rust` revision
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`, MIT):
`library/alloc/src/vec/mod.rs:1801–1824`,
`library/alloc/src/vec/spec_from_elem.rs:12–29`, and
`library/alloctests/tests/vec.rs:547–575,2327–2338`.
Hirsel copies flat bytes instead of invoking user Clone/drop callbacks. The
Packed truncate fallback copies into fresh storage because the existing LIR
cannot publish a shortened sequential initialized extent. Fixed-stride lists
retain their separate in-place implementation.
