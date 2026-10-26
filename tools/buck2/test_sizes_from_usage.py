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

writes `test-sizes.json` (the evidence); sync.py derives the gitignored
`test_sizes.bzl` (the requests `platforms.bzl` reads) from it. A binary's
shards are one row: they run the same harness.

CPU: one CPU up to 1.6 sustained cores (total CPU / total wall), otherwise
at most two. One CPU may burst to two; short parallel bursts do not establish a
need for a multi-CPU reservation. No full-run exemption: shard the work instead.

Memory: reserve the largest enforced PSS peak times 1.2, rounded to 64 MiB,
with a 128 MiB floor. Separate guard limits still protect virtual-memory allocation;
the pool's kill threshold (twice the reservation) is not a sizing discount.
Unobserved binaries retain their previous evidence rather than being guessed.
"""

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
SHARE_CORES = 1.6
GATE_CPUS = 2
MAX_CPUS = 4
MIN_SAMPLES = 20
MIN_WALL_SECONDS = 1.0
KILL_FACTOR = 2
MEMORY_MARGIN = 1.2
ESCALATION = 1.75
MEMORY_STEP_KB = 65536
MIN_MEMORY_KB = 131072
# Conformance shards are single-threaded and ran past their limit on the
# slower worker class; they keep asking for the fast workers.
CPU_CLASSES = {
    "gate-conformance": "fast",
    # Ran past 600 s on bazelbox-02 and finished on bazelbox-05 (2026-10-20).
    "gate-native-changed": "fast",
    "hirsel-cli-test-cli": "fast",
}
NATIVE_STAGES = {
    "gate-native-full", "gate-native-sanitize", "gate-native-hash-sweep",
    "gate-native-library-c", "gate-native-runtime-c", "gate-native-toolchain-pin",
    "gate-agree-native-full",
    "gate-native-cranelift", "gate-native-cranelift-aarch64", "gate-native-history",
}


def binary(target):
    """A test target's binary: its shards share one row."""
    return re.sub(r"-shard-\d+$", "", target)


def runs(paths, since=0, until=float("inf")):
    """Finished pool test runs by binary, and each run's binary by operation."""
    found = defaultdict(list)
    operations = {}
    seen = set()
    for path in paths:
        with Path(path).open() as source:
            for line in source:
                fields = line.rstrip("\n").split("\t")
                if not fields[0].isdigit():
                    continue
                if not since <= int(fields[0]) < until:
                    continue
                row = dict(field.split("=", 1) for field in fields[1:] if "=" in field)
                test = re.fullmatch(r"run:[^:]*:root//:(.*)", row.get("test", ""))
                if not test or not test.group(1).startswith(("hirsel-", "gate-")):
                    continue
                name = binary(test.group(1))
                operations[row.get("op")] = name
                if row.get("event") == "start" or not row.get("wall_ms", "-").isdigit():
                    continue
                key = (row.get("op"), int(fields[0]), row["wall_ms"])
                if key in seen:
                    continue
                seen.add(key)
                found[name].append(dict(
                    end=int(fields[0]),
                    wall=int(row["wall_ms"]) / 1000,
                    cpu=int(row["cpu_usec"]) / 1e6 if row.get("cpu_usec", "-").isdigit() else None,
                    requested_cpu=int(row["requested_cpu"]),
                    requested_kb=int(row["requested_kb"]),
                ))
    return found, operations


def pss_peaks(paths, operations, cpus=None):
    """Each binary's PSS peaks (KiB) from the scheduler's usage events."""
    peaks = defaultdict(list)
    fallback = defaultdict(list)
    for path in paths:
        with Path(path).open() as source:
            for line in source:
                event = json.loads(line)
                name = operations.get(event.get("operation"))
                if name and event.get("event") == "usage" and event.get("sampled") and event.get("peak_memory_kb"):
                    fallback[name].append(event["peak_memory_kb"])
                    if cpus is None or event.get("effective_cpu", 1) <= cpus[name]:
                        peaks[name].append(event["peak_memory_kb"])
    return defaultdict(list, {name: peaks[name] or values for name, values in fallback.items()})


def kills(path, operations):
    """Each binary's escalated reservations (KiB) after a memory kill."""
    floors = defaultdict(list)
    if path:
        with Path(path).open() as source:
            for line in source:
                line = re.sub(r"\x1b\[[0-9;]*m", "", line)
                kill = re.search(r"Uuid\(([0-9a-f-]+)\), reserved_kb: (\d+)", line)
                name = kill and operations.get(kill.group(1))
                if name:
                    floors[name].append(math.ceil(int(kill.group(2)) * ESCALATION))
    return floors


def guard_stops(path=None):
    """Each binary's memory guard stops: its escalated reservations (KiB) and
    the CPUs a row without runs gets."""
    path = path or HERE / "guard-stops.json"
    if not path.exists():
        return {}
    return {name: ([max(math.ceil(stop["reserved_kb"] * ESCALATION),
                        math.ceil(stop.get("needed_kb", 0) / KILL_FACTOR)) for stop in stops],
                   max(stop.get("cpu_count", 1) for stop in stops))
            for name, stops in json.loads(path.read_text()).items()}


def stopped_row(floors, cpu):
    """A binary the guard stopped before the pool measured it: its floor."""
    return dict(cpu_count=cpu, memory_kb=max([MIN_MEMORY_KB] + [step(f) for f in floors]), cpu_class="",
                runs=0, pss_runs=0, memory_kills=len(floors))


def escalation_floors(paths, operations):
    """Unresolved memory kills retain a floor; completed retries use actual PSS."""
    attempts = defaultdict(list)
    for path in paths:
        with Path(path).open() as source:
            for line in source:
                event = json.loads(line)
                if event.get("event") == "usage" and event.get("operation") in operations:
                    attempts[event["operation"]].append(event)
    floors = defaultdict(list)
    for operation, events in attempts.items():
        # A completed retry establishes its actual peak. Do not turn a
        # geometric retry overshoot into a permanent reservation floor.
        if any(e.get("outcome") == "COMPLETED" for e in events):
            continue
        for event in events:
            if event.get("outcome") == "KILLED_MEMORY":
                floors[operations[operation]].append(math.ceil(
                    event["effective_memory_kb"] * KILL_FACTOR * MEMORY_MARGIN))
    return floors


def native_rows(found, peaks, floors):
    """Size measured native stages and explicitly inherit the largest for missing ones."""
    measured = {name: size(name, samples, peaks[name], floors[name])
                for name, samples in sorted(found.items())
                if name in NATIVE_STAGES and samples}
    if not measured:
        raise ValueError("no measured native stage to use as a fallback")
    largest = max(measured, key=lambda name: (
        measured[name]["memory_kb"], measured[name]["cpu_count"], name))
    for name in sorted(NATIVE_STAGES - measured.keys()):
        measured[name] = {key: measured[largest][key]
                          for key in ("cpu_count", "memory_kb", "cpu_class")}
        measured[name].update(runs=0, pss_runs=0, memory_kills=0, fallback_from=largest,
                              window=measured[largest]["window"])
    return measured


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
    mean = sum(s["cpu"] for s in samples if s["cpu"] is not None) / sum(s["wall"] for s in samples if s["cpu"] is not None) if cores else 0.0
    wanted = 1 if mean <= SHARE_CORES else min(MAX_CPUS, max(GATE_CPUS, math.ceil(p95)))
    cpu = min(wanted, GATE_CPUS)
    p = parallel_fraction(samples)
    slowdown = round(relative_wall(p, GATE_CPUS) / relative_wall(p, wanted), 2) if p is not None and wanted > GATE_CPUS else None
    peak = max(peaks, default=0)
    memory_kb = max([MIN_MEMORY_KB, step(peak * MEMORY_MARGIN)] + [step(f) for f in floors])
    return dict(
        cpu_count=cpu,
        memory_kb=memory_kb,
        cpu_class=CPU_CLASSES.get(name, ""),
        runs=len(samples),
        timed_runs=len(cores),
        p95_cores=round(p95, 2),
        mean_cores=round(mean, 2),
        parallel_fraction=round(p, 2) if p is not None else None,
        two_cpu_slowdown=slowdown,
        pss_runs=len(peaks),
        pss_peak_mib=round(peak / 1024),
        memory_kills=len(floors),
        mean_wall_seconds=round(sum(s["wall"] for s in samples) / len(samples), 1),
        requested=sorted({f"{s['requested_cpu']}/{s['requested_kb']}" for s in samples}),
        window=[min(s["end"] for s in samples), max(s["end"] for s in samples)],
    )


def write_rows(rows):
    """Keep the measured evidence; sync.py derives the requests Buck2 reads from it."""
    rows = dict(sorted(rows.items()))
    (HERE / "test-sizes.json").write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")


def requests_bzl(rows) -> str:
    """The gitignored `test_sizes.bzl` that platforms.bzl reads, from the rows."""
    lines = [
        "# @generated by tools/buck2/sync.py from test-sizes.json; do not edit.",
        "# A pool test binary's request, `[cpu, memory_kb, cpu_class]`; its shards share it.",
        "TEST_REQUESTS = {",
    ]
    lines += ['    "{}": [{}, {}, "{}"],'.format(name, r["cpu_count"], r["memory_kb"], r["cpu_class"])
              for name, r in sorted(rows.items())]
    lines.append("}")
    lines += ["TEST_GUARD_LIMITS = {", *['    "{}": {},'.format(name, r["guard_limit_kb"])
               for name, r in sorted(rows.items()) if r.get("guard_limit_kb")], "}"]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--since", type=float, default=0)
    parser.add_argument("--until", type=float, default=float("inf"))
    parser.add_argument("--usage", nargs="+", required=True, help="copies of the workers' actions.log")
    parser.add_argument("--pss", nargs="+", required=True, help="copies of the scheduler's usage-event trace files")
    parser.add_argument("--native-stages", action="store_true",
                        help="refresh only full native stages, retaining other evidence; missing stages inherit the largest measured request")
    parser.add_argument("--kills", help="the workers' kiln-executor journal lines for memory kills")
    parser.add_argument("--only", nargs="+", help="refresh only these measured binaries, retaining other evidence")
    args = parser.parse_args()
    if args.native_stages and args.only:
        parser.error('--only and --native-stages are mutually exclusive')
    found, operations = runs(args.usage, args.since, args.until)
    cpus = {name: size(name, samples, [], [])["cpu_count"] for name, samples in found.items()}
    peaks = pss_peaks(args.pss, operations, cpus)
    floors = kills(args.kills, operations)
    # Scheduler events also cover kills whose executor journal has rotated out.
    # Prefer their retry evidence, without counting the same kill twice.
    for name, reservations in escalation_floors(args.pss, operations).items():
        floors[name] = reservations + [f for f in floors[name] if f > max(reservations)]
    stops = guard_stops()
    if args.native_stages:
        rows = json.loads((HERE / "test-sizes.json").read_text())
        rows.update(native_rows(found, peaks, floors))
    else:
        if args.only and (missing := set(args.only) - found.keys()):
            parser.error('no measurements for: ' + ', '.join(sorted(missing)))
        rows = json.loads((HERE / "test-sizes.json").read_text())
        rows.update({name: size(name, samples, peaks[name], floors[name])
                     for name, samples in sorted(found.items()) if not args.only or name in args.only})
    if not args.only and any(name in found for name in NATIVE_STAGES):
        fresh_native = native_rows(found, peaks, floors)
        for name, row in list(rows.items()):
            if name not in found and name in fresh_native:
                rows[name] = fresh_native[name]
    for name, (reservations, cpu) in stops.items():
        if name not in rows:
            rows[name] = stopped_row([], 1)
        rows[name]["guard_limit_kb"] = 2 * max(step(f) for f in reservations)
    write_rows(rows)


if __name__ == "__main__":
    main()
