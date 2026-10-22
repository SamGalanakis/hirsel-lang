# Finite dependency ordering

`std@1/toposort.order(node_count, edges, max_nodes, max_edges)` returns
`Result[List[u64], GraphError]`. `Edge(from: a, to: b)` requires `a` before
`b`; vertices are exactly `0..node_count`, including isolated IDs. Both caps
are inclusive caller limits. An empty graph returns `Ok([])`.

Counts are validated first (nodes, then edges), followed by all endpoints in
input order (from, then to), before graph storage or indegree additions.
`NodeLimit(count)` and `EdgeLimit(count)` carry the rejected count;
`Endpoint(edge_index, vertex)` identifies the first invalid endpoint.
Caller-owned input storage is outside these allocation checks. Caps bound work
and storage, rather than guaranteeing the host can allocate the supplied size.

Initially ready vertices enter the queue in ascending ID order. Newly ready
vertices append in input edge order. This FIFO policy is deterministic for the
same input; it does not promise a lexicographically smallest order. For example,
three vertices with only `0 -> 1` yield `[0, 2, 1]`. Duplicate edges contribute
one indegree increment and decrement each; each vertex is emitted once.

`Blocked(vertices)` contains every vertex left when the queue empties, in
ascending ID order. It is a blocked set, not a cycle witness: with `0 -> 1`,
`1 -> 0`, `1 -> 2`, vertex 2 is blocked too.

A build tool can number its steps (`0 = generate`, `1 = compile`, `2 = package`):

```hirsel
let dependencies: List[toposort.Edge] = [
  toposort.Edge(from: 0, to: 1),
  toposort.Edge(from: 1, to: 2),
]
let steps = toposort.order(3, dependencies, 100, 1000)?
// steps is [0, 1, 2]; the tool can now execute these numbered steps.
```

The implementation uses O(V + E) time and storage: edge indices form append-only
successor chains, with the input length as their sentinel. The output list is
also the ready queue, consumed through an advancing head. No repeated front
removal, nested graph copies, sorting, recursion or priority queue is needed.

The port follows CPython `8b7d6c53eee2226b08bed761d5a1742b5adb8507`,
`Lib/graphlib.py:56–85,125–135,189–200` (successors, predecessor counts and
ready transfer) and `Lib/test/test_graphlib.py:39–137` (chains, independent
components, duplicates and cycles), under PSF-2.0; see third-party/NOTICES.md.
Adaptations: dense integer IDs replace hashable keys; explicit caps and endpoint
validation bound the finite call; linked edge indices preserve adjacency order;
one-shot FIFO replaces concurrent `get_ready`/`done`; a sorted blocked set
replaces DFS cycle extraction. The diamond and downstream-tail cases extend
the upstream vectors to exercise the specified FIFO and blocked-set contracts.

The private Rust harness checks all 531 directed graphs on zero through three
vertices, including loops, against exhaustive permutations for order existence
and independent transitive closure for blocked vertices. Every successful
result is checked for unique vertices and forward edges. Fixed controls and
instrumented source twins check determinism, caps, maximum endpoints, and exact
vertex/edge visit and enqueue counts. All controls run on the interpreter,
H, LLVM and Cranelift, without a timing benchmark. The instrumented twin is
created from the current module source and does not add a public metrics API.
