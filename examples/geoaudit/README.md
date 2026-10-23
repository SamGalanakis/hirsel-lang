# geoaudit

Audit sampled locations against bounded polygons on a Cartesian integer grid.
This is not latitude/longitude or continuous crossing detection. A transition
means only that two consecutive samples have different classifications.

```
geoaudit ROOT INPUT OUTPUT JOBS [--interruptible]
```

Run `hirsel run examples/geoaudit/hirsel.json --grants examples/geoaudit/grants.json`.
ROOT selects a granted directory; INPUT/OUTPUT are relative paths beneath it.
JOBS is 1..4. Interruptible mode requires Signals and races the complete workflow
against a delivered Interrupt; scoped cancellation joins the losing workflow.

Input version 1 has required `regions` and `tracks` arrays. Each region has an
`outer` ring and `holes` array. A ring is an array of `{ "x": 0, "y": 0 }` points
with its first point repeated at the end. Either orientation is accepted.
Each track has a `samples` array; duplicate samples and empty tracks are allowed.
Regions may overlap and retain declaration order. Empty region/track arrays are
valid. Unknown fields are ignored; duplicate keys, noninteger coordinates and
malformed JSON are refused by std JSON/derived decoding.

```
{"version":1,"regions":[{"outer":[{"x":0,"y":0},{"x":2,"y":0},{"x":2,"y":2},{"x":0,"y":2},{"x":0,"y":0}],"holes":[]}],"tracks":[{"samples":[{"x":1,"y":1},{"x":0,"y":1}]}]}
```

The atomically published compact JSON has sorted keys and one final LF:

```
{"results":[{"regions":[{"classes":["inside","boundary"],"transitions":[1]}]}],"version":1}
```

Results follow track order, then region order, then sample order. Transition
indices name the later sample (zero-based). Outer/hole boundaries both classify
as `boundary`; hole interiors as `outside`. No edge between distant samples is
examined: two outside samples can pass over a region without a reported change.

| Resource | Bound |
|---|---:|
| Regions | 4 |
| Holes per region | 2 |
| Vertices per ring, excluding repeated closing point | 16 |
| Tracks | 4 |
| Samples per track | 16 |
| Each signed coordinate | -1000000..1000000 |
| Input bytes | 32768 |
| Output bytes including LF | 16384 |
| Independent track jobs | 4 |

Validation rejects open/short rings, adjacent repeats, zero area, nonadjacent
segment contact, self crossings, adjacent retracing, holes touching or crossing
the outer ring or each other, outside holes and nested holes. Collinear vertices
that continue forward are valid. All validation completes before tasks run.
Coordinate differences are at most 2e6, cross products at most 8e12, and the
16-term doubled area at most 32e12. Every signed multiply/add/subtract is also
checked. The defensive output cap exceeds the largest admitted report; separate
helper controls exercise its exact bound and one-over without replacing main.

Regions and tracks are frozen into immutable shared snapshots. Each scoped child
owns one track result; the parent joins in any completion order, then publishes
in declaration order. Jobs are submitted in batches, bounded by JOBS.
Exit 0 prints `geoaudit: N tracks` after commit. Exit 2 prints `geoaudit: CODE`
to stderr for domain/I/O/cancellation errors. Standard-stream denial exits 3.
Failures before commit preserve the old destination. A later stdout failure can
occur after publication, as with other command-line tools.

Geometry adapts geo 0.33.1 winding, hole precedence and segment tests; see
LICENSES.md for pins, notices and precise deviations. Frozen full-main vectors
and the independent rational-parity oracle are in
`crates/hirsel-cli/tests/support/geofence_audit_34/`. Qualification limits and
recovery obligations are recorded in FRICTION.md. No std/compiler/runtime patch
is part of this application.
