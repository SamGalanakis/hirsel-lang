#!/usr/bin/env python3
"""Size first-party compiles by joining Buck2 events to pool worker usage logs.

Buck2 records
owners/requests/times; the worker records CPU use and anonymous/cgroup memory.
The executor may escalate memory after a kill; the usage request can exceed
Buck2's initial request. Join only a unique record with the same category/CPU, at least the requested memory, finish time (+/-5s)
and duration (+/-0.1s or 2%). Never size from cache hits or ambiguous matches.

  python3 tools/buck2/action_sizes_from_log.py --events events.pb.zst \
    --usage usage-03.log usage-04.log -o tools/buck2/action-sizes.json

Final CPU: 1 while p95 cores is at most 1.6, otherwise clamp(ceil(p95), 2, 4).
Every worker gives a one-CPU action the larger of its request and the slot
share (1.67 cores at the least, by the pool executor's action supervisor), so a second
CPU below that only reserves a core the action leaves idle; such targets keep
two codegen units (defs.bzl). Schedulability on a shared pool beats peak
parallelism: a previous eight-CPU probe must not become a permanent floor.
With <20 samples use the maximum measured core use instead of a percentile.
Memory: anonymous p99 * 1.25 + 512 MiB, rounded to 256 MiB, min 1 GiB;
legacy cgroup peaks * 1.25. With <20 samples use maxima instead of percentiles.
A target's request covers all its compile phases; dev/release are sized apart.

A compile measured under a one-CPU request reads as using one core, so the rule
alone never grows it. The critical-path compiles in CPU_CLASSES are calibrated
instead: build with `-c hirsel.probe_cpus=8` (platforms.bzl), which gives each
of them eight CPUs and eight codegen units for that invocation, and size them
from that evidence with `--merge`, which replaces only the calibrated rows.
CALIBRATED_CPUS keeps the reservation that measured the least wall time,
queue included, on the loaded pool, with that measurement; only such an entry
may exceed four CPUs.
"""

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'


def events(path):
    if path.name.endswith('.pb.zst'):
        command = [str(BUCK2), '--isolation-dir', 'log-reader', 'log', 'show', str(path.resolve())]
        lines = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout.splitlines()
    else:
        lines = path.read_text().splitlines()
    for line in lines:
        if '"ActionExecution"' not in line or '"SpanEnd"' not in line:
            continue
        action = json.loads(line)['Event']['data']['SpanEnd']['data']['ActionExecution']
        owner = action['key']['owner'].get('TargetLabel', {})
        label = owner.get('label', {})
        if label.get('package') != 'root//' or action['name']['category'] != 'rustc':
            continue
        for command in action.get('commands') or []:
            details = command['details']
            remote = details['command_kind']['command'].get('RemoteCommand')
            if not remote or remote['cache_hit'] or action['failed']:
                continue
            props = {p['name']: p['value'] for p in remote['details']['platform']['properties']}
            metadata = details['metadata']
            wall = metadata['execution_time_us'] / 1e6
            seconds, nanos = metadata['start_time']
            yield dict(target=label['name'], profile=('dev' if (':dev#' in owner['configuration']['full_name']
                                                               or owner['configuration']['full_name'].startswith('hirsel-rust-host#'))
                                                    else 'release'),
                       phase=action['name']['identifier'], wall=wall, end=seconds + nanos / 1e9 + wall,
                       cpu=int(props['cpu_count']), memory_kb=int(props['memory_kb']))


def usage(paths):
    records = {}
    for path in paths:
        for line in path.read_text().splitlines():
            fields = line.split('\t')
            if not fields[0].isdigit():
                continue
            row = dict(field.split('=', 1) for field in fields[1:] if '=' in field)
            if row.get('exit') != '0' or row.get('category') != 'rustc':
                continue
            if not all(row.get(k, '-').isdigit() for k in ('wall_ms', 'cpu_usec', 'peak_bytes',
                                                          'requested_cpu', 'requested_kb')):
                continue
            row = row | dict(end=int(fields[0]), worker=path.stem)
            records[row['op']] = row
    return records


def join(actions, records):
    taken = set()
    by_end = defaultdict(list)
    for key, row in records.items():
        by_end[row['end']].append((key, row))
    for action in actions:
        candidates = [(key, row) for second in range(math.floor(action["end"]) - 5, math.ceil(action["end"]) + 6)
                      for key, row in by_end[second]
                      if (row.get('pkg', '-') == '-' or action['target'].startswith(row['pkg']))
                      and abs(row['end'] - action['end']) <= 5
                      and int(row['requested_kb']) >= action['memory_kb']
                      and int(row['requested_cpu']) == action['cpu']
                      and abs(int(row['wall_ms']) / 1000 - action['wall']) <= max(.1, .02 * action['wall'])]
        # Older supervisors cannot see Cargo's identity behind rustc_action.py.
        # Matching is still exact in request and bounded in both measured times;
        # an ambiguous record is deliberately left out, rather than guessed.
        if len(candidates) != 1 or candidates[0][0] in taken:
            yield action, None
            continue
        key, row = candidates[0]
        taken.add(key)
        yield action, row


def percentile(values, p):
    values = sorted(values)
    return values[math.ceil(len(values) * p) - 1]


def peak(row):
    return int(row['anon_peak_bytes'] if row.get('anon_peak_bytes', '-').isdigit() else row['peak_bytes'])


# A one-CPU request already runs unthrottled up to the slot share.
SHARE_CORES = 1.6
MAX_CPUS = 4

# The compiles that ask for the pool's fast workers (`cpu_class=fast`): the
# serial chain of an edit.
# Every other compile, every test run and every gate shard runs on any worker.
# The slowest worker runs the same Rust action about 1.6x slower than the fast
# ones, which matters only for these. A row routes every action of its target.
CPU_CLASSES = {
    "dev": {
        "hirsel-check": "fast",
        "hirsel-check-unittest-harness": "fast",
        "hirsel-driver": "fast",
        "hirsel-driver-test-it-harness": "fast",
        "hirsel-engine": "fast",
        "hirsel-lir": "fast",
        "hirsel-lir-test-it-harness": "fast",
        "hirsel-lir-unittest-harness": "fast",
        "hirsel-modes": "fast",
        "hirsel-modes-unittest-harness": "fast",
        "hirsel-pipeline-tests-test-front-harness": "fast",
        "hirsel-pipeline-tests-test-rc-harness": "fast",
        "hirsel-rc": "fast",
        "hirsel-rc-unittest-harness": "fast",
        "hirsel-types": "fast",
        "hirsel-types-unittest-harness": "fast",
        "hirsel-validate": "fast",
        "hirsel-validate-test-it-harness": "fast",
        "hirsel-validate-unittest-harness": "fast",
    },
    "release": {
        "hirsel-check": "fast",
        "hirsel-driver": "fast",
        "hirsel-engine": "fast",
        "hirsel-modes": "fast",
        "hirsel-rc": "fast",
    },
}

# Critical-path reservations calibrated with `-c hirsel.probe_cpus=N` (N = 2,
# 4, 8) beside the measured requests: the CPU count whose codegen (rlib) or
# harness link action measured the least mean wall time including queue, over
# builds with types and driver edits (2026-10-16). Rows for crates the driver
# split moved were measured after it (split-*). The dev rows of the crates at
# dev opt-level 1 (hirsel-types, validate, lir and rc, with their unit-test and
# `it` harnesses) were measured at that level with N = 1, 2 and 4 over span,
# types, lir and validate edits;
# a tie takes the smaller count. On the loaded pool an eight-CPU slot waited
# up to 321 s while four-CPU requests waited at most 17 s, so a row takes its
# fastest count within MAX_CPUS unless eight CPUs win by more than such a wait
# risks. They replace the p95 rule for their rows and alone may exceed MAX_CPUS.
CALIBRATED_CPUS = {
    "dev": {
        "hirsel-driver": {"cpu_count": 2, "wall_with_queue_seconds": {2: 2.3, 4: 10.5, 8: 10.2}},
        "hirsel-driver-test-it-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 16.6, 2: 6.2, 4: 7.1, 8: 12.2}},
        "hirsel-engine": {"cpu_count": 4, "wall_with_queue_seconds": {1: 9.9, 2: 7.5, 4: 5.3, 8: 3.2}},
        "hirsel-lir": {"cpu_count": 4, "wall_with_queue_seconds": {1: 88.2, 2: 51.0, 4: 47.3}},
        "hirsel-lir-test-it-harness": {"cpu_count": 4, "wall_with_queue_seconds": {1: 36.3, 2: 24.6, 4: 21.9}},
        "hirsel-lir-unittest-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 19.9, 2: 13.7, 4: 27.8}},
        "hirsel-pipeline-tests-test-front-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 8.4, 2: 4.3, 4: 4.6, 8: 7.5}},
        "hirsel-pipeline-tests-test-rc-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 11.0, 2: 5.4, 4: 6.3, 8: 8.7}},
        "hirsel-rc": {"cpu_count": 4, "wall_with_queue_seconds": {1: 73.5, 2: 56.2, 4: 42.7}},
        "hirsel-rc-unittest-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 15.0, 2: 10.3, 4: 14.5}},
        "hirsel-types": {"cpu_count": 2, "wall_with_queue_seconds": {1: 24.8, 2: 14.7, 4: 23.6}},
        "hirsel-types-unittest-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 49.7, 2: 34.8, 4: 35.6}},
        "hirsel-validate": {"cpu_count": 2, "wall_with_queue_seconds": {1: 70.8, 2: 44.8, 4: 53.3}},
        "hirsel-validate-test-it-harness": {"cpu_count": 2, "wall_with_queue_seconds": {1: 30.8, 2: 21.6, 4: 21.6}},
        "hirsel-validate-unittest-harness": {"cpu_count": 4, "wall_with_queue_seconds": {1: 81.6, 2: 58.0, 4: 53.8}},
    },
    "release": {
        "hirsel-driver": {"cpu_count": 2, "wall_with_queue_seconds": {2: 11.5, 4: 11.7, 8: 14.6}},
        "hirsel-engine": {"cpu_count": 4, "wall_with_queue_seconds": {1: 68.8, 2: 24.2, 4: 13.3, 8: 13.6}},
        "hirsel-rc": {"cpu_count": 4, "wall_with_queue_seconds": {1: 80.3, 2: 55.7, 4: 40.6, 8: 64.8}},
    },
}


def check_calibrated(calibrated):
    """Each calibrated request is its fastest measured count, or its fastest
    count within MAX_CPUS (a larger count's slot wait is the risk)."""
    for profile, rows in calibrated.items():
        for target, row in rows.items():
            measured = row["wall_with_queue_seconds"]
            within = {c: w for c, w in measured.items() if c <= MAX_CPUS}
            if row["cpu_count"] not in (min(measured, key=measured.get), min(within, key=within.get, default=None)):
                raise ValueError(f"{profile}/{target}: a calibrated request must be its fastest measured count")


check_calibrated(CALIBRATED_CPUS)


def cpu_request(p95_cores):
    if p95_cores <= SHARE_CORES:
        return 1
    return max(2, min(MAX_CPUS, math.ceil(p95_cores)))


def compile_cpus(profile, target, p95_cores):
    calibrated = CALIBRATED_CPUS[profile].get(target)
    return calibrated["cpu_count"] if calibrated else cpu_request(p95_cores)


def sizes(matched):
    grouped = defaultdict(list)
    unmatched = []
    for action, row in matched:
        if row is None:
            unmatched.append(action)
            continue
        grouped[(action['profile'], action['target'])].append((action, row))
    result = dict(schema=1, profiles={'dev': {}, 'release': {}}, unmatched=unmatched)
    for (profile, target), samples in sorted(grouped.items()):
        # Very short actions do not establish CPU parallelism.
        cores = [int(row['cpu_usec']) / (int(row['wall_ms']) * 1000)
                 for action, row in samples if int(row['wall_ms']) >= 1000
                 or (action['phase'].split()[0] in ('rlib', 'link') and int(row['cpu_usec']) >= 1_000_000)] or [1.0]
        peaks = [peak(row) for _, row in samples]
        p95 = percentile(cores, .95) if len(cores) >= 20 else max(cores)
        p99 = percentile(peaks, .99) if len(peaks) >= 20 else max(peaks)
        cpus = compile_cpus(profile, target, p95)
        need = max(peak(row) * 1.25
                   + (512 * 1024**2 if row.get('anon_peak_bytes', '-').isdigit() else 0)
                   for _, row in samples)
        memory_kb = max(1048576, math.ceil(need / (256 * 1024**2)) * 256 * 1024)
        if memory_kb > 8 * 1024**2:
            raise ValueError(f'{profile}/{target} exceeds the 8 GiB measurement cap')
        result['profiles'][profile][target] = dict(cpu_count=cpus, memory_kb=memory_kb,
            samples=len(samples), p95_cores=p95, p99_peak_bytes=p99,
            measurements=[dict(phase=a['phase'], wall_seconds=round(a['wall'], 3),
                               cpu_seconds=round(int(r['cpu_usec']) / 1e6, 3),
                               anonymous_peak_bytes=peak(r),
                               requested_cpu=a['cpu'], requested_memory_kb=int(r['requested_kb']), worker=r['worker'],
                               usage_operation=r.get('op', '-'))
                          for a, r in samples])
    return result


def merge(existing, measured):
    """`existing` with its CALIBRATED_CPUS rows replaced by `measured`, and both
    sets of logs: probe evidence must not resize the rows the p95 rule sizes."""
    for profile, rows in measured['profiles'].items():
        existing['profiles'][profile].update({t: row for t, row in rows.items() if t in CALIBRATED_CPUS[profile]})
    existing['event_logs'] = existing.get('event_logs', []) + measured['event_logs']
    existing['unmatched'] = existing.get('unmatched', []) + [
        a for a in measured['unmatched'] if a['target'] in CALIBRATED_CPUS[a['profile']]]
    return existing


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--events', nargs='+', type=Path, required=True)
    parser.add_argument('--usage', nargs='+', type=Path, required=True)
    parser.add_argument('-o', '--output', type=Path, required=True)
    parser.add_argument('--merge', action='store_true',
                        help='replace only the CALIBRATED_CPUS rows in the existing output')
    args = parser.parse_args()
    actions = [a for path in args.events for a in events(path)]
    result = sizes(join(actions, usage(args.usage)))
    result['event_logs'] = [p.name for p in args.events]
    if args.merge:
        result = merge(json.loads(args.output.read_text()), result)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    print(f"sized {sum(map(len, result['profiles'].values()))} targets; {len(result['unmatched'])} unmatched actions")


if __name__ == '__main__':
    main()
