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
