#!/usr/bin/env python3
"""Interleave frozen NeuralNet before/after/Rust binaries; retain every sample.

The packages must contain the original run.js/test.js and the grants must mount
that package root. Build before.hir from the lane-private CLI fixture and the
current neural_net.hir with identical main/harness/registry, compiler and std.
See RESULTS-la-neural-net-10.md for the preparation and allocation boundaries.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile

import compare

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', nargs=3, metavar=('BINARY', 'GRANTS', 'ROOT'), required=True)
    parser.add_argument('--after', nargs=3, metavar=('BINARY', 'GRANTS', 'ROOT'), required=True)
    parser.add_argument('--rust', nargs=2, metavar=('BINARY', 'ROOT'), required=True)
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    parser.add_argument('--identity', type=Path, action='append', default=[], help='additional frozen consumed input')
    parser.add_argument('--cpu', type=int, default=10)
    parser.add_argument('--seed', type=int, default=20261022)
    parser.add_argument('--instructions-only', action='store_true', help='skip an occupied timing lock; no wall ratios')
    args = parser.parse_args()
    if args.cpu == 31:
        parser.error('CPU31 is reserved for other lanes')
    args.output.mkdir(parents=True, exist_ok=False)
    participants = {}
    for name in ('before', 'after'):
        binary, grants, directory = map(lambda p: str(Path(p).resolve()), getattr(args, name))
        participants[name] = dict(binary=binary, args=['--hirsel-grants', grants], cwd=directory)
    binary, directory = map(lambda p: str(Path(p).resolve()), args.rust)
    participants['rust'] = dict(binary=binary, args=[], cwd=directory)
    paths = [Path(__file__), ROOT / 'tools/capped', ROOT / 'bench/langarena/compare.py',
             ROOT / 'bench/micro/measure.py', *args.identity]
    for name, participant in participants.items():
        paths.append(Path(participant['binary']))
        if name != 'rust':
            paths.extend([Path(participant['args'][1]), Path(participant['cwd']) / 'run.js',
                          Path(participant['cwd']) / 'test.js'])
    identities = {str(p.resolve()): compare.identity(p) for p in paths}
    result = dict(seed=args.seed, cpu=args.cpu, rounds=10, participants=participants,
                  identities=identities, samples=[], correctness=[], complete=False,
                  scope='whole process including startup, preparation, warmup and checksum')

    def save():
        (args.output / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')

    def verify():
        result['verified_frozen'] = {p: compare.identity(p) == identity for p, identity in identities.items()}
        save()
        if not all(result['verified_frozen'].values()):
            raise RuntimeError('consumed inputs changed')

    def sample(phase, config, label, measured):
        participant = participants[phase]
        configuration = config if phase != 'rust' else str(Path(participants['before']['cwd']) / config)
        command = [participant['binary'], *participant['args'], configuration, 'Etc::NeuralNet']
        counters = args.output / (label + '.perf')
        rss = args.output / (label + '.rss')
        if measured:
            command = ['perf', 'stat', '-x', ';', '-o', str(counters), '-e',
                       'task-clock,cycles,instructions,cycles:u,instructions:u', '--',
                       '/usr/bin/time', '-f', '%M', '-o', str(rss), *command]
        command = ['nice', '-n', '10', str(ROOT / 'tools/capped'), '--timeout', '900',
                   'taskset', '-c', str(args.cpu), *command]
        row = dict(phase=phase, config=config, argv=command, cwd=participant['cwd'],
                   stat_before=Path('/proc/stat').read_text())
        log_path = args.output / (label + '.log')
        with log_path.open('w') as log:
            process = subprocess.run(command, cwd=participant['cwd'], stdout=log, stderr=subprocess.STDOUT)
            log.write(f'\nEXIT {process.returncode}\n')
        row.update(exit=process.returncode, stat_after=Path('/proc/stat').read_text(), log=str(log_path))
        result['samples' if measured else 'correctness'].append(row)
        save()
        if process.returncode:
            raise RuntimeError(f'{label} failed; see {log_path}')
        row['seconds'] = compare.parse_result(log_path.read_text(), 'Etc::NeuralNet')
        if measured:
            row['counts'], row['coverage'] = compare.measure.perf_counts(counters.read_text())
            row['rss_kib'] = int(rss.read_text().strip())
            if any(percent != 100 for percent in row['coverage'].values()):
                save()
                raise RuntimeError(f'{label}: incomplete PMU coverage')
        save()
        print(label, row.get('counts', {}).get('instructions'), flush=True)

    save()
    # The source configurations, not shortened timed runs, define the oracles.
    for phase in participants:
        sample(phase, 'test.js', 'correctness-' + phase, False)
    lock_path = Path(tempfile.gettempdir()) / f'hirsel-micro-cpu{args.cpu}.lock'
    lock = lock_path.open('a+')
    try:
        if not args.instructions_only:
            # Fail immediately if occupied; never hold one lock waiting on another.
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result.update(timing_lock=str(lock_path), locked=not args.instructions_only)
        rng = random.Random(args.seed)
        verify()
        for index in range(10):
            jobs = list(participants)
            rng.shuffle(jobs)
            for phase in jobs:
                sample(phase, 'run.js', f'r{index:02}-{phase}', True)
                result['samples'][-1]['round'] = index
            verify()
        result['complete'] = True
        save()
    finally:
        lock.close()


if __name__ == '__main__':
    main()
