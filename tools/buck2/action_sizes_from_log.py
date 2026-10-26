#!/usr/bin/env python3
"""Size pool compiles from the last day of execution evidence.

Join exported Buck2 commands to scheduler dispatches by action digest and time,
then to worker CPU usage and enforced PSS peaks by operation. Cache hits, failed
attempts, ambiguous joins and evidence outside --since/--until are excluded.
Both first-party and third-party compiles are sized, dev/release separately.

CPU uses total CPU seconds / total wall seconds across the run, rather than a
short phase's burst or an old calibration. One CPU can burst to two on the pool;
reserve two only above 1.6 sustained cores. Four CPUs need a fresh comparative
critical-path calibration; none is currently established. Memory reserves the
largest measured PSS peak times 1.2, rounded to 64 MiB, with a 128 MiB floor.
The 20% margin covers sampler jitter and code changes without a fixed 512 MiB
surcharge on small compiles. Keep unmatched actions visible in the evidence.

  action_sizes_from_log.py --commands commands.jsonl --pss trace.jsonl \
    --usage usage-*.log --since EPOCH --until EPOCH -o action-sizes.json
"""

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path

def usage(paths):
    records = {}
    for path in paths:
        with path.open() as source:
            for line in source:
                fields = line.rstrip('\n').split('\t')
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


# A one-CPU request may burst to two CPUs.
SHARE_CORES = 1.6
MAX_CPUS = 2

# The compiles that ask for the pool's fast workers (`cpu_class=fast`): the
# serial chain of an edit.
# Every other compile, every test run and every gate shard runs on any worker.
# The slowest worker runs the same Rust action about 1.6x slower than the fast
# ones, which matters only for these. A row routes every action of its target.
CPU_CLASSES = {
    "dev": {
        "hirsel-authority": "fast",
        "hirsel-authority-unittest-harness": "fast",
        "hirsel-check": "fast",
        "hirsel-check-unittest-harness": "fast",
        "hirsel-driver": "fast",
        "hirsel-driver-test-it-harness": "fast",
        "hirsel-engine": "fast",
        "hirsel-lir": "fast",
        "hirsel-lir-test-it-harness": "fast",
        "hirsel-lir-unittest-harness": "fast",
        "hirsel-ownership": "fast",
        "hirsel-ownership-unittest-harness": "fast",
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
        "hirsel-authority": "fast",
        "hirsel-check": "fast",
        "hirsel-driver": "fast",
        "hirsel-engine": "fast",
        "hirsel-ownership": "fast",
        "hirsel-rc": "fast",
    },
}

# Historical 2026-10-16 calibrations have expired. A future four-CPU request
# must carry fresh comparative evidence; old probes are not permanent floors.
CALIBRATED_CPUS = {"dev": {}, "release": {}}


def cpu_request(mean_cores):
    if mean_cores <= SHARE_CORES:
        return 1
    return 2


def compile_cpus(profile, target, mean_cores):
    calibrated = CALIBRATED_CPUS[profile].get(target)
    return calibrated["cpu_count"] if calibrated else cpu_request(mean_cores)


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
        wall = sum(int(r['wall_ms']) / 1000 for _, r in samples)
        mean = sum(int(r['cpu_usec']) / 1e6 for _, r in samples) / wall if wall >= 1 else 1.0
        cpus = compile_cpus(profile, target, mean)
        need = max(int(row.get('pss_peak_bytes', peak(row))) for _, row in samples) * 1.2
        memory_kb = max(131072, math.ceil(need / (64 * 1024**2)) * 64 * 1024)
        if memory_kb > 8 * 1024**2:
            raise ValueError(f'{profile}/{target} exceeds the 8 GiB measurement cap')
        result['profiles'][profile][target] = dict(cpu_count=cpus, memory_kb=memory_kb,
            samples=len(samples), mean_cores=mean, p95_cores=p95, p99_peak_bytes=p99,
            pss_peak_bytes=max(int(r.get("pss_peak_bytes", peak(r))) for _, r in samples),
            window=[min(r.get("end", 0) for _, r in samples), max(r.get("end", 0) for _, r in samples)],
            phases={phase: dict(
                samples=len(group),
                wall_seconds=round(sum(a['wall'] for a, _ in group), 3),
                cpu_seconds=round(sum(int(r['cpu_usec']) / 1e6 for _, r in group), 3),
                max_wall_seconds=round(max(a['wall'] for a, _ in group), 3),
                requested_cpus=sorted({a['cpu'] for a, _ in group}))
                for phase in sorted({a['phase'] for a, _ in samples})
                for group in [[(a, r) for a, r in samples if a['phase'] == phase]]})
    return result


def measured_commands(commands, traces, records, since, until):
    """Digest joins cannot confuse concurrent equal-sized compiles."""
    by_digest = defaultdict(list)
    peaks = {}
    for path in traces:
        with path.open() as source:
            for line in source:
                e = json.loads(line)
                if e.get('event') == 'dispatch':
                    by_digest[e['action_digest'].replace(':', '-')].append(e)
                elif e.get('event') == 'usage' and e.get('sampled') and e.get('outcome') in ('COMPLETED', 'KILLED_MEMORY'):
                    peaks[e['operation']] = max(peaks.get(e['operation'], 0), e.get('peak_memory_kb', 0) * 1024)
    seen = set()
    for path in commands:
        with path.open() as source:
            for line in source:
                a = json.loads(line)
                if a['category'] != 'rustc' or a['cached'] or a['failed'] or not since <= a['end'] < until:
                    continue
                a['target'] = a['display'].removeprefix('root//')
                candidates = {e['operation'] for e in by_digest.get(a['digest'].replace(':', '-'), [])
                              if abs(e['start_s'] - (a['end'] - a['wall'])) <= 5
                              and e['operation'] in records and peaks.get(e['operation'])}
                if len(candidates) != 1:
                    yield a, None
                    continue
                op = next(iter(candidates))
                if op in seen:
                    continue
                seen.add(op)
                yield a, records[op] | {'pss_peak_bytes': peaks[op]}


def special_requests(traces, paths, since, until):
    identities = {}
    for path in traces:
        with path.open() as source:
            for line in source:
                e = json.loads(line)
                if e.get('event') == 'usage' and since <= e.get('end_s', 0) < until and e.get('sampled') and e.get('outcome') == 'COMPLETED' and 'repo:hirsel-lang|' in e.get('identity', ''):
                    identities[e['operation']] = e
    groups = defaultdict(list)
    seen = set()
    for path in paths:
        with path.open() as source:
            for line in source:
                fields = line.rstrip('\n').split('\t')
                row = dict(f.strip().split('=', 1) for f in fields[1:] if '=' in f)
                op = row.get('op')
                if row.get('category') in ('precheck', 'native_observation', 'clippy') and op in identities and op not in seen and row.get('exit') == '0':
                    groups[row['category']].append(identities[op])
                    seen.add(op)
    return {name: dict(cpu_count=1, memory_kb=max(131072, math.ceil(max(e['peak_memory_kb'] for e in samples) * 1.2 / 65536) * 65536),
                       samples=len(samples), pss_peak_kb=max(e['peak_memory_kb'] for e in samples),
                       cpu_seconds=sum(e.get('cpu_time_ms', 0) for e in samples) / 1000,
                       wall_seconds=sum(e['wall_time_ms'] for e in samples) / 1000)
            for name, samples in sorted(groups.items())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--commands', nargs='+', type=Path)
    parser.add_argument('--pss', nargs='+', type=Path)
    parser.add_argument('--since', type=float, default=0)
    parser.add_argument('--until', type=float, default=float('inf'))
    parser.add_argument('--usage', nargs='+', type=Path, required=True)
    parser.add_argument('-o', '--output', type=Path, required=True)
    args = parser.parse_args()
    if not args.commands or not args.pss:
        parser.error('--commands and --pss are required: size from enforced PSS, not anonymous memory')
    result = sizes(measured_commands(args.commands, args.pss, usage(args.usage), args.since, args.until))
    missing = defaultdict(int)
    for a in result['unmatched']:
        missing[(a['profile'], a['target'], a['phase'])] += 1
    result['unmatched'] = [dict(profile=p, target=t, phase=phase, samples=n)
                           for (p, t, phase), n in sorted(missing.items())]
    result['special_requests'] = special_requests(args.pss, args.usage, args.since, args.until)
    # A quiet profile or on-demand campaign is not evidence that its measured
    # targets became small. Refresh observed rows and retain unobserved ones.
    if args.output.exists():
        previous = json.loads(args.output.read_text())
        for profile in ('dev', 'release'):
            result['profiles'][profile] = previous['profiles'][profile] | result['profiles'][profile]
        result['special_requests'] = previous.get('special_requests', {}) | result['special_requests']
    result['event_logs'] = [p.name for p in args.commands]
    result['measurement_window'] = [args.since, args.until]
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    print(f"sized {sum(map(len, result['profiles'].values()))} targets; {len(result['unmatched'])} unmatched actions")


if __name__ == '__main__':
    main()
