# Managed core resolution and metrics

`hirsel_run_config::managed_cores` implements the host-side resolution and metrics
parts of multicore K1. These are placement diagnostics, outside canonical run
bytes and identity. Replay does not construct the host reader or a metrics writer.

The live launcher supplies `HostGrant { max_cores, cpus }` independently of the
run record. Capture `LinuxLimits` on the admission thread before pinning, then
read it once per second through `CoreResolver::refresh`. The refresh callback
is invoked only when due and reads the current grant and topology. It discovers
cgroup-v2 membership again on each read (including after cgroup migration), walks
every visible ancestor's `cpu.max`, and retains the original admission affinity.
Managers fold quotas hidden above the namespace root into their grant.

`resolve` intersects the placement cpulist, admission affinity, effective cpuset
and host grant. Fixed widths bypass quota; auto uses the ceiling of the minimum
quota ratio; ranges clamp auto, then obey the hard CPU/grant ceiling. Admission
and whole control replacements refuse empty intersections. Only external
contraction may override a conflicting placement cpulist, with `cpus_conflict`.
An unreadable/malformed host limit refuses resolution rather than widening the
run. Missing controller files mean no limit from that controller; cgroup v1
is explicitly unsupported. CPU sets use merged intervals, so large input ranges
do not allocate one element per CPU. Quota comparisons use exact u128 products.

The caller owns actual worker resizing and publishes actual effective/pending
widths, not the resolver's target as if it had already taken effect. The caller
also owns destination pinning and exclusion against mounts and sinks, control
socket admission, compute permits and lost-permit accounting. Width remains one
until the executor's differential gate passes; resolving a target never starts
workers. Shared process permits, nested runners, divergent stopped workers and
heap migration are integration responsibilities of the live-core lane.

`MetricsSnapshot` contains every signal from the design's section 7. Executor
measurements that are not supplied are JSON null. `MetricsWriter` receives an
already admitted `Write`, emits/flushes one JSON line per second and computes
`throttled_usec` deltas. A cgroup migration/reset cannot underflow that delta.
A write failure is returned and disables further writes to avoid appending after
a partial line. Control may serialize the same snapshot without advancing the
periodic stream's delta. Queue and scheduler counters, delay percentiles, rates
and stall sites are supplied by their owning executor/allocator; this module
neither samples tasks nor fabricates measurements.

The cgroup reader starts from Go runtime's cgroup-v2 reader at commit
`ebe2feefc400f813aefdc8a036e66d2619da6649`:
`src/internal/runtime/cgroup/cgroup_linux.go:116-140,201-237`,
`cgroup.go:72-107`, and `cgroup_test.go:74-155`. Its quota test vectors are ported.
Changes required here: walk all visible ancestors (Go reads only the leaf), use
exact rational comparison and integer ceiling, reject zero/negative/malformed
periods including for `max`, retain admission affinity across pinning, and
rediscover membership at each refresh. Mountinfo's escaped paths are decoded;
a visible namespace-root mount is preferred over a delegated bind mount to
avoid hiding observable ancestor limits. File reads replace Go's cached FDs so
cgroup migration cannot leave a stale limit in use.

Run `//:hirsel-run-config-test-it` for resolution, identity and metrics tests.
For an actual capped-host probe, use the built test harness:

```sh
HIRSEL_JOBS=2 tools/capped <harness> managed_cores::real_host_limit_probe --exact --nocapture
HIRSEL_JOBS=8 tools/capped <harness> managed_cores::real_host_limit_probe --exact --nocapture
```

The probe reports target, hard limit and quota. An enclosing ancestor quota or
host affinity may legitimately reduce these below the requested cap. Set
`HIRSEL_CORES_EXPECTED` only where the complete host grant/ancestor limits are
known. Deterministic fixtures check exact widths 2 and 8 and parent-only quotas
independently of the shared host's grant.

## Live admission and control

The native live launcher captures the admission view, resolves placement and
rechecks every second, including fixed integer requests. It pins both sidecars
against **all** acquired mounts and file-backed sinks before opening either.
Replay skips this service and opens neither path. A control socket is bound
relative to its pinned parent, chmodded to 0600 before listening, and checks
Linux `SO_PEERCRED` against the launcher uid on every connection. Existing leaves
are refused; shutdown removes only the socket inode that admission created.

`hirsel ctl <socket> cores 6` sends one bounded JSON-line transaction. The response
contains requested, effective and pending width. `hirsel ctl <socket> metrics`
returns the same diagnostic snapshot. A request can patch `placement.cores` and `placement.cpus`; unknown
members, invalid values, empty CPU intersections and destination changes refuse
the whole request. Destinations require a fresh admission.

`hirsel_rt::managed_cores::ManagedCores` exposes the same transaction and injected
clock seam to embedding hosts. Its worker enable flag is an executor gate;
production keeps it false until the ordered/free differential gates land. A
resolved target above one remains pending, with actual width reported as one.
Explicit worker test knobs enable live resizing. Workers above a contracted
width finish their current segment and hand off at a scheduling boundary. Their
threads exit, heaps become orphaned, and subsequent growth re-adopts those heaps.
No pure segment or foreign interval is preempted. Retired destination objects
stay alive until run reclamation so suspended handoff frames remain valid.

The existing process permit pool covers width-one roots, compute workers,
callbacks, scheduler code and local runners. Waiting callers release their
permit before entering a callee. Stopped computing workers retain theirs;
`pool::lost_compute_permits()` includes such workers even after their run handles
are dropped. A manager can raise `set_compute_grant`, but no launch manufactures
replacement permits. Effective diagnostic width is capped by the process grant
minus lost permits. `permit_wait_ns` is the mean acquisition time in B1's
`bench/multicore/telemetry.json`; clocks and counters run only when worker metrics
are enabled.

Prior art: the existing Rust std condition-variable predicate and Tokio permit
return-before-wake protocol cited in `docs/notes/multicore-workers.md` are retained.
Socket creation follows Rust std `ea137335b788`,
`library/std/src/os/unix/net/listener.rs:73-109`; credentials follow
`library/std/src/os/unix/net/ucred.rs:49-78`. The deviations are a pinned parent,
exclusive new leaf, chmod before listen, bounded newline transactions and a
shutdown descriptor, required for capability exclusion and prompt host cleanup.

Integration tests add repeated 1/2/4/8 growth/shrink, whole-request refusal,
external contraction, production gating, mount/sink exclusion and socket mode.
The live-width probe can run under `tools/capped` with `HIRSEL_JOBS=2` or `8`
and `HIRSEL_CORES_EXPECTED` on hosts whose ancestor grants allow that width.
Production concurrent execution and its width-1/8 tape differential remain owned
by E1/F1; this service does not declare those gates passed.

## Host core broker

`tools/core-broker /run/hirsel/broker.json` manages guaranteed and burst grants
across runs through the admitted control sockets. It works with either task
provider. `--once` performs one rebalance and exits, returning nonzero on failure.
The daemon emits a JSON diagnostic line per iteration; its default interval is
one second, configurable from 100 to 1000 milliseconds. None of this becomes a
run input or tape event.

```json
{
  "cores": 8,
  "interval_ms": 1000,
  "delay_ns": 1000000,
  "cgroup_root": "/sys/fs/cgroup",
  "runs": [
    {
      "name": "interactive",
      "control": "/run/hirsel/interactive/ctl",
      "cgroup": "/sys/fs/cgroup/hirsel/interactive",
      "guaranteed": 2,
      "burst": 6
    },
    {
      "name": "batch",
      "control": "/run/hirsel/batch/ctl",
      "cgroup": "/sys/fs/cgroup/hirsel/batch",
      "guaranteed": 2,
      "burst": 6
    }
  ]
}
```

`burst` is the total maximum width, including the guarantee. Admission refuses
overcommitted guarantees and unknown config members. The host manager supplies
the pool size and grants within its available CPUs and any limits invisible
above `cgroup_root`. It keeps each run's configured cgroup path current (restart
the broker after moving a run), admits its control socket outside program
grants, and runs the broker with the launcher uid. This daemon controls widths;
it does not alter cgroup quotas. Run one broker for the pool and give it exclusive
ownership of these runs' width controls. An exclusive lock on the config file
prevents two brokers using that same file; separate configs must not overlap.

Each iteration reads all snapshots and all visible ancestor `cpu.max` files
before issuing changes. Every integer target is at most the ceiling of the
smallest quota ratio and the run's burst grant. Quota contraction can reduce a
guarantee; the report marks `guarantee_limited`. K1 additionally enforces the
admitted CPU intersection and process permit ceiling. Failed reads or malformed
metrics freeze the iteration and report an error. Successfully applied shrinks
remain in effect if a later request fails; the next iteration reconciles actual
state from new snapshots.

When p99 segment-ready delay rises above `delay_ns` and `role_busy < 0.8`, a run
can gain one core per iteration. Idle runs return burst capacity to their
guarantee. Burst allocation rotates across equally congested runs after reserving
guarantees. A saturated scheduler role is flagged for `tasks.free@1` or runner
sharding. Unknown/null queue or role measurements never trigger growth; supplying
those counters belongs to the executor. Pending width increases are reserved,
shrinks must release actual workers before capacity is lent, and lost permits
or workers pending reclamation reduce available capacity. The broker never
manufactures replacement capacity or restarts a stopped run. If stopped workers
consume guaranteed capacity, growth waits for reclamation or an explicit host
grant increase.

The policy ports Caladan's guarantee/max admission and queue-delay congestion
structure from `shenango/caladan` at
`bdb4ddebc2198cd95991079ed20e44a5f210d6d3`:
`iokernel/simple.c:16-40,90-112,226-259` and `iokernel/sched.c:890-908`.
Its guarantee-before-burst and delay-threshold vectors are exercised in
`tools/tests/core_broker.py`. Deviations: K1 JSON-line transactions replace
kernel-bypass core assignment at the specified host-manager cadence; rising
delay and the scheduler-role saturation filter implement Hirsel's section 7;
cooperative shrink acknowledgments and stopped-worker reservations replace
immediate preemption; ancestor quota ceilings prevent integer controls from
bypassing quotas. Socket framing and peer credentials reuse the K1 protocol.

Run `.buck2/bin/buck2 test //:gate-core-broker` (also collected by `tools/main-runs`
as `python3 tools/tests/core_broker.py`).
The toy-broker test launches the actual broker against two Unix control peers,
rebalances widths from 3/1 to 1/3, and asserts both change. Other tests cover
pending resize, guarantees, burst limits, saturation, missing signals, quota
contraction and exact ancestor ratios, lost capacity and invalid requests.
