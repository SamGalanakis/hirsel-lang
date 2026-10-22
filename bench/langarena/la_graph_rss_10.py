#!/usr/bin/env python3
"""Measure frozen before/candidate/Rust Graph programs, or audit a saved session.

Prior art: compare.py Session.sample and bench/micro/measure.py. This runner adds
an immutable baseline participant without compare.py's incremental source edits.
All programs consume the same upstream run.js; main/harness must be unchanged.
Use release LLVM executables and a Rust release build at 3c9f84ddaeec834c0631747cb789e8cfd602f993.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import random
import re
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'bench/micro'))
import measure  # noqa: E402

TASKS = ('Graph::BFS', 'Graph::DFS', 'Graph::AStar')
LABELS = ('before', 'after', 'rust')
SEED = 20261022


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(data):
    samples = data['samples']
    if len(samples) != 90 or not data.get('freeze_verified'):
        raise ValueError('need 90 passing samples and verified frozen inputs')
    groups = {}
    for task in TASKS:
        groups[task] = {}
        for label in LABELS:
            rows = [s for s in samples if s['task'] == task and s['label'] == label]
            if len(rows) != 10 or {s['round'] for s in rows} != set(range(10)):
                raise ValueError(f'{task}/{label}: incomplete or duplicate rounds')
            for row in rows:
                if row['exit'] != 0 or row['status'] != 'OK':
                    raise ValueError(f'{task}/{label}: failed sample')
                if (not re.search('^'+re.escape(task)+r': OK in [\d.]+s$', row['stdout'], re.M)
                        or not re.search(r'^Summary: .*, 1, 1, 0$', row['stdout'], re.M)):
                    raise ValueError(f'{task}/{label}: missing full-output oracle')
                for event in ('instructions', 'cycles', 'instructions:u', 'cycles:u', 'task-clock'):
                    if row['counters'][event] <= 0 or row['running_percent'][event] != 100:
                        raise ValueError(f'{task}/{label}: PMU coverage incomplete')
            groups[task][label] = {
                'rss_kib': statistics.median(s['rss_kib'] for s in rows),
                'rss_range_kib': [min(s['rss_kib'] for s in rows), max(s['rss_kib'] for s in rows)],
                **{event: statistics.median(s['counters'][event] for s in rows)
                   for event in ('instructions', 'instructions:u')},
            }
    return groups


def run(args):
    session = args.session.resolve()
    session.mkdir(exist_ok=False, parents=True)
    participants = json.loads(args.participants.read_text())
    if set(participants) != set(LABELS):
        raise ValueError('participants JSON needs before, after, rust')
    # Each command ends in its config argument. The task argument is appended.
    # Freeze every source, grant, config, compiler, archive, tool and binary used.
    paths = set()
    for row in participants.values():
        for name in row['inputs']:
            path = Path(name).resolve()
            paths.update([path] if path.is_file() else [p for p in path.rglob('*') if p.is_file()])
    paths.update([Path(__file__).resolve(), ROOT/'bench/micro/measure.py', args.participants.resolve()])
    frozen = {str(p): sha(p) for p in sorted(paths)}
    (session/'freeze.json').write_text(json.dumps(frozen, indent=2)+'\n')
    if args.cpu == 31:
        raise ValueError('CPU31 is reserved; choose an ordinary free CPU')
    # Fail immediately with no held locks, rather than queue behind another lane.
    lock = open(Path('/tmp')/f'hirsel-micro-cpu{args.cpu}.lock', 'a+')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    jobs = [(task, label) for task in TASKS for label in LABELS]
    rng = random.Random(SEED)
    samples = []
    data = dict(cpu=args.cpu, seed=SEED, rounds=10, participants=participants,
                wall_ratios='withheld: ordinary shared CPU', samples=samples)
    for index in range(10):
        rng.shuffle(jobs)
        for task, label in jobs:
            prefix = session/f'{index:02}-{task.replace("::", "-")}-{label}'
            before, after = prefix.with_suffix('.before.stat'), prefix.with_suffix('.after.stat')
            command = ['/usr/bin/time', '-f', '%M', '-o', str(prefix.with_suffix('.rss')),
                       *participants[label]['command'], task]
            command = measure.perf_command(command, args.cpu, prefix.with_suffix('.perf'), before, after)
            # Keep stdout and stderr verbatim; preserve failed groups without publishing a ratio.
            with prefix.with_suffix('.out').open('w') as stream:
                result = subprocess.run(command, cwd=participants[label]['cwd'],
                                        env=dict(os.environ, LC_ALL='C'), stdout=stream, stderr=subprocess.STDOUT)
            output = prefix.with_suffix('.out').read_text()
            counters, running = measure.perf_counts(prefix.with_suffix('.perf').read_text())
            match = re.search('^'+re.escape(task)+r': OK in ([\d.]+)s$', output, re.M)
            row = dict(round=index, label=label, task=task, command=command,
                       exit=result.returncode, status='OK' if result.returncode == 0 and match else 'failed',
                       stdout=output, counters=counters, running_percent=running,
                       rss_kib=int(prefix.with_suffix('.rss').read_text().strip()),
                       seconds=float(match[1]) if match else None)
            samples.append(row)
            (session/'measurements.json').write_text(json.dumps(data, indent=2)+'\n')
            if row['status'] != 'OK':
                raise ValueError(f'{task}/{label}: sample failed; evidence retained')
        for path, expected in frozen.items():
            if sha(path) != expected:
                raise ValueError(f'input changed during session: {path}')
    data['freeze_verified'] = True
    (session/'measurements.json').write_text(json.dumps(data, indent=2)+'\n')
    print(json.dumps(verify(data), indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verify', type=Path, help='audit measurements JSON without rerunning programs')
    parser.add_argument('--participants', type=Path, help='commands, cwd and complete frozen input paths for three participants')
    parser.add_argument('--session', type=Path, help='new output directory, never reused')
    parser.add_argument('--cpu', type=int, default=3)
    args = parser.parse_args()
    if args.verify:
        print(json.dumps(verify(json.loads(args.verify.read_text())), indent=2))
    elif args.participants and args.session:
        run(args)
    else:
        parser.error('use --verify, or --participants and --session')


if __name__ == '__main__':
    main()
