# Collection references and adaptations

The references were cloned and inspected at the revisions in
[the notices](../third-party/NOTICES.md). These are the source locations used
by this change, with adaptations recorded explicitly.

| Collection | Reference and structure | Vectors |
|---|---|---|
| Map/Set | indexmap `41a87088`, `src/inner.rs:21-38`; CPython `59942e24`, `Objects/dictobject.c:35-88,2934-2973`: ordered entries, separate hash index, dummy deletion | indexmap `tests/quick.rs:82-115`: contains, insert/remove and insertion order, compared against `ordered_map`; drains at 2,000 and 8,000 entries count interpreter row work |
| Deque | Zig `bff490a2`, `lib/std/deque.zig:15-29,68-283`: head, length, wrapped ring, growth in logical order | `deque.zig:374-517,634-710`: basic, buffer endpoints, slow growth, slices, iteration and List oracle |
| Priority queue | Zig `bff490a2`, `lib/std/priority_queue.zig:17-82,138-176`: stored comparison, peek, bottom-up heapify, shared sift | `priority_queue.zig:294-483,654-674`: min/max, duplicates, empty, three elements, odd indices, empty/singleton/full heapify, sift-up removal |
| Heap kernels | Zig `priority_queue.zig:59-78,138-156` | `priority_queue.zig:371-386,654-674`: direct upward repair and downward draining |
| Ordered map | Rust `f6c1107`, `library/alloc/src/collections/btree/map/tests.rs:571-929` | All nineteen named `test_range*` scenarios have corpus counterparts; both fold directions are checked |

The following deviations preserve Hirsel's ownership and API semantics:

- Map/Set retain the existing seeded hash, tagged buckets, 7/8 load threshold
  and lazy compaction policy. CPython's pointer/null layout and indexmap's
  swap removal cannot preserve Hirsel's owned values and insertion order as
  they stand. Set uses Map's optional `(key, unit)` entries and shared
  maintenance. Native descriptor version 13 and codec program version 4
  identify the layout change; encoded user values keep their existing wire
  format. The interpreter uses its existing host hash index over optional
  entries, with identical live-entry and insertion-order semantics.
  The corpus model uses one short seed to fit the one-million-step budget;
  the Rust driver runs eight seeds with 200 operations each.
- The List oracle runs deterministic operation streams rather than Zig's
  byte-driven fuzzer and allocator. Hirsel has no injected allocator or
  bounded-buffer API. Slice operations are endpoint pushes; iterator values
  are observed through indexed access and draining. Allocation-failure
  injection, exact capacity and pointer alias assertions are inapplicable.
- Ring slots are `Option[unique T]`, rather than uninitialized memory.
  Growth doubles from four slots and moves elements into a preinitialized
  buffer. This preserves ownership and keeps endpoints amortized constant.
  `get`, `front`, `back` and heap `peek` copy only copyable values; owned
  resources leave through the pop operations.
- The heap comparator is a unique, no-allocation, capability-free callback.
  Construction takes an ordinary owned callback and makes it unique once,
  including captured context; `push` and `pop` accept no comparator.
  Zig's arbitrary-index removal regression runs directly on the shared
  kernels because the public heap has no arbitrary removal API. Iterator,
  allocator shrink and value-update APIs are not added; their storage or
  update-specific assertions are outside the existing heap surface.
- The heap kernel retains the shared heapsort convention: downward sift is
  a max heap, upward sift a min heap. Priority queues reverse arguments for
  downward sift, avoiding overflow from negating `i32::MIN`.
- Rust's range iterators become `fold_range`, with copied observations.
  Hirsel has no mutable range iterator or heterogeneous `Borrow` key API;
  the mutable case checks the same entries and tree validity, and String
  bounds use ordinary borrowed String arguments. The root-boundary test
  uses Hirsel's degree 16 (32 inserts), rather than Rust's degree 6.
- Backwards and equal-excluded bounds yield an empty predicate intersection,
  as the current fold API specifies, rather than Rust's panic. The cyclic
  comparator case checks this empty result. Rust's state-changing `Ord`
  cannot be expressed by a `uses {}` comparator; a pure reverse ordering
  checks consistent bound traversal instead. The exhaustive 200-key pair
  enumeration runs in an unbounded Rust driver test; the corpus counterpart
  uses Rust's reduced Miri sampling to stay within its step budget.
