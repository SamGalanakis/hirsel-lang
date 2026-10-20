#!/usr/bin/env python3
"""Run repeated YAML cases with two workers; publish evidence even on failure."""
import argparse
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--tool', nargs=2, action='append', default=[])
    parser.add_argument('--repetitions', type=int, default=8)
    parser.add_argument('--shard', type=int, default=1)
    args = parser.parse_args()
    if args.repetitions < 1 or args.shard < 1:
        parser.error('repetitions and shard must be positive')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    tools = {name: Path(path).resolve() for name, path in args.tool}
    env = os.environ | {
        'LLVM_SYS_211_PREFIX': str(tools['llvm']),
        'LD_LIBRARY_PATH': str(tools['llvm'] / 'lib'),
        'HIRSEL_LINK_KIT': str(tools['kit']),
        'HIRSEL_GLIBC_FLOOR': str(tools['glibc_floor']),
        'HIRSEL_RUNTIME_ARCHIVES': str(tools['archives']),
        'HIRSEL_NATIVE_TIMEOUT_DIR': str(output / 'timeouts'),
        'HIRSEL_JOBS': '2',
    }
    if 'precheck' in tools:
        env['HIRSEL_PRECHECK'] = str(tools['precheck'])
    # The fixture grants require a root whose ancestry the sandbox can resolve.
    tmpfs = '/dev/shm' if os.access('/dev/shm', os.W_OK) else None
    with tempfile.TemporaryDirectory(prefix='hirsel-timeouts-', dir=tmpfs) as scratch:
        scratch = Path(scratch)
        corpus = scratch / 'corpus'
        shutil.copytree(args.root, corpus)
        native = scratch / 'native'
        shutil.copy2(args.native, native)
        cases = output / 'cases'
        cases.mkdir()
        prefix = f'timeout-repro-{args.shard}-'
        for index in range(args.repetitions):
            for case in ('crlf', 'explicit'):
                source = corpus / f'conformance/SEM/SEM-3-yaml-edit-{case}-run.hir'
                name = f'{prefix}{index:03}-{case}.hir'
                shutil.copy2(source, corpus / 'native-tests/regressions' / name)
                shutil.copy2(source, cases / name)
                sidecar = source.with_name(source.name + '.json')
                if sidecar.exists():
                    manifest = json.loads(sidecar.read_text())
                    for item in manifest['sources']:
                        if item['path'] == source.name:
                            item['path'] = name
                    # conformance/SEM and native-tests/regressions have the
                    # same depth, so dependency paths keep their meaning.
                    text = json.dumps(manifest, indent=2) + '\n'
                    (corpus / 'native-tests/regressions' / (name + '.json')).write_text(text)
                    (cases / (name + '.json')).write_text(text)
        command = [str(native), '--root', str(corpus), '--cranelift', '--jobs', '2', '--filter', prefix]
        started = time.monotonic()
        with (output / 'campaign.log').open('w') as log:
            result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=False)
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        (output / 'result.json').write_text(json.dumps({
            'exit_code': result.returncode, 'cases': args.repetitions * 2, 'jobs': 2,
            'wall_seconds': time.monotonic() - started,
            'child_user_seconds': usage.ru_utime, 'child_system_seconds': usage.ru_stime,
        }, indent=2) + '\n')
        if (scratch / 'native-gate').exists():
            shutil.copytree(scratch / 'native-gate', output / 'native-gate')
    print(f'native timeout diagnostics: {output}; inspect result.json and campaign.log')


if __name__ == '__main__':
    main()
