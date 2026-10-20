#!/usr/bin/env python3
"""Build private std copies with candidate crossovers, then count interleaved runs.

python3 bench/numeric/toom3.py --output /tmp/toom3
Results use bench/micro/run: ten seeded rounds, pinned CPU, perf cycles/instructions.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'bench/native'))
from suite import buck2_build, native_environment  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sizes', default='128,192,256,320,384,512,768,1024,1536,2048')
    parser.add_argument('--thresholds', default='128,256,512,1024,18446744073709551615')
    parser.add_argument('--cpu', type=int, default=31)
    parser.add_argument('--epochs', type=int, default=100)
    parser.add_argument('--build-only', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    toolchain = buck2_build('--target-platforms', '//tools/buck2:release', '//:hirsel-toolchain')['hirsel-toolchain']
    env = native_environment()
    binaries = []
    for threshold in map(int, args.thresholds.split(',')):
        name = 'karatsuba' if threshold == 2**64 - 1 else f'toom-{threshold}'
        directory = output / name
        directory.mkdir(exist_ok=True)
        std = directory / 'stdlib'
        shutil.copytree(ROOT / 'stdlib', std, dirs_exist_ok=True)
        module = std / 'std@1/bigint.hir'
        module.write_text(re.sub(r'const TOOM3_THRESHOLD: u64 = \d+',
                                f'const TOOM3_THRESHOLD: u64 = {threshold}', module.read_text()))
        source = directory / 'multiply.hir'
        shutil.copyfile(ROOT / 'bench/micro/programs/bigint_mul.hir', source)
        source.with_suffix('.hir.json').write_text(json.dumps({
            'schema': 1, 'package': {'name': 'benchmark', 'version': '1'},
            'entry': {'module': [], 'name': 'main'}, 'sources': [{'path': source.name, 'module': []}],
            'dependencies': [{'manifest': str(std / 'std@1/hirsel.json'),
                              'package': {'name': 'std', 'version': '1'}, 'expose': []}]}))
        binary = directory / 'multiply'
        subprocess.run([str(ROOT / 'tools/capped'), '--timeout', '900',
                        str(Path(toolchain) / 'hirsel'), 'build', '--release', str(source), '-o', str(binary)],
                       env=env, check=True)
        binaries.append((name, binary))
    for size in map(int, args.sizes.split(',')):
        participants = [{'name': name, 'command': [str(binary), '--hirsel-grants',
                        str(ROOT / 'bench/micro/grants.json'), str(size), str(args.epochs)]}
                        for name, binary in binaries]
        manifest = output / f'participants-{size}.json'
        manifest.write_text(json.dumps(participants, indent=2) + '\n')
        if not args.build_only:
            subprocess.run([str(ROOT / 'bench/micro/run'), str(manifest), '--output',
                            str(output / f'session-{size}.json'), '--reference', 'karatsuba',
                            '--rounds', '10', '--seed', '42', '--cpu', str(args.cpu)], check=True)


if __name__ == '__main__':
    main()
