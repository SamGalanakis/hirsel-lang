#!/usr/bin/env python3
"""Derive the registry and package manifest, then build a native executable."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def groups():
    result = {}
    for path in sorted(HERE.glob('*.hir')):
        names = re.findall(r'^// langarena: (\S+)\s*$', path.read_text(), re.M)
        if names:
            if not re.fullmatch(r'[a-z][a-z0-9_]*', path.stem):
                raise ValueError(f'invalid module name: {path.name}')
            result[path.stem] = names
    flat = [name for names in result.values() for name in names]
    if len(set(flat)) != len(flat):
        raise ValueError('duplicate task registration')
    known = {task['name'] for task in json.loads((HERE / 'run.js').read_text())}
    if set(flat) - known:
        raise ValueError(f'unknown tasks: {set(flat) - known}')
    return result


def generate():
    modules = groups()
    directory = HERE / 'build'
    directory.mkdir(exist_ok=True)
    cases = [(name, module) for module, names in modules.items() for name in names]
    registry = 'pub fn contains(name: String) -> bool {\n  ' + ' || '.join(
        f'name == {json.dumps(name)}' for name, _ in cases) + '\n}\n\n'
    registry += ('pub fn execute(rt: Io, name: String, config: harness.Config) '
                 '-> Result[harness.Report, String] {\n')
    for name, module in cases:
        registry += f'  if name == {json.dumps(name)} {{ return {module}.execute(rt, name, config) }}\n'
    registry += '  Error("task is not registered")\n}\n'
    (directory / 'registry.hir').write_text(registry)
    sources = [dict(path='../main.hir', module=[]),
               dict(path='../harness.hir', module=['harness']),
               dict(path='../cpu_harness.hir', module=['cpu_harness']),
               dict(path='registry.hir', module=['registry'])]
    sources += [dict(path=f'../{module}.hir', module=[module]) for module in modules]
    manifest = dict(schema=1, package=dict(name='langarena', version='1'),
                    entry=dict(module=[], name='main'), sources=sources,
                    dependencies=[dict(manifest='../../../stdlib/std@1/hirsel.json',
                                       package=dict(name='std', version='1'), expose=[])])
    path = directory / 'hirsel.json'
    path.write_text(json.dumps(manifest, indent=2) + '\n')
    grants = dict(io=dict(tasks='tasks.free@1',
                          cpu=dict(provider='cpu.scoped@1', max_workers=16, max_domains=1,
                                   max_physical_bytes=1073741824, max_batch_bytes=16777216),
                          stdout='stdout.bytes@1', monotonic_clock='clock.monotonic.ns@1',
                          wall_clock='clock.wall.unix_ns@1',
                          dirs=[dict(path='', host=str(ROOT), access='read')]), args=[])
    (directory / 'grants.json').write_text(json.dumps(grants, indent=2) + '\n')
    return path


def compiler():
    if os.environ.get('HIRSEL'):
        return Path(os.environ['HIRSEL']).resolve()
    subprocess.run(['python3', 'tools/buck2/prepare.py'], cwd=ROOT, check=True)
    subprocess.run(['python3', 'tools/buck2/sync.py', '--if-stale'], cwd=ROOT, check=True)
    result = subprocess.run([str(ROOT / '.buck2/bin/buck2'), 'build',
                             '--target-platforms', '//tools/buck2:release',
                             '//:hirsel-cli-bin-hirsel', '--show-json-output'],
                            cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    outputs = json.loads(result.stdout)
    return ROOT / next(iter(outputs.values()))


def native_environment():
    """Use the same declared LLVM/link inputs as bench/native/suite.py:159-173."""
    targets = ['//tools/buck2/native-toolchain:' + name for name in ('llvm', 'kit', 'glibc_floor')]
    targets.append('//:hirsel-rt-archives')
    result = subprocess.run([str(ROOT / '.buck2/bin/buck2'), 'build', *targets,
                             '--show-json-output'], cwd=ROOT, text=True,
                            stdout=subprocess.PIPE, check=True)
    paths = {key.rsplit(':', 1)[-1]: str((ROOT / path).resolve())
             for key, path in json.loads(result.stdout).items()}
    env = os.environ.copy()
    env.update(LLVM_SYS_231_PREFIX=paths['llvm'], HIRSEL_RUNTIME_ARCHIVES=paths['hirsel-rt-archives'],
               HIRSEL_LINK_KIT=paths['kit'], HIRSEL_GLIBC_FLOOR=paths['glibc_floor'],
               PATH=paths['llvm'] + '/bin:' + env.get('PATH', '/usr/bin:/bin'),
               LD_LIBRARY_PATH=paths['llvm'] + '/lib:' + env.get('LD_LIBRARY_PATH', ''))
    return env


def build(cli, output=None, debug=False):
    manifest = generate()
    output = output or HERE / 'build/langarena'
    subprocess.run([str(cli), 'build', '--debug' if debug else '--release',
                    str(manifest), '-o', str(output)], cwd=ROOT, env=native_environment(), check=True)
    return output


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--generate-only', action='store_true')
    parser.add_argument('--debug', action='store_true')
    args = parser.parse_args()
    if args.generate_only:
        print(generate())
    else:
        print(build(compiler(), debug=args.debug))
