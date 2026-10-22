#!/usr/bin/env python3
"""Ten-round instruction comparison of frozen Sieve executables.

Build each participant first. This runner never edits sources, requests CPU31,
waits for timing locks, or reports wall-clock speedups. Production run.js and
correctness test.js retain their original preparation/warmup/checksum boundaries.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('before', 'after', 'bound', 'rust', 'grants', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--cpu', type=int, default=12)
    parser.add_argument('--seed', type=int, default=20261022)
    args = parser.parse_args()
    if args.cpu == 31:
        parser.error('CPU31 requires the separate exclusive timing runner')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {name: getattr(args, name).resolve()
                for name in ('before', 'after', 'bound', 'rust')}
    frozen = {str(p): sha(p) for p in [*binaries.values(), args.grants.resolve(),
              ROOT / 'bench/langarena/run.js', ROOT / 'bench/langarena/test.js',
              Path(__file__).resolve()]}
    (args.output / 'frozen.json').write_text(json.dumps(frozen, indent=2) + '\n')
    rng = random.Random(args.seed)
    with (args.output / 'samples.jsonl').open('w') as data:
        for config in ('test.js', 'run.js'):
            for round_id in range(10):
                order = list(binaries)
                rng.shuffle(order)
                for name in order:
                    label = f'{config}-{round_id}-{name}'
                    counter = args.output.resolve() / (label + '.perf')
                    argv = [str(binaries[name])]
                    if name != 'rust':
                        argv += ['--hirsel-grants', str(args.grants.resolve())]
                    argv += ['bench/langarena/' + config, 'Etc::Sieve']
                    command = ['nice', '-n', '10', str(ROOT / 'tools/capped'),
                               'taskset', '-c', str(args.cpu), 'perf', 'stat',
                               '--no-big-num', '-x', ';', '-e', 'instructions,cycles',
                               '-o', str(counter), '--', *argv]
                    result = subprocess.run(command, cwd=ROOT, text=True,
                                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                            env=os.environ.copy())
                    row = dict(config=config, round=round_id, participant=name,
                               cpu=args.cpu, seed=args.seed, command=command,
                               exit=result.returncode, output=result.stdout,
                               perf=counter.read_text() if counter.exists() else '')
                    data.write(json.dumps(row) + '\n')
                    data.flush()  # Failed attempts stay visible too.
                    assert result.returncode == 0, row
                    assert re.search(r'^Etc::Sieve: OK in [\d.]+s$', result.stdout, re.M), row
                    for event in ('instructions', 'cycles'):
                        line = next(line for line in row['perf'].splitlines()
                                    if ';' + event + ';' in line)
                        float(line.split(';')[0])  # Fail closed on denied counters.
                assert all(sha(path) == digest for path, digest in frozen.items())


if __name__ == '__main__':
    main()
