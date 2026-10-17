#!/usr/bin/env python3
"""Size each pool test binary's reservation from what the pool measured.

Three read-only sources, copied to local files:

- each worker's `actions.log` (and `actions.log.1`): the action supervisor's
  row per finished action, with its CPU and wall time, tagging a test run with
  its `TEST_TARGET` (`test=run:<shard index>:root//:<target>`) and operation;
- the scheduler's per-execution usage events (kiln-resource-hints `trace/*.jsonl`
  on the cache host): each operation's `peak_memory_kb`, the peak of the PSS
  sampler that enforces memory;
- optionally, the workers' `kiln-executor` journal lines for kills
  ("exceeded its memory reservation ... Uuid(<operation>) ... reserved_kb").

  python3 tools/buck2/test_sizes_from_usage.py --usage /tmp/usage-*.log \
    --pss /tmp/trace-*.jsonl --kills /tmp/kills.txt

writes `test-sizes.json` (the evidence) and `test_sizes.bzl` (the requests
`platforms.bzl` reads). A binary's shards are one row: they run the same harness.

CPU: 1 while p95 cores (CPU seconds per wall second, runs of at least 1 s) is
at most 1.6, otherwise clamp(ceil(p95), 2, 4). A one-CPU action gets the
larger of its request and the worker's slot share (1.25-1.67 cores), so a
second CPU below that reserves a core the run leaves idle. The launcher runs
as many test threads as the reservation's whole CPUs, so a multi-threaded
binary keeps the count it measured using. With fewer than 20 runs the maximum
replaces p95. The pool holds its one reservation for a blocked multi-CPU
action, so a three- or four-CPU run can wait minutes where a two-CPU one starts
in seconds: a binary that rule gives more than two CPUs takes two when its
measured parallelism predicts at most 30% more wall time there. That fraction
is Amdahl's, from each run's cores used over the test threads it ran (its
request, at most 4), the median over runs. The agreement shards keep their
count for their timeouts.

Memory: the worker samples an action's process-group PSS (shared libraries and
mapped binaries included) every 250 ms and kills it past twice its memory_kb on
two samples, then the scheduler requeues it with more. A binary reserves its
largest PSS peak / 2 * 1.2, rounded up to 256 MiB, at least 512 MiB, and at
least the escalated reservation (1.75x) of any run of it killed for memory.
"""

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
SHARE_CORES = 1.6
MAX_CPUS = 4
MIN_SAMPLES = 20
MIN_WALL_SECONDS = 1.0
KILL_FACTOR = 2
MEMORY_MARGIN = 1.2
ESCALATION = 1.75
MEMORY_STEP_KB = 262144
MIN_MEMORY_KB = 524288
TWO_CPU_SLOWDOWN = 1.3
KEEP_CPUS = {"gate-agree", "gate-agree-full"}
# Conformance shards are single-threaded and ran past their limit on the
# slower worker class; they keep asking for the fast workers.
CPU_CLASSES = {"gate-conformance": "fast"}


def binary(target):
    """A test target's binary: its shards share one row."""
    return re.sub(r"-shard-\d+$", "", target)


def runs(paths):
    """Finished pool test runs by binary, and each run's binary by operation."""
    found = defaultdict(list)
    operations = {}
    for path in paths:
        for line in Path(path).read_text().splitlines():
            fields = line.split("\t")
            if not fields[0].isdigit():
                continue
            row = dict(field.split("=", 1) for field in fields[1:] if "=" in field)
            test = re.fullmatch(r"run:[^:]*:root//:(.*)", row.get("test", ""))
            if not test or not test.group(1).startswith(("hirsel-", "gate-")):
                continue
            name = binary(test.group(1))
            operations[row.get("op")] = name
            if row.get("event") == "start" or not row.get("wall_ms", "-").isdigit():
                continue
            found[name].append(dict(
                end=int(fields[0]),
                wall=int(row["wall_ms"]) / 1000,
                cpu=int(row["cpu_usec"]) / 1e6 if row.get("cpu_usec", "-").isdigit() else None,
                requested_cpu=int(row["requested_cpu"]),
                requested_kb=int(row["requested_kb"]),
            ))
    return found, operations


def pss_peaks(paths, operations):
    """Each binary's PSS peaks (KiB) from the scheduler's usage events."""
    peaks = defaultdict(list)
    for path in paths:
        for line in Path(path).read_text().splitlines():
            event = json.loads(line)
            name = operations.get(event.get("operation"))
            if name and event.get("event") == "usage" and event.get("sampled") and event.get("peak_memory_kb"):
                peaks[name].append(event["peak_memory_kb"])
    return peaks


def kills(path, operations):
    """Each binary's escalated reservations (KiB) after a memory kill."""
    floors = defaultdict(list)
    if path:
        for line in Path(path).read_text().splitlines():
            kill = re.search(r"Uuid\(([0-9a-f-]+)\), reserved_kb: (\d+)", line)
            name = kill and operations.get(kill.group(1))
            if name:
                floors[name].append(math.ceil(int(kill.group(2)) * ESCALATION))
    return floors


def percentile(values, p):
    values = sorted(values)
    return values[math.ceil(len(values) * p) - 1]


def high(values, p):
    return percentile(values, p) if len(values) >= MIN_SAMPLES else max(values)


def step(kb):
    return math.ceil(kb / MEMORY_STEP_KB) * MEMORY_STEP_KB


def parallel_fraction(samples):
    """Amdahl's parallel fraction, the median over timed runs of more than one thread."""
    fractions = []
    for s in samples:
        threads = min(s["requested_cpu"], MAX_CPUS)
        if s["cpu"] is None or s["wall"] < MIN_WALL_SECONDS or threads < 2:
            continue
        speedup = min(max(s["cpu"] / s["wall"], 1.0), threads)
        fractions.append((1 - 1 / speedup) / (1 - 1 / threads))
    return percentile(fractions, .5) if fractions else None


def relative_wall(p, cpus):
    return (1 - p) + p / cpus


def size(name, samples, peaks, floors):
    cores = [s["cpu"] / s["wall"] for s in samples if s["cpu"] is not None and s["wall"] >= MIN_WALL_SECONDS]
    p95 = high(cores, .95) if cores else 0.0
    cpu = 1 if p95 <= SHARE_CORES else min(MAX_CPUS, max(2, math.ceil(p95)))
    p = parallel_fraction(samples)
    slowdown = round(relative_wall(p, 2) / relative_wall(p, cpu), 2) if p is not None and cpu > 2 else None
    if slowdown is not None and slowdown <= TWO_CPU_SLOWDOWN and name not in KEEP_CPUS:
        cpu = 2
    peak = max(peaks, default=0)
    memory_kb = max([MIN_MEMORY_KB, step(peak / KILL_FACTOR * MEMORY_MARGIN)] + [step(f) for f in floors])
    return dict(
        cpu_count=cpu,
        memory_kb=memory_kb,
        cpu_class=CPU_CLASSES.get(name, ""),
        runs=len(samples),
        timed_runs=len(cores),
        p95_cores=round(p95, 2),
        parallel_fraction=round(p, 2) if p is not None else None,
        two_cpu_slowdown=slowdown,
        pss_runs=len(peaks),
        pss_peak_mib=round(peak / 1024),
        memory_kills=len(floors),
        mean_wall_seconds=round(sum(s["wall"] for s in samples) / len(samples), 1),
        requested=sorted({f"{s['requested_cpu']}/{s['requested_kb']}" for s in samples}),
        window=[min(s["end"] for s in samples), max(s["end"] for s in samples)],
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--usage", nargs="+", required=True, help="copies of the workers' actions.log")
    parser.add_argument("--pss", nargs="+", required=True, help="copies of the scheduler's usage-event trace files")
    parser.add_argument("--kills", help="the workers' kiln-executor journal lines for memory kills")
    args = parser.parse_args()
    found, operations = runs(args.usage)
    peaks = pss_peaks(args.pss, operations)
    floors = kills(args.kills, operations)
    rows = {name: size(name, samples, peaks[name], floors[name]) for name, samples in sorted(found.items())}
    (HERE / "test-sizes.json").write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")
    lines = [
        "# @generated by tools/buck2/test_sizes_from_usage.py from test-sizes.json; do not edit.",
        "# A pool test binary's request, `[cpu, memory_kb, cpu_class]`; its shards share it.",
        "TEST_REQUESTS = {",
    ]
    lines += ['    "{}": [{}, {}, "{}"],'.format(name, r["cpu_count"], r["memory_kb"], r["cpu_class"]) for name, r in rows.items()]
    lines.append("}")
    (HERE / "test_sizes.bzl").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
