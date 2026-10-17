"""Shared frozen benchmark contract and build/process helpers (Python stdlib)."""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import platform
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
LANGUAGES = ('c', 'rust')
VARIANTS = ('system', 'mimalloc', 'arena')
TARGETS = ('x86_64-linux-gnu', 'aarch64-linux-gnu')
PRIMARY = ('regions-on-whole', 'regions-on-singleton')
HIRSEL = PRIMARY + ('regions-off-size-class', 'regions-off-system')
PROTOCOL = {'warmups':5,'rounds':30,'minimum_seconds':0.250,'order_seed':5067074,
            'bootstrap_seed':129507384,'resamples':10000,'upper_quantile':0.95,'gate':1.25,'maximum_upper_inflation':1.10}
SOURCE_FILES = ('c/binarytrees-arena.h','c/main.c','rust/src/main.rs','rust/Cargo.toml','rust/Cargo.lock',
                'rust/rust-toolchain.toml',
                'suite.py','generate','run','analyse','analysis.py','allocators.py',
                'build-hirsel','../../crates/hirsel-native-build/Cargo.toml',
                '../../crates/hirsel-native-build/src/main.rs',
                'programs/common.hir','programs/binarytrees.hir','programs/records-messages-scalar.hir',
                'check-program','programs/saxpy-checked-integer.hir','programs/slots-churn.hir',
                'programs/closure-iter-scoped.hir','programs/closure-iter-erased.hir',
                'programs/resource-failure-throughput.hir','programs/resource-failure-heavy.hir',
                'programs/expr-compiler.hir','programs/batch-validation.hir',
                'programs/json-roundtrip.hir','programs/wordfreq.hir',
                'programs/persistent-list-map-unique.hir','programs/persistent-list-map-shared.hir',
                'programs/records-messages-flat.hir','programs/saxpy-checked-f32.hir',
                'programs/saxpy-checked-f64.hir')

BINARYTREES_GATE = {'ruling':'2026-10-15', 'heap':['c/system','c/mimalloc'],
                    'arena':['c/system','c/mimalloc','c/arena']}
C_ARENA_CASES = ('binarytrees',)
ARENA_CASES = ('binarytrees','expr-compiler','json-roundtrip','wordfreq',
               'records-messages/flat','batch-validation','persistent-list-map/unique')

def arena_exclusions():
    return {c:('Retained shared roots have independent release lifetimes.' if c.endswith('/shared') else
               'Compute or traversal dominates; no allocation-heavy arena reference is registered.')
            for c in load_generator().CASES if c not in ARENA_CASES}

def load_generator():
    loader=importlib.machinery.SourceFileLoader('native_generate',str(ROOT/'generate'))
    spec=importlib.util.spec_from_loader(loader.name,loader)
    module=importlib.util.module_from_spec(spec);loader.exec_module(module);return module

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def load_manifest(m=None, check_files=True):
    m=json.loads((ROOT/'manifest.json').read_text()) if m is None else m
    assert m['schema']==1 and m['protocol']==PROTOCOL,'unrecognized measurement contract'
    assert tuple(m['required_targets'])==TARGETS,'missing required target'
    assert tuple(m['languages'])==LANGUAGES and tuple(m['variants'])==VARIANTS,'missing reference or variant'
    assert tuple(m['hirsel_primary'])==PRIMARY and tuple(m['hirsel_configurations'])==HIRSEL
    expected={(c,s) for c in load_generator().CASES for s in ('small','medium','large')}
    assert {(c['case'],c['size']) for c in m['cases']}==expected and len(m['cases'])==len(expected),'missing case'
    assert tuple(m['rust_arena_cases'])==ARENA_CASES
    assert m['arena_exclusions']==arena_exclusions()
    assert tuple(m.get('c_arena_cases', ())) in ((), C_ARENA_CASES)
    assert m.get('binarytrees_gate') in (None, BINARYTREES_GATE), 'unknown binarytrees gate'
    if not check_files:return m
    assert m['binarytrees_gate']==BINARYTREES_GATE
    assert tuple(m['c_arena_cases'])==C_ARENA_CASES
    for c in m['cases']:
        assert sha(ROOT/c['input'])==c['input_sha256'] and sha(ROOT/c['oracle'])==c['oracle_sha256'],'fixture changed'
        assert f"{load_generator().digest((ROOT/c['oracle']).read_bytes()):016x}"==c['checksum']
    for p,h in m['source_sha256'].items():
        assert sha(ROOT/p)==h,f'source changed: {p}; freeze the manifest before evaluation'
    assert set(m['source_sha256'])==set(SOURCE_FILES)
    return m

def freeze():
    m=json.loads((ROOT/'manifest.json').read_text())
    m.update(schema=1,protocol=PROTOCOL,required_targets=TARGETS,languages=LANGUAGES,variants=VARIANTS,
             hirsel_primary=PRIMARY,hirsel_configurations=HIRSEL,
             rust_arena_cases=ARENA_CASES,c_arena_cases=C_ARENA_CASES,binarytrees_gate=BINARYTREES_GATE,arena_exclusions=arena_exclusions())
    m['source_sha256']={p:sha(ROOT/p) for p in SOURCE_FILES}
    (ROOT/'manifest.json').write_text(json.dumps(m,indent=2)+'\n')

def variants(m,case,language):
    return ['system','mimalloc']+(['arena'] if case['case'] in m.get(language+'_arena_cases', ()) else [])

def native_spec(hirsel,participant,case):
    spec = (hirsel or {}).get(participant, {})
    return spec.get('cases', {}).get(case['case']) if 'cases' in spec else spec or None

def participants(m,case,hirsel=False):
    refs=[f'{lang}/{v}' for lang in LANGUAGES for v in variants(m,case,lang)]
    if isinstance(hirsel,dict):
        return refs+[p for p in HIRSEL if native_spec(hirsel,p,case)]
    return refs+list(HIRSEL) if hirsel else refs

def gate_references(m,case,participant,hirsel):
    # Archived datasets keep their embedded contract. New binarytrees datasets
    # distinguish storage independently of the four partition/configuration slots.
    if case['case']!='binarytrees' or 'binarytrees_gate' not in m:
        return participants(m,case)
    storage=(native_spec(hirsel,participant,case) or {}).get('binarytrees_storage','heap')
    if storage not in ('heap','arena'):
        raise ValueError('invalid binarytrees_storage: '+str(storage))
    return m['binarytrees_gate'][storage]

def native_target():
    arch=platform.machine()
    if arch=='amd64':arch='x86_64'
    if arch=='arm64':arch='aarch64'
    if arch not in ('x86_64','aarch64') or platform.system()!='Linux':raise ValueError('required native Linux host')
    return arch+'-linux-gnu'

def controlled_environment(extra=None):
    env=os.environ.copy()
    for key in list(env):
        if key in ('LD_PRELOAD','MALLOC_CONF','GLIBC_TUNABLES','CARGO_ENCODED_RUSTFLAGS','CARGO_BUILD_RUSTFLAGS','RUSTFLAGS') or key.startswith(('HIRSEL_RT_', 'MIMALLOC_','CARGO_PROFILE_')):
            env.pop(key)
    env.update(extra or {})
    return env

def buck2_build(*args):
    """Build Buck2 targets from the repository root; each output path by its target's name."""
    out=subprocess.run([str(ROOT.parent.parent/'.buck2/bin/buck2'),'build',*args,'--show-full-output'],
                       cwd=ROOT.parent.parent,check=True,stdout=subprocess.PIPE,text=True).stdout
    return {target.split(' ')[0].rsplit(':',1)[1]:path for target,path in
            (line.rsplit(' ',1) for line in out.splitlines() if line.startswith('root//'))}

def native_environment(extra=None):
    """controlled_environment plus the pinned native tools and runtime archives, as the Buck2 test launcher sets them."""
    tools=buck2_build(*('//tools/buck2/native-toolchain:'+t for t in ('llvm','arm','qemu','valgrind')))
    llvm,arm,qemu,valgrind=(tools[t] for t in ('llvm','arm','qemu','valgrind'))
    archives=buck2_build('//:hirsel-rt-archives')['hirsel-rt-archives']
    env=controlled_environment(extra)
    env.update(LLVM_SYS_211_PREFIX=llvm,HIRSEL_RUNTIME_ARCHIVES=archives,
               HIRSEL_AARCH64_CC=arm+'/bin/aarch64-none-linux-gnu-gcc',HIRSEL_AARCH64_SYSROOT=arm+'/aarch64-none-linux-gnu/libc',
               HIRSEL_QEMU_AARCH64=qemu+'/qemu-aarch64',HIRSEL_VALGRIND=valgrind+'/bin/valgrind',
               VALGRIND_LIB=valgrind+'/libexec/valgrind',HIRSEL_VALGRIND_RUNTIME=valgrind+'/lib/x86_64-linux-gnu',
               PATH=llvm+'/bin:'+env.get('PATH','/usr/bin:/bin'),
               LD_LIBRARY_PATH=llvm+'/lib'+(':'+env['LD_LIBRARY_PATH'] if env.get('LD_LIBRARY_PATH') else ''))
    return env

def shell(argv,**kw):
    env=controlled_environment()
    return subprocess.run(argv,check=True,env=env,cwd=ROOT,**kw)

def toolchains():
    found={tool:shell(command,capture_output=True,text=True).stdout.strip() for tool,command in
           [('clang',['clang','--version']),('rust',['rustc','+1.99.0','--version'])]}
    assert 'clang version 18.1.3' in found['clang'].splitlines()[0],found['clang']
    assert found['rust'].startswith('rustc 1.99.0 '),found['rust']
    return found

def text_size(path):
    r=shell(['size','--format=sysv',str(path)],capture_output=True,text=True)
    return sum(int(line.split()[1]) for line in r.stdout.splitlines() if line.split() and line.split()[0]=='.text')

def build(m,only_languages=LANGUAGES):
    from allocators import prepare
    versions=toolchains();libs=prepare(m)
    (ROOT/'build').mkdir(exist_ok=True)
    (ROOT/'build/bin').mkdir(exist_ok=True)
    records={}
    for lang in only_languages:
        for variant in VARIANTS:
            for instrument in (False,True):
                suffix='-stats' if instrument else ''
                output=ROOT/'build/bin'/f'{lang}-{variant}{suffix}'
                link=libs.get(variant,[])
                env={}
                if lang=='c':
                    cmd=['clang','-std=c17','-O3','-ffp-contract=off','-fno-lto','-march=x86-64' if native_target().startswith('x86_64') else '-march=armv8-a','-Wall','-Wextra','-Werror']
                    if instrument:cmd+=['-DSTATS']
                    if variant=='arena':cmd+=['-DARENA']
                    cmd+=['c/main.c','-o',str(output)]+link
                else:
                    features=[]
                    if instrument:features.append('stats')
                    if variant=='mimalloc':features.append('mimalloc')
                    target_dir=ROOT/'build'/f'cargo-{variant}{suffix}'
                    cmd=['cargo','+1.99.0','build','--release','--locked','--manifest-path','rust/Cargo.toml','--target-dir',str(target_dir)]
                    if features:cmd+=['--features',','.join(features)]
                    # LLVM's default Rust FP operators have no contract/reassociate
                    # fast-math flags. LTO is off; target-cpu never uses native.
                    env['CC']='clang';env['CXX']='clang++';env['CFLAGS']='-O3 -ffp-contract=off -fno-lto '+('-march=x86-64' if native_target().startswith('x86_64') else '-march=armv8-a')
                    env['RUSTFLAGS']='-C target-cpu=x86-64 -C lto=off' if native_target().startswith('x86_64') else '-C target-cpu=generic -C lto=off'
                started=time.perf_counter()
                proc=subprocess.run(cmd,cwd=ROOT,env=controlled_environment(env),capture_output=True,text=True)
                elapsed=time.perf_counter()-started
                if proc.returncode:raise RuntimeError(f'build failed: {cmd}\n{proc.stderr}')
                if lang=='rust':
                    import shutil
                    staged=output.with_suffix('.new')
                    shutil.copy2(target_dir/'release/hirsel-native-reference',staged)
                    os.replace(staged,output)
                records[f'{lang}/{variant}{suffix}']={'path':str(output),'sha256':sha(output),'compile_seconds':elapsed,
                    'compile_kind':'invocation with retained tool caches','text_bytes':text_size(output),'command':cmd,'env':env}
    return {'toolchains':versions,'allocators':libs,'executables':records}

def command(participant,case,rounds,build_data,hirsel=None,stats=False,emit=False):
    if participant in HIRSEL:
        spec=native_spec(hirsel,participant,case)
        cmd=spec['counter_command' if stats else 'command']
        substitutions={'case':case['case'],'input':case['input'],'rounds':str(rounds),'emit':'--emit' if emit else ''}
        return [s.format(**substitutions) for s in cmd if s!='{emit}' or emit]
    lang,variant=participant.split('/')
    path=build_data['executables'][participant+('-stats' if stats else '')]['path']
    return [path,case['case'],str(ROOT/case['input']),str(rounds),variant]+(['--emit'] if emit else [])

def expected_checksum(case,repeats):
    return f"{(int(case['checksum'],16)*repeats)&((1<<64)-1):016x}\n".encode()

def snapshot(cpu):
    base=Path(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq')
    def read(p):
        try:return p.read_text().strip()
        except OSError:return None
    return {'utc_ns':time.time_ns(),'cpu':cpu,'affinity':sorted(os.sched_getaffinity(0)),
            'load':os.getloadavg(),'frequency_khz':read(base/'scaling_cur_freq'),
            'governor':read(base/'scaling_governor')}

def hardware(cpu):
    return {'target':native_target(),'uname':platform.uname()._asdict(),
            'cpuinfo':Path('/proc/cpuinfo').read_text(),'initial':snapshot(cpu),
            'os_release':Path('/etc/os-release').read_text()}


def parse_measurement(report):
    """Keep runtime-native units; arena unavailable is never a fabricated zero."""
    lines = report.splitlines()
    if not lines or lines[0] != 'hirsel-measurement 1':
        raise ValueError('unrecognized native measurement report')
    counts = {'classes': {}}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) == 4 and fields[0] == 'class':
            _, capacity, allocations, frees = fields
            if capacity in counts['classes']:
                raise ValueError('duplicate class')
            values = [int(allocations), int(frees)]
            if min(values) < 0:
                raise ValueError('negative class count')
            counts['classes'][capacity] = dict(allocations=values[0], frees=values[1])
        elif len(fields) == 2 and fields[0] not in counts:
            key, value = fields
            counts[key] = value if value == 'unavailable' else int(value)
        else:
            raise ValueError('malformed or duplicate measurement row')
    required = ('count_increments', 'count_decrements', 'cow_detaches', 'deep_copies',
                'copied_blocks', 'copied_bytes', 'reuse_hits', 'reuse_misses',
                'live_bytes', 'peak_live_bytes', 'region_high_water_bytes',
                'region_pool_high_water_bytes')
    if not counts['classes'] or any(type(counts.get(k)) is not int or counts[k] < 0 for k in required):
        raise ValueError('missing or invalid native counters')
    if counts.get('arena_high_water_bytes') != 'unavailable':
        raise ValueError('unexpected arena availability')
    return counts
