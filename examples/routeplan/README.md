# routeplan

Plan bounded shortest routes in a versioned directed network. Queries can close
edges temporarily, cap the cost, and request one alternative by excluding a
selected edge from the primary path. Parallel and zero-weight edges are valid.

```sh
hirsel run examples/routeplan/hirsel.json --grants examples/routeplan/grants.json
```

The example grants a single read-write `data` directory and uses application Args
`["data","network.json","routes.json","4"]`. Set its host path for your launch
working directory. The application accepts:

```
routeplan ROOT INPUT OUTPUT JOBS [--interruptible]
```

ROOT selects a granted directory; INPUT and OUTPUT are relative paths beneath it.
JOBS is1..4. `--interruptible` additionally requires an Interrupt Signals grant.
It races the complete workflow with a delivered signal, cancels and joins the
loser, and reports `canceled`. It uses no timer.

An input file has this shape (see fixtures/network.json for a complete batch):

```json
{"version":1,"revision":7,"nodes":3,
 "edges":[{"source":0,"target":1,"weight":0},{"source":1,"target":2,"weight":5}],
 "queries":[{"revision":7,"source":0,"target":2,"closed":[],"max_cost":5,"alternative":0}]}
```

Node keys are integers0..nodes-1. Edge IDs are zero-based input ordinals. Each
query revision must match the snapshot revision. `max_cost` and `alternative`
may be omitted or null. Other fields are required. Unknown fields are ignored;
std JSON refuses duplicate object keys and malformed input. Weights and numeric
fields must be nonnegative u64 integers. Empty networks and query batches are
valid when no endpoint is referenced.

The complete batch is validated before search. Closures are query-local; the
next query sees the original snapshot. Closure duplicates are accepted and count
against the limit. An alternative excludes the named input edge only when it
belongs to the found primary path; a valid ordinal absent from that primary is
an `alternative` domain error. Unreachable/overflow primaries have no alternative.

The atomically published report, followed by one newline, has this shape:

```json
{"version":1,"revision":7,"results":[{"primary":{"status":"found","cost":5,"edges":[0,1]},"alternative":{"status":"unreachable","cost":null,"edges":[]}}]}
```

`found` paths are simple. Source=target yields cost0 and an empty edge path.
`unreachable` includes no route within the optional cap. `overflow` is a typed
search refusal when an attempted unsettled relaxation exceeds u64; addition is
checked before comparing the cap. Closed edges and already settled destinations
are skipped before arithmetic. The other path statuses carry null cost and an
empty edge list. Reconstruction independently checks edge continuity, summed
cost, repeated nodes and a node-count traversal guard.

Outgoing edges are examined in input ordinal order. The generic std heap orders
frontier entries by distance and then increasing insertion serial. Equal-distance
relaxations retain the first predecessor; settled predecessors are never changed.
This policy is stable across hash seeds, parallel edges and zero-cost cycles.
Query results are published in input order after all scoped jobs join.

| Resource | Limit |
|---|---:|
| Nodes |64|
| Directed edges |256|
| Queries per batch |16|
| Closure entries per query |128|
| Input bytes |131072|
| Output bytes, including newline |65536|
| Independent query jobs |4|

The output byte cap is defensive: two simple paths per query, at most63 edge
ordinals per path, cannot reach65536 bytes within the other bounds. Supplementary
helper tests exercise exact cap/one-over publication and corrupt predecessors;
they are separate from full-main acceptance.

Exit0 publishes the report and prints `routeplan: N queries` to stdout. Exit2
prints `routeplan: CODE` to stderr for domain, I/O or cancellation errors and
preserves the destination until atomic commit. Missing standard-stream grants
exit3. Publication can succeed before a later standard-stream write fails, as
with ordinary command-line tools.

The implementation adapts NetworkX's Dijkstra frontier/settled-distance/stale-entry
structure and XG/MXG/XG2 test vectors at commit
`9d866a5f447d9fe1c989b31f90c520dc2813ee26`. See LICENSES.md for its BSD-3-Clause
notice. Dense bounded keys replace Python graph attributes; checked u64 overflow
replaces arbitrary integers; input ordinals/insertion serial fix ties; a single
predecessor replaces NetworkX's optional list of equal-cost predecessors.
The bounded edge-array scan avoids a second adjacency representation and costs
at most64*256 inspections per search, plus closure membership checks.

The focused CLI module is `route_planner_33`; its support directory contains
frozen independent vectors, exhaustive-simple-path validation for graphs<=7nodes,
public native Fixture launches, replay refusal controls and cap helper controls.
Use `hirsel.tests=host` for retained external evidence (compilation still uses
the pool), `--env ROUTE_PLANNER_33_EVIDENCE=/absolute/retained/path`, and the named
test filter. Original evaluator cap stops remain explicitly INCOMPLETE; native
oracle expectations are labelled separately. See FRICTION.md for remaining
qualification obligations and exact recovery selectors.
