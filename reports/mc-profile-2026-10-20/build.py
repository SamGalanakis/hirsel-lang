"""Seal measurement executables and pin their tool inputs outside buck-out.

Usage: python3 reports/mc-profile-2026-10-20/build.py /absolute/output/directory
The application is launched through the existing standalone C admission API;
all other cases use the native fixture profile. No runtime ABI is modified.
"""
import json
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'bench/native'))
from suite import native_environment, buck2_build

out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
env = native_environment({'HIRSEL_BENCH_MEASUREMENT': '1'})
tools = buck2_build('//:hirsel-native-build-bin-hirsel-native-build',
                   '//:hirsel-std-precheck', '//:hirsel-rt-archives')
pinned = out / 'pinned'
pinned.mkdir(exist_ok=True)
helper = pinned / 'native-build'
shutil.copy2(tools['hirsel-native-build-bin-hirsel-native-build'], helper)
shutil.copy2(tools['hirsel-std-precheck'], pinned / 'std.precheck')
archive_relative = Path('x86_64-unknown-linux-gnu/measurement/libhirsel_rt.a')
archive = pinned / 'runtime-archives' / archive_relative
archive.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(Path(tools['hirsel-rt-archives']) / archive_relative, archive)
shutil.copytree(env['HIRSEL_LINK_KIT'], pinned / 'kit', dirs_exist_ok=True)
llvm = root / 'tools/buck2/toolchains/llvm-files'
env.update(HIRSEL_RUNTIME_ARCHIVES=str(pinned / 'runtime-archives'),
           HIRSEL_LINK_KIT=str(pinned / 'kit'), HIRSEL_PRECHECK=str(pinned / 'std.precheck'),
           LLVM_SYS_211_PREFIX=str(llvm), LD_LIBRARY_PATH=str(llvm / 'lib'),
           PATH=str(llvm / 'bin') + ':' + env['PATH'])
(out / 'environment.json').write_text(json.dumps(env))

programs = ['examples/tasks/hirsel.json',
            'conformance/CONC/CONC-2-task-all-ok-run.hir',
            'conformance/MEM/MEM-11-tour-11-shared-config-run.hir',
            'conformance/CONC/CONC-2-task-select-fast-slow-run.hir',
            'conformance/CONC/CONC-3-check-canceled-run.hir',
            'conformance/CLO/CLO-3-child-mutex-capture-run.hir',
            'bench/native/programs/controls/resource-drops.hir']
for source in programs:
    name = 'tasks-app' if source.endswith('hirsel.json') else Path(source).stem
    dest = out / name
    dest.mkdir(exist_ok=True)
    with (dest / 'build.stdout').open('w') as stdout, (dest / 'build.stderr').open('w') as stderr:
        result = subprocess.run([str(root / 'tools/capped'), '--timeout', '900', str(helper),
                                 str(root / source), str(dest / 'program'), 'regions-on-whole'],
                                env=env, cwd=root, stdout=stdout, stderr=stderr)
    if result.returncode:
        raise SystemExit(f'{name}: build failed; see {dest / "build.stderr"}')
    if name == 'tasks-app':
        # The benchmark builder emits a bound fixture launcher. Reuse its sealed
        # objects with a C host that selects fresh grants and process stdin.
        objects = dest / 'measurement-objects'
        shutil.copy2(dest / 'program', dest / 'program-bound')
        subprocess.run([str(root / 'tools/capped'), str(llvm / 'bin/clang'), '-O2',
                        '-I', str(root / 'crates/hirsel-rt/include'),
                        str(Path(__file__).with_name('standalone.c')),
                        str(objects / 'main.o'), str(objects / 'unit-0.o'), str(archive),
                        '-fuse-ld=lld', '-Wl,--wrap=hirsel_rt_v2_main',
                        '-lgcc_s', '-lutil', '-lrt', '-lpthread', '-lm', '-ldl',
                        '-o', str(dest / 'program')], env=env, check=True)
    print(name, 'built', flush=True)
