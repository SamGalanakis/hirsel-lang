# Heap executor fixtures

Each `<name>.broken.lir` is a deliberately broken post-RC program and
`<name>.fixed.lir` its corrected twin, in the canonical native snapshot format
(`hirsel_lir::snapshot`, a `heapcheck-program`). Both twins print the same
bytes; only the heap audit tells them apart. `<name>.fixed.trace` is the
corrected twin's ownership trace.

`crates/hirsel-heapcheck/tests/it/heap.rs` builds every pair, checks it against
these files, decodes the files and runs the decoded programs. Regenerate with
`tools/bless.sh heap` and review the diff.

| Fixture | Broken twin | Violation |
|---|---|---|
| omitted-decrement | the box is never released | LeakedCredit |
| stale-reuse-generation | a plain alias is read after its block was reset and reused | StaleGeneration |
| uninitialized-scan | a moved-out field is released again by an all-live mask | UninitializedScan |
| duplicate-finalizer | an explicitly finalized resource is released whole | DuplicateFinalizer |
| leaked-token | a reset token is neither reused nor disposed | LeakedToken |
| borrowed-store | a borrowed parameter is stored into a new block without a credit | BorrowedStore |
| region-escape | a heap block keeps a reference into a region past its exit | RegionEscape |
| view-after-detach | a view of `p.a` is used after a sibling write detached `p` | ViewDetached |

The moved String/Bytes variant source regression lives in
[DROP-2-variant-payloads-joined-cleanup-run.hir](../../conformance/DROP/DROP-2-variant-payloads-joined-cleanup-run.hir).
It is compiled fresh by conformance and the native matrix; the hand-built
partial-move controls remain in `hirsel-heapcheck/tests/it/moved_variant.rs`.
