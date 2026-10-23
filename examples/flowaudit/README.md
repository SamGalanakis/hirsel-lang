# Flow capacity auditor

Audit up to four independent capacity scenarios on one directed network. Each
report carries a feasible flow and a minimum cut, so verification does not need
to trust the augmenting-path solver.

From this directory, with the Hirsel CLI on PATH:

```sh
hirsel run hirsel.json --grants grants.json
```

The grants select `audit data network.json report.json 4` and map `data` to
`fixtures`. The bundled Cormen network has maximum value 23 in its base scenario.
For verification, change the first grants argument to `verify` and run again.
A report contains capacities and nonnegative flows in input-arc order, its total
value, residual reachable vertices, cut arc ordinals and cut capacity. A successful
verification establishes capacity bounds, conservation, source/sink value,
residual reachability and equality of flow value and separating cut capacity.
It accepts alternate optimal flows; changing one flow, value or cut can fail
with `flowaudit: witness` and exit 2. Verification never changes the report.

The exact CLI and version 1 schema are in
[SCHEMA.md](../../crates/hirsel-cli/tests/support/flow_audit_36/SCHEMA.md).
Names use UTF-8 bytes. Bounds are 2–16 vertices, 0–64 arcs, capacity at most
1,000,000, 1–4 scenarios and jobs, 128 bytes per name, 64 KiB manifest and
32 KiB report. Duplicate directed arcs, loops and unknown endpoints refuse.
Antiparallel arcs have independent capacities and share antisymmetric net flow.
The solver publishes `max(net_flow, 0)` for each input arc.

Neighbor traversal uses declared vertex order. Bidirectional BFS expands the
smaller frontier, choosing the source on ties. There is no iteration cutoff or
partial maximum. Tasks share immutable topology, own their residual matrix and
results, and publish in scenario order after joining. Publication validates the
complete encoded report before atomic replacement; failures preserve the old
file. `--interruptible` races the workflow with a granted interrupt and joins
canceled work. No clock or artificial timer is needed.

The independent test oracle enumerates all source-containing, sink-excluding
cuts. Tests retain their original boundary inputs and individual outcomes.
See [FRICTION.md](FRICTION.md) for copies, adaptations and qualification limits,
and [LICENSES.md](LICENSES.md) for the upstream algorithm and vector notices.

Qualification uses the fixed case partitions in the support directory. For
example, select `FLOW_AUDIT_36_CASE=classic,pending,denial` when running
`flow_audit_36_reference_checked_workflows` or
`flow_audit_36_native_four_engine_replay` on CLI shard1. The explicit ignored
Ordered test uses `FLOW_AUDIT_36_ORDERED=all` or selected `backend:width:kind`
identities; it requires the supported exact-four CPU pool request. Preserve
per-cell receipts rather than treating a wrapper exit as full qualification.
The original cap/step-limit and short control-observation gaps are documented in
FRICTION; no all-engine success is claimed for those incomplete identities.
