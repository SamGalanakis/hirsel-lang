# Owned iterator lookahead

`std@1/peekable.Peekable[T]` owns a core `Iter[T]` and one optional cached
result. `new(take it)` never pulls. The wrapper has three states:

| State | Representation | `fill(mut p)` | `next(mut p)` |
|---|---|---|---|
| Unfilled | `None` | Pull once, cache item or end | Pull once, return item or cache end |
| Item | `Some(Some(item))` | Return `true`, no pull | Move item out, become Unfilled |
| End | `Some(None)` | Return `false`, no pull | Return `None`, remain End |

`peek[T: copyable](p) -> Option[T] in p` observes only the cache. Call `fill`
first: an unfilled cache and an exhausted cache both return `None` from `peek`.
Repeated `fill` or `peek` preserves the same item and makes no producer call.
A failed producer call propagates through the ordinary failure and cleanup path.
An `Iter[Result[T, E]]` still yields `Error` as an item; this wrapper adds no
error handling or fallible-stream policy.

The explicit fill/borrow split uses existing Hirsel ownership rules. A peeked
value borrows from the wrapper, so scope its observation before calling `fill`
or `next` again. Non-copyable payloads support `fill` and `next`; they do not
support `peek`. `next` moves the cached payload using `take`, without a deep
copy. Dropping the wrapper drops both its producer state and any unconsumed
cached payload exactly once.

For example, a token parser can stop before consuming a separator:

```hir
let mut tokens = std@1/peekable.new(iter.from(0u64, token))
let mut total = 0u64
while std@1/peekable.fill(mut tokens) {
  let stop = std@1/peekable.peek(tokens) == Some(59u64)
  if stop {
    break
  }
  match std@1/peekable.next(mut tokens) {
    Some(value) => {
      total += value
    },
    None => failure.check(false, Bounds()),
  }
}
// The semicolon is still cached and is consumed here.
let separator = std@1/peekable.next(mut tokens)
```

The complete executable example is
[`SEM-5-iter-peekable-7-parser-run.hir`](../conformance/SEM/SEM-5-iter-peekable-7-parser-run.hir).
The scoped ownership twin uses a list payload so that the returned observation
retains a real loan; its rejected twins try both `fill` and `next` while that
loan remains live.

## Reference and adaptations

Ported from Rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`:

- `library/core/src/iter/adapters/peekable.rs:13–42`: owned iterator, separate
  empty cache and cached end, consuming the cached item.
- The same file, lines 216–219: lazy one-element lookahead.
- `library/coretests/tests/iter/adapters/peekable.rs:5–33`: repeated peek and
  alternating next vectors for `[0, 1, 2, 3, 4, 5]`.

The reference is MIT/Apache-2.0 dual licensed; this adaptation uses the MIT
license recorded in `third-party/NOTICES.md`. The opened clone's revision was
checked without changing it.

Deliberate adaptations:

1. `fill(mut p)` performs the mutation before `peek(p)` borrows. Hirsel's
   existing borrowed accessor contract requires `T: copyable`; no new
   mutable-to-return borrow rule or iterator trait is introduced.
2. The first `None` permanently fuses the wrapper, including a direct `next`
   before any fill. Rust `Peekable` can resume a non-fused underlying iterator
   after consuming its cached `None`; Hirsel's adapters end at the first `None`.
3. Only `new`, `fill`, `peek` and `next` are provided. Rust's length and
   double-ended assertions are omitted because this wrapper has neither API.

The sequence fixture covers empty input, next before fill, alternating access,
and `Some/None/Some` producers on both fill and direct-next paths. A separate
counted producer returns identical values and records every invocation, proving
construction is lazy and repeated observation/EOF never pulls. Resource fixtures
cover a discarded cache, a payload surviving wrapper destruction, an unpulled
producer, and failure cleanup through both entry points. The driver test checks
that every public helper lowers to an ordinary source native unit.

## Lane files

Implementation and documentation:

- `stdlib/std@1/peekable.hir`
- `docs/iter-peekable.md`
- `crates/hirsel-driver/tests/it-stdlib/iter_peekable_7.rs`

Each case below has an adjacent `.hir.json` launch manifest selecting ordinary
`std@1` sources:

- `conformance/DROP/DROP-2-iter-peekable-7-fill-fail.hir`
- `conformance/DROP/DROP-2-iter-peekable-7-fill-run.hir`
- `conformance/DROP/DROP-2-iter-peekable-7-next-fail.hir`
- `conformance/DROP/DROP-2-iter-peekable-7-next-run.hir`
- `conformance/DROP/DROP-2-iter-peekable-7-owned-run.hir`
- `conformance/OWN/OWN-3-iter-peekable-7-fill-borrowed-bad.hir`
- `conformance/OWN/OWN-3-iter-peekable-7-next-borrowed-bad.hir`
- `conformance/OWN/OWN-3-iter-peekable-7-scoped-run.hir`
- `conformance/SEM/SEM-5-iter-peekable-7-counted-run.hir`
- `conformance/SEM/SEM-5-iter-peekable-7-parser-run.hir`
- `conformance/SEM/SEM-5-iter-peekable-7-vectors-run.hir`

Registration adds one module declaration to the stdlib harness's `modules.rs`,
one source to `stdlib/std@1/hirsel.json`, eleven paths to
`conformance/passing.txt`, and one Rust attribution row to
`third-party/NOTICES.md`. No generated registry is edited.
