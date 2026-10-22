#!/usr/bin/env python3
"""Ten-round Brainfuck::Recursion instruction campaign using frozen executables.

Before/after executables live beside their frozen hirsel.json, source files,
run.js and grants.json. Reference is the unchanged pinned LangArena Rust
executable. See RESULTS-la-brainfuck-recursion-10.md for build commands.
No wall-clock ratio is reported: this runner uses an ordinary CPU, not CPU31.
"""
import argparse
import json
from pathlib import Path
import random
import subprocess

import compare


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--reference', type=Path, required=True)
    parser.add_argument('--reference-root', type=Path, required=True)
    parser.add_argument('--toolchain-identities', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cpu', type=int, required=True)
    parser.add_argument('--rounds', type=int, default=10)
    parser.add_argument('--seed', type=int, default=20261022)
    args = parser.parse_args()
    if args.cpu == 31 or args.rounds < 10:
        parser.error('use an ordinary CPU other than reserved CPU31 and at least ten rounds')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {name: getattr(args, name).resolve() for name in ('before', 'after', 'reference')}
    root = compare.ROOT
    tracked = subprocess.check_output(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=root, text=True
    ).splitlines()
    paths = [root / path for path in tracked if (root / path).is_file()]
    for name in ('before', 'after'):
        paths += [p for p in binaries[name].parent.iterdir() if p.is_file()]
    paths += [*binaries.values(), args.toolchain_identities.resolve()]
    paths += [p for p in (args.reference_root / 'src').rglob('*') if p.is_file()]
    paths += [args.reference_root / name for name in ('Cargo.toml', 'Cargo.lock')]
    frozen = {str(p.resolve()): compare.sha(p) for p in paths}
    toolchain = json.loads(args.toolchain_identities.read_text())

    def verify():
        for path, digest in frozen.items():
            if compare.sha(path) != digest:
                raise RuntimeError('consumed input changed: ' + path)

    def verify_tools():
        recorded = [toolchain['compiler'], *toolchain['runtime'].values()]
        for entry in recorded:
            if compare.identity(entry['path']) != entry:
                raise RuntimeError('compiler/runtime input changed: ' + entry['path'])

    verify_tools()
    config = binaries['before'].parent / 'run.js'
    selected = next(row for row in json.loads(config.read_text())
                    if row['name'] == 'Brainfuck::Recursion')
    if selected['checksum'] != 954437102:
        raise RuntimeError('expected unchanged run.js Brainfuck checksum')
    if config.read_bytes() != (binaries['after'].parent / 'run.js').read_bytes():
        raise RuntimeError('participant configs differ')
    data = dict(seed=args.seed, rounds=args.rounds, cpu=args.cpu,
                scope='whole process including startup, preparation, warmup; no timing lock',
                config=selected, inputs=frozen, samples=[], status='running')
    (args.output / 'identities.json').write_text(json.dumps(frozen, indent=2) + '\n')
    rng = random.Random(args.seed)
    for index in range(args.rounds):
        jobs = list(binaries)
        rng.shuffle(jobs)
        for name in jobs:
            verify()
            label = f'{index:02d}-{name}'
            counters = args.output / (label + '.perf')
            binary = binaries[name]
            command = [str(binary)]
            if name != 'reference':
                command += ['--hirsel-grants', str(binary.parent / 'grants.json')]
            command += [str(config) if name == 'reference' else 'run.js', 'Brainfuck::Recursion']
            command = [str(root / 'tools/capped'), '--timeout', '300', 'nice', '-n', '10',
                       'taskset', '-c', str(args.cpu), 'perf', 'stat', '-x,', '-o', str(counters),
                       '-e', 'instructions,cycles,instructions:u,cycles:u', '--', *command]
            with (args.output / (label + '.out')).open('w') as log:
                result = subprocess.run(command, cwd=binary.parent, stdout=log, stderr=subprocess.STDOUT)
                log.write(f'EXIT {result.returncode}\n')
            text = (args.output / (label + '.out')).read_text()
            sample = dict(round=index, participant=name, argv=command, exit=result.returncode,
                          counters=counters.read_text(), status='failed')
            if result.returncode == 0:
                sample['seconds'] = compare.parse_result(text, 'Brainfuck::Recursion')
                if 'Summary:' in text and ', 1, 1, 0' in text:
                    sample['status'] = 'OK'
            data['samples'].append(sample)
            (args.output / 'measurements.json').write_text(json.dumps(data, indent=2) + '\n')
            if sample['status'] != 'OK':
                raise RuntimeError(f'failed sample {label}; retained separately')
            verify()
            print(label, 'OK', flush=True)
    verify_tools()
    data['status'] = 'complete'
    data['verified_after'] = True
    (args.output / 'measurements.json').write_text(json.dumps(data, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
