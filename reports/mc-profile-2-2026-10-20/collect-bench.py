"""Collect B1's actual native op timings with M0's sealed measurement toolchain.

The B1 programs' default n=1000 is retained. No timing comparison is made with
M0's session or B1's baseline. These traces only feed placement simulations.
"""
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUT = Path('/tmp/mc-profile-2-bench')
PINNED = Path('/tmp/hirsel-mc-profile')


def main():
    OUT.mkdir(exist_ok=True)
    # Reuse the exact compiler/archive M0 archived hashes for (no source build).
    env = json.loads((PINNED/'environment.json').read_text())
    env['HIRSEL_RUNTIME_ARCHIVES'] = str(PINNED/'pinned/runtime-archives')
    env['HIRSEL_LINK_KIT'] = str(PINNED/'pinned/kit')
    env['HIRSEL_PRECHECK'] = str(PINNED/'pinned/std.precheck')
    llvm = ROOT/'tools/buck2/toolchains/llvm-files'
    env['LLVM_SYS_211_PREFIX'] = str(llvm)
    env['LD_LIBRARY_PATH'] = str(llvm/'lib')
    env['PATH'] = str(llvm/'bin') + ':' + os.environ['PATH']
    env['HIRSEL_BENCH_MEASUREMENT'] = '1'
    helper = PINNED/'pinned/native-build'
    manifest = json.loads((ROOT/'bench/multicore/manifest.json').read_text())
    # Match M0's compiler/precheck source revision instead of rechecking a newer std.
    deps = OUT/'deps'
    deps.mkdir(exist_ok=True)
    archive = subprocess.check_output(['git', 'archive', '8c1675a7a502', 'stdlib'], cwd=ROOT)
    subprocess.run(['tar', '-x', '-C', str(deps)], input=archive, check=True)
    traces = []
    for name in manifest['workloads']:
        source = ROOT/'bench/multicore/programs'/f'{name}.hir'
        if not source.exists():
            continue  # The extern wrapper is R8-owned, as in B1.
        dest = OUT/name
        dest.mkdir(exist_ok=True)
        copied = dest/source.name
        shutil.copy2(source, copied)
        copied.with_suffix('.hir.json').write_text(json.dumps(dict(schema=1,
            package=dict(name='multicore', version='1'), entry=dict(module=[], name='main'),
            sources=[dict(path=copied.name, module=[])], dependencies=[dict(
                manifest=str(deps/'stdlib/std@1/hirsel.json'),
                package=dict(name='std', version='1'), expose=[])])))
        with (dest/'build.log').open('w') as log:
            build = subprocess.run([str(ROOT/'tools/capped'), '--timeout', '900',
                str(helper), str(copied), str(dest/'program'), 'regions-on-whole'],
                env=env, cwd=ROOT, stdout=log, stderr=log)
        if build.returncode:
            raise RuntimeError(f'{name} build failed: {dest}/build.log')
        run_env = dict(env, HIRSEL_RT_MC_PROFILE='1', HIRSEL_RT_MEASUREMENT=str(dest/'measurement'),
                       HIRSEL_RT_ROOT=str(dest/'root'), HIRSEL_RT_TAPE=str(dest/'tape'))
        Path(run_env['HIRSEL_RT_ROOT']).mkdir(exist_ok=True)
        run = subprocess.run([str(ROOT/'tools/capped'), '--timeout', '120', 'taskset', '-c', '31',
                              str(dest/'program')], cwd=ROOT, env=run_env, capture_output=True, check=True)
        n = 1000
        expected = {'8*n': 8*n, '56*n': 56*n, '6*n': 6*n,
                    'n*(n+1)//2': n*(n+1)//2}[manifest['workloads'][name]['checksum']]
        assert run.stdout == f'{expected}\n'.encode(), (name, run.stdout, expected)
        traces.append(dict(program=name, source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                           stdout=run.stdout.decode(), log=(dest/'measurement').read_text()))
        print(name, run.stdout.decode().strip(), flush=True)
    (HERE/'bench-traces.json.gz').write_bytes(gzip.compress(json.dumps(dict(
        schema=1, iterations=1000, std_revision='8c1675a7a502',
        compiler_sha256=hashlib.sha256(helper.read_bytes()).hexdigest(),
        archive_sha256=hashlib.sha256((PINNED/'pinned/runtime-archives/x86_64-unknown-linux-gnu/measurement/libhirsel_rt.a').read_bytes()).hexdigest(),
        traces=traces)).encode(), mtime=0))


if __name__ == '__main__':
    main()
