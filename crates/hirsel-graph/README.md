# hirsel-graph

Pure graph algorithms over adjacency lists of dense node indices.
Allowed dependencies: hirsel-syntax, for `ContentHash` only. This crate knows no
IR, ownership fact or compiler pass.

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

`scc_keys` gives every node reachable from its roots a `ContentHash` from its local
content and its edges in successor order (design §3, shared by layout keys,
definition hashes and recursive type groups). An acyclic node is a Merkle hash of
its content and its successors' keys. A member of a cyclic Tarjan component, a
self-loop included, hashes the DFS from itself over the component, writing a
member already reached as its discovery ordinal and a node outside as its key.
Keys never depend on node identities or request order, and editing a node changes
the key of every node that reaches it.
