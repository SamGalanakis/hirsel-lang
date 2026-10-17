# hirsel-graph

Pure control-flow graph algorithms over adjacency lists of dense node indices.
Allowed dependencies: none. This crate knows no IR, ownership fact or compiler pass.

`reverse_postorder` and `postorder` preserve adjacency order. `Dominators` uses
Cooper–Harvey–Kennedy iteration and tree intervals for constant-time reflexive
dominance queries. Unreachable nodes neither dominate nor are dominated.
`reachable`, Tarjan `components` and recursive loop-forest `feedback_edges` share
iterative walks; no algorithm uses the native call stack or an iteration cutoff.
All targets and roots must index the supplied adjacency list.

The algorithms are extracted from the LIR validator and RC passes. A caller
chooses its edge policy, including whether dead predecessors contribute loop
entries. Feedback pairs identify every parallel edge between their endpoints.
Property tests compare reachability, dominance, components and feedback edges
with independent transitive-closure references on random graphs, including
irreducible loops, and pin traversal order against a recursive reference.
