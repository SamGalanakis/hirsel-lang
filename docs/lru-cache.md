# Fixed-capacity String-key LRU cache

`std@1/lru` stores owned data values under String keys. Capacity counts
entries, not bytes. Keys, nested values, and values returned to the caller
can have arbitrary byte sizes.

- `new[T](max_entries)` returns `Ok(unique Cache[T])` for positive
  capacity, or `Error(CapacityError(requested: 0))`.
- `put(mut cache, take key, take value)` consumes a unique key and value. It returns
  `Inserted`, `Replaced(entry)`, or `Evicted(entry)`. Both entry results own
  the previous key and value. An existing key becomes most recently used.
- `peek[T: copyable](cache, key)` returns `Option[T] in cache`, a view
  tied to the cache, without changing recency. It uses the same copyability
  constraint as deque/priority_queue observations. `copy` shares a borrowed
  view here; it does not deep-copy a payload. End this view before mutation.
- `touch(mut cache, key)` reports presence and makes a hit most recently
  used without reading or copying its payload.
- `remove(mut cache, key)` returns an owned `Option[unique Entry[T]]`,
  clearing the key/value owners from the slot. The result outlives the cache.
- `len`, `capacity`, and `slot_count` report live entries, the fixed limit,
  and the allocated logical slots (including reusable empty ones).

Noncopyable data works with put, touch, and remove. There is no implicit
payload copying and no ordinary get that changes recency. An optional
get_copy is deliberately omitted; programs can use a scoped peek followed
by touch when they need distinct observation and recency operations.

## Storage and invariants

A standard String map points to reusable arena slots. Links are slot+1;
zero is absent. Head is MRU, tail is LRU. Empty caches have both endpoints
zero. Each live slot has one map entry and exactly one position in the
bidirectional list. Endpoint outward links are zero; adjacent links agree.
Every empty allocated slot appears exactly once in the free list, has no
entry, and has zero links. No free slot retains a key or payload owner.

Lookup returns a scalar before links change. Unlink updates both neighbors
and endpoints; detach removes the map entry, unlinks the node, replaces its
entry with None, and adds the index to the free list. Insertion consumes a
free slot first and grows only when there are fewer allocated slots than
capacity. Replacement and eviction detach before inserting. Thus live
payload count, slot backing, map entries and free indices are O(capacity),
independently of operation count. The underlying collections may reserve
more machine storage than their logical lengths; their geometric backing
still remains O(capacity). Ordinary operations use map lookup and constant
link edits; hash collisions retain the standard map's behavior.

## Prior art and deviations

Opened reference pins were rechecked read-only:

- LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`,
  `rust/src/benchmarks/cache_simulation.rs:5-158`: HashMap, arena, free list,
  doubly linked recency structure. Its `test.js:108-115` identifies the
  existing cache workload checksum 179773224; that benchmark and checksum
  remain unchanged and are not used as this module's correctness oracle.
- BuildBuddy `51a570d9a2e641262e5912b6ccff7336b1b734ad`,
  `server/util/lru/lru_test.go:40-103,129-158`: independent insertion,
  replacement, and oldest-entry controls, adapted from weighted capacity
  to unit-count capacity. Touch/peek controls supplement these vectors.

Hirsel adaptations: unsigned zero-sentinel links replace Rust Option<usize>;
owned optional entries clear removed storage; explicit result variants
transfer displaced values instead of callbacks or silent destruction;
peek and touch separate borrowing from mutation; positive capacity is
checked. Values are generic owned payloads, including nested data and noncopyable opaque owners.
The payload type is unbounded because the current `data` bound implies
`copyable`; restricting Cache to data would exclude the requested noncopyable
ownership API. Only peek requires copyable, matching deque and priority_queue.
The map and arena each own their key, so insertion explicitly copies key
bytes; payload bytes are never copied. A borrowing helper creates that key
owner without consuming the unique input key.
There is no TTL, weighted capacity, callback or concurrency surface.
A small success helper receives the opaque cache before constructing Result,
preserving its declared cleanup summary across the constructor boundary.
No language, IR, checker or runtime ABI changes are involved.

## Validation

The lane-private Rust harness runs the ordinary std package's independent
vectors, nested noncopyable ownership, and ended-borrow controls through
H, LLVM, and Cranelift. Two compile-refusal fixtures check a live peek
against mutation and a noncopyable payload against peek.

The model fixture runs 3,600 deterministic operations (three capacities
1/2/7, four seeds, 300 operations each) against a simple ordered vector.
Every operation compares result kind and displaced contents, all eleven
possible key values, exact MRU-to-LRU key order, live count and slot bound.
It also audits the actual source module's map/list/free-list invariants,
including None storage in every free slot, through a test-only function in
the same module. The manifest compiles the original lru.hir directly; it
does not copy the implementation or export the audit in std. Complete
drains audit every removal. This larger case uses a bounded Rust harness
rather than increasing the source corpus's 1M-step cap.

## Reference license notices (MIT)

LangArena: Copyright (c) 2026 'Konstantin Makarchev'

BuildBuddy: Copyright (c) 2020-present Iteration, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to
do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Verified commands and inventory

`root//:hirsel-driver-test-it-stdlib-shard-1` with
`-- --test-arg=lru_cache_7` executes five Rust tests: four positive fixtures
on H/LLVM/Cranelift and one test checking both compile refusals. Native
fixtures use standalone builds with every-stage validation and distinct
output directories, avoiding the run helper's process-wide scratch directory.
The model's borrowed slot inspection stays within small helper calls so its
payload reads always have an active native loan.

The final focused run passed 5/5 tests. The first execution run passed 1/5
(the refusal controls) and failed the other four at the constructor boundary;
these were implementation bring-up results, not a baseline suite regression.
The selected conformance run passed 2/2 cases (both matched and executed),
after an initial 1/2 at that same constructor boundary. No tests were repeated
just because main moved.

```sh
.buck2/bin/buck2 test root//:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=lru_cache_7
.buck2/bin/buck2 test -c hirsel.conformance_paths='conformance/SEM/SEM-5-lru-cache-7-|conformance/MODE/MODE-1-lru-cache-7-' root//:gate-conformance-selected-shard-1
```

New fixture paths under `crates/hirsel-driver/tests/fixtures/lru-cache-7/`:
`vectors.hir`, `model.hir`, `owned.hir`, `peek-ended.hir`,
`peek-live-bad.hir`, `noncopy-peek-bad.hir`, each with a `.hir.json` manifest;
`payload.hir` and `payload.hir.json` define the external noncopyable owner;
`audit.hir` augments only the test module. The model manifest selects the
actual std source in its own module without a second std dependency, since
a source path has one module context per selected package graph.

New corpus paths: `conformance/SEM/SEM-5-lru-cache-7-run.hir` and
`conformance/MODE/MODE-1-lru-cache-7-peek-bad.hir`, each with a `.hir.json`
manifest. Registration uses `conformance/passing.txt`, the std source manifest,
and the Rust harness's union-merged `modules.rs` only. Reference notices are
registered in `third-party/NOTICES.md`.
