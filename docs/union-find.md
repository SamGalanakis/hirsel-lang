# Bounded disjoint sets

`std@1/union_find` groups dense IDs in `[0, nodes)`. `new(nodes, max_nodes)`
returns `Error(NodeLimit(nodes, max_nodes))` before allocating if the requested
node count exceeds the caller's cap. Zero nodes and a zero cap are valid.
The cap bounds logical nodes (two u64 list entries per node), not allocator
bytes or allocation failure; List growth can reserve spare capacity.

`find(mut forest, id)` returns a representative. `union(mut forest, a, b)`
merges their sets and returns the resulting representative. `same(mut forest,
a, b)` reports equivalence. Invalid IDs return `Error(InvalidId(id, nodes))`;
both endpoints are validated before any compression. When both are invalid,
the first endpoint's error wins. `count(forest)` counts components, including
isolated nodes. Repeated and self unions leave that count unchanged.

Representatives are opaque IDs, not minimum IDs. Equal ranks prefer the first
root; a larger rank wins otherwise. No insertion, deletion or rollback is
provided. The private parent/rank representation prevents unchecked forests.

```hir
pub fn main() -> Result[(), union_find.Error] {
  let mut forest = union_find.new(5, 100)?
  let _ = union_find.union(mut forest, 0, 1)?
  let _ = union_find.union(mut forest, 1, 4)?
  let _ = union_find.union(mut forest, 2, 3)?
  failure.check(union_find.count(forest) == 2, Bounds())
  failure.check(union_find.same(mut forest, 0, 4)?, Bounds())
  let roots = union_find.snapshot(mut forest)
  failure.check(roots[0] == roots[4] && roots[0] != roots[2], Bounds())
  Ok(())
}
```

Construction takes O(nodes) work and storage. Union by rank and iterative
compression give amortized O(alpha(nodes)) parent-table work per find/union
on an unaliased forest; before compression a path has at most floor(log2(nodes))
edges, because rank increases strictly towards the root and a rank-r root has
at least 2^r members. The loop rewrites each visited parent to its grandparent
and advances to the old parent, exactly as rustc does. `count` takes O(1).
`snapshot` performs a find per ID and allocates an owned root list.

Forest values use ordinary Hirsel alias/COW semantics. Mutating one alias
preserves another, and snapshots survive later mutations and forest destruction.
A mutation of an aliased forest may copy O(nodes) storage; these functions do
not promise `@noalloc` or near-constant total cost for such copies.

The implementation ports rust-lang/rust commit
`15c10447a46529b68e38c7f138aeb95240e4d7d6`,
`compiler/rustc_data_structures/src/union_find.rs:28-95`; isolated and transitive
vectors come from `union_find/tests.rs:1-32`. Rust is used under its MIT license
(Copyright The Rust Project Contributors); see `third-party/NOTICES.md`.
Adaptations are dense u64 IDs and parallel Lists instead of typed IndexVec
entries, u64 ranks (bounded by 63), checked ID/cap errors, component accounting,
and Hirsel COW/owned snapshot semantics. The compression update/advance order
and union tie policy are preserved.

The `union_find_9` driver tests run fixed contracts, an independent adjacency
reachability oracle after generated union sequences, and private structural
checks on H, LLVM and Cranelift. Test-only sources share the actual library
module to inspect parents/ranks without adding a public inspection API.
They check domains, acyclicity, rank growth and member bounds, counts, exact
compression order, rejection without mutation, and alias/snapshot isolation.
These are structural correctness controls; no wall-time benchmark is claimed.
