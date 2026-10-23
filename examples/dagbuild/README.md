# dagbuild

A declared local task DAG. This app does not build Hirsel or use compiler caches.

`dagbuild plan manifest.json` explains clean/dirty tasks without spawning or
writing state. `dagbuild build manifest.json` runs ready commands with bounded
concurrency and publishes successful cache records after every child is joined.
See grants.json for explicit mounts. Only one invocation may write a state grant
at a time. Commands are literal argv, with no implicit shell, cwd or environment.

Schema: `{tasks:[{name,argv,dependencies,inputs,outputs}],jobs:2,fail_fast:false}`.
All five task fields are required. Artifact paths are canonical relative paths in
project; commands receive their own explicit absolute host paths. Dependencies
must include every producer whose artifacts a task consumes. Child side effects
are real: failed commands do not roll back artifacts. State is isolated in its
own mount. Before spawning, build atomically clears prior state, so interrupted
runs conservatively lose hits. Final publication includes only successful tasks.

Limits: 32 nodes, 128 raw edges (duplicates count), 64 total input/output path
declarations, 64 KiB manifest, 4096 argv bytes including NUL terminators per
command, 64 argv entries, jobs 1..4, 1 MiB cumulative artifact bytes checked per
workflow. Each artifact size is checked before a bounded read; growth can refuse.
The admitted command workflow has a 30-second scope deadline. Output is drained
and discarded. Input bytes, argv, dependency results and output digests determine
rebuilds; timestamps never establish cleanliness. State format v1 is local,
unauthenticated cache data. Corruption refuses rather than silently trusting it.

Exit 0: success/plan/help; 1: task failed, blocked or canceled; 2: invalid DAG or
state; 3: file, resource or capability refusal. Host tooling and language failures
retain their launcher classifications. Reports are declaration-order JSON lines.

Verification and remaining engine/replay dispositions are in FRICTION.md.
## Concrete smoke invocation

From the checkout root, with a matching built CLI as `$HIRSEL`:

```sh
mkdir -p /tmp/dagbuild-demo/project /tmp/dagbuild-demo/state
cp examples/dagbuild/fixtures/manifest.json /tmp/dagbuild-demo/project/manifest.json
python3 - <<'PY'
import json
from pathlib import Path
x=json.loads(Path('examples/dagbuild/grants.json').read_text())
x['io']['dirs'][0]['host']='/tmp/dagbuild-demo/project'
x['io']['dirs'][1]['host']='/tmp/dagbuild-demo/state'
Path('/tmp/dagbuild-demo/grants.json').write_text(json.dumps(x))
PY
"$HIRSEL" build examples/dagbuild/hirsel.json --backend llvm -o /tmp/dagbuild-demo/dagbuild
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 \
  /tmp/dagbuild-demo/dagbuild --hirsel-record /tmp/dagbuild-demo/build.tape \
  --hirsel-grants /tmp/dagbuild-demo/grants.json build manifest.json
/tmp/dagbuild-demo/dagbuild --hirsel-grants /tmp/dagbuild-demo/grants.json plan manifest.json
/tmp/dagbuild-demo/dagbuild --hirsel-replay /tmp/dagbuild-demo/build.tape
```

The shipped manifest uses `/usr/bin/true` for two dependent validation tasks.
Replace argv with exact granted programs and explicit arguments for real work.
Put tapes outside both mutable mounts. Change placement.cores to 1,2,4 to request
Ordered widths; observe a control snapshot before claiming effective placement.
Observer/record flags precede the grants operand; application arguments follow it.

Harness partitions: `DAGBUILD_29_CASE` selects exact fixture IDs;
`DAGBUILD_29_CAPS=1` selects unchanged boundary vectors; `DAGBUILD_29_BACKEND`
selects one native backend; `DAGBUILD_29_REAL_ONLY=1` selects live handshakes.
No public checked-H host-tape replay adapter exists: that cell is unsupported.
Required failures/unrun cells remain INCOMPLETE even when harness disposition
assertions pass. Per-cell external receipts are the engine verdict.

`grants-plan.json` is the read-only template and grants no process authority.
Both modes need Tasks and the monotonic clock for their whole-run deadline.
For the focused harness, set `HIRSEL_MEMORY_LIMIT=2G`; run the named native
backend test separately. The two ignored tests audit external immutable tapes
and refusal controls; they require the documented DAGBUILD_29_* tape paths.
Every incomplete cell and its owner is retained in FRICTION.md.
