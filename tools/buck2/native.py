#!/usr/bin/env python3
"""Run exact native program IDs on both backends in cost-bounded Buck2 shards."""
import argparse
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


# A third of the action timeout leaves room for setup and host variance. Unknown
# programs get a whole budget; a missing timing must never make a large action.
NATIVE_SLICE_SECONDS = 600


def native_slices(programs, root=ROOT) -> list[list[str]]:
    costs = {}
    for target in ('gate-native-changed', 'gate-native-cranelift-changed'):
        table = root / 'tools/buck2/test-costs' / f'{target}.tsv'
        if table.exists():
            for line in table.read_text().splitlines():
                name, seconds = line.split('\t')
                cost = float(seconds)
                if not 0 < cost < float('inf'):
                    raise ValueError(f'{table}: invalid cost for {name}: {seconds}')
                costs[name] = max(costs.get(name, 0), cost)
    # Longest first, as in test_launcher.sh; bins have a bounded load instead
    # of a fixed count. An indivisible case over budget gets its own action.
    bins, loads = [], []
    for program in sorted(programs, key=lambda name: (-costs.get(name, NATIVE_SLICE_SECONDS), name)):
        cost = costs.get(program, NATIVE_SLICE_SECONDS)
        fits = [i for i, load in enumerate(loads) if load + cost <= NATIVE_SLICE_SECONDS]
        if fits:
            at = min(fits, key=lambda i: (loads[i], i))
            bins[at].append(program)
            loads[at] += cost
        else:
            bins.append([program])
            loads.append(cost)
    return [sorted(part) for part in bins]


NATIVE_TARGETS = ['root//:gate-native-changed', 'root//:gate-native-cranelift-changed']


def config(programs, root=ROOT):
    return ['-c', 'hirsel.native_filters=' + ';'.join('|'.join(part) for part in native_slices(programs, root))]


def targets(programs, root=ROOT):
    count = len(native_slices(programs, root))
    return [target if count == 1 else f'{target}-shard-{i}'
            for target in NATIVE_TARGETS for i in range(1, count + 1)]


def command(programs, root=ROOT):
    return [sys.executable, str(root / 'tools/buck2/event_logs.py'), 'test',
            *os.environ.get('HIRSEL_BUCK2_CONFIG', '').split(), *config(programs, root), *targets(programs, root),
            '--', '--timeout', '1800']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('programs', nargs='+', help='exact native program IDs')
    args = parser.parse_args()
    programs = set(args.programs)
    print(f'native: {len(programs)} programs in {len(targets(programs))} backend actions', flush=True)
    return subprocess.call(command(programs), cwd=ROOT)


if __name__ == '__main__':
    sys.exit(main())
