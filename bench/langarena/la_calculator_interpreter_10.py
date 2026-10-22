#!/usr/bin/env python3
"""Measure frozen Calculator executables without rebuilding or changing inputs.

The JSON spec contains `inputs` (files/directories to hash), `participants`
(name -> command, cwd, optional allocation_command), and optional
`allocation_env` (environment for the separate instrumented executables).
Commands must select exactly Calculator::Interpreter with run.js. Sources,
compiler, runtime, reference, grants and config belong in inputs, including
untracked generated manifests. See RESULTS-la-calculator-interpreter-10.md.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import random
import statistics
import subprocess

import compare

measure = compare.measure
TASK = 'Calculator::Interpreter'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--spec', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cpu', type=int, default=18)
    parser.add_argument('--rounds', type=int, default=10)
    parser.add_argument('--seed', type=int, default=20261022)
    args = parser.parse_args()
    if args.cpu == 31 or args.rounds != 10:
        parser.error('use an ordinary CPU other than reserved CPU31, and ten rounds')
    if args.output.exists():
        parser.error('output already exists; retain each attempt under a new name')
    spec = json.loads(args.spec.read_text())
    participants = spec['participants']
    if set(participants) != {'before', 'candidate', 'rust'}:
        parser.error('expected frozen before, candidate and rust participants')
    paths = [args.spec, Path(__file__), compare.HERE / 'compare.py',
             compare.ROOT / 'bench/micro/measure.py', compare.ROOT / 'tools/capped']
    paths += [Path(p) for p in spec['inputs']]
    for part in participants.values():
        for key in ('command', 'allocation_command'):
            if key not in part:
                continue
            command = part[key]
            if command[-1] != TASK or not command[-2].endswith('run.js'):
                parser.error('commands must select the full Interpreter workload')
            paths.append(Path(command[0]))
    freeze = {str(p.resolve()): compare.identity(p) for p in paths}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    logs = args.output.with_suffix('.logs')
    logs.mkdir()
    data = dict(cpu=args.cpu, rounds=args.rounds, seed=args.seed,
                inputs=freeze, participants=participants, samples=[], allocations={},
                frozen_identity_verified=False,
                scope='isolated benchmark process tree including startup, preparation, '
                      'warmup, timed iterations and destruction',
                wall_timing='unlocked ordinary CPU; no wall or cycle speedup claim')

    def save():
        args.output.write_text(json.dumps(data, indent=2) + '\n')

    def run(command, part, label, env=None):
        path = logs / (label + '.log')
        with path.open('w') as stream:
            child = subprocess.run(
                [str(compare.ROOT / 'tools/capped'), '--timeout', '900',
                 'nice', '-n', '10', *map(str, command)], cwd=part['cwd'],
                env=env, stdout=stream, stderr=subprocess.STDOUT)
            stream.write(f'EXIT {child.returncode}\n')
        output = path.read_text()
        if child.returncode:
            raise RuntimeError(f'{path}: child exit {child.returncode}')
        compare.parse_result(output, TASK)
        if not any(line.startswith('Summary: ') and line.endswith(', 1, 1, 0')
                   for line in output.splitlines()):
            raise RuntimeError(f'{path}: expected one selected, passing benchmark')
        return str(path)

    # A nonblocking observation takes no CPU lease and never uses CPU31.
    with measure.TIMING_LOCK.open('a+') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            data['timing_lock'] = 'available; immediately released, no timing campaign'
            fcntl.flock(lock, fcntl.LOCK_UN)
        except BlockingIOError:
            data['timing_lock'] = 'busy; no wait and no CPU lock held'
    save()
    for name, part in participants.items():
        run(part['command'], part, 'oracle-' + name)
        if 'allocation_command' in part:
            target = logs / (name + '-allocations.txt')
            env = os.environ | spec.get('allocation_env', {})
            env['HIRSEL_RT_MEASUREMENT'] = str(target.resolve())
            run(part['allocation_command'], part, 'allocation-' + name, env)
            data['allocations'][name] = json.loads(
                Path(str(target) + '.sites.json').read_text())['totals']
            save()
    rng = random.Random(args.seed)
    for iteration in range(args.rounds):
        order = list(participants)
        rng.shuffle(order)
        for name in order:
            part = participants[name]
            label = f'{iteration}-{name}'
            counters = logs / (label + '.perf')
            before = logs / (label + '.before.stat')
            after = logs / (label + '.after.stat')
            command = measure.perf_command(part['command'], args.cpu, counters, before, after)
            log = run(command, part, label)
            counts, running = measure.perf_counts(counters.read_text())
            sample = dict(round=iteration, participant=name, counts=counts,
                          running_percent=running, log=log)
            siblings = measure.siblings(args.cpu)
            sample['sibling_before'] = measure.cpu_snapshot(siblings, before)
            sample['sibling_after'] = measure.cpu_snapshot(siblings, after)
            sample['sibling_busy_share'] = measure.busy_share(
                sample['sibling_before'], sample['sibling_after'])
            data['samples'].append(sample)
            save()
            print(iteration, name, int(counts['instructions']), flush=True)
    for path, identity in freeze.items():
        if compare.identity(path) != identity:
            raise RuntimeError('consumed input changed: ' + path)
    data['frozen_identity_verified'] = True
    data['summary'] = {
        name: {event: statistics.median(s['counts'][event] for s in data['samples']
                                       if s['participant'] == name)
               for event in measure.EVENTS} for name in participants}
    save()
    print(json.dumps(data['summary'], indent=2), flush=True)


if __name__ == '__main__':
    main()
