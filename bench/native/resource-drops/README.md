# Real destruction row B: open draft, no timing claim

The source-model row A is measured separately with the original sealed C/Rust
references and `programs/resource-failure-{throughput,heavy}.hir`.

Row B needs real Hirsel failure isolation (`task.isolate`) and a way for drops
to report into a result without shared mutable state. The minimal correctness
control is `../programs/controls/resource-drops.hir`: two resources print `b`,
then `a`, before the caught failure. It is a correctness control, never a timing
row. The C `goto cleanup` and Rust `Drop` drafts here preserve reverse cleanup
on both success and failure-result exits; they are not sealed timing baselines.

The larger Hirsel drafts in `drafts/` show the missing result-observation seam.
Their optional stdout trace can validate order. Disabling the trace leaves
empty finalizers that C/Rust can erase, so those drafts cannot measure effectful
destruction. Always logging would swamp destruction with stdout cost. The owner
rejected both timing interpretations in reply to 325654. No row B timings,
ratios, pass or allocator/arena claims are made.

The drafts leave every phase-1 reference source and input unchanged. Their
package pins retain Rust 1.99.0, mimalloc 0.1.39 and libmimalloc-sys 0.1.35;
there is no compiler or runtime shim.
