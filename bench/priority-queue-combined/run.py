#!/usr/bin/env python3
"""Build matched standalone controls and take ten seeded CPU31 interleaved rounds.

python3 bench/priority-queue-combined/run.py /absolute/evidence-directory
Uses the existing capped runner and perf sampler; never edits LangArena results.
"""
import hashlib
import fcntl
from contextlib import contextmanager
import json
import os
import re
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'bench/native'))
from suite import buck2_build, native_environment
sys.path.insert(0, str(ROOT / 'bench/micro'))
from measure import perf_command, perf_counts, siblings, cpu_snapshot, busy_share, schedule, TIMING_LOCK


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(command, **kwargs):
    return subprocess.run([str(ROOT / 'tools/capped'), '--timeout', '900', *map(str, command)],
                          cwd=ROOT, check=True, **kwargs)


def oracle(mode, k, n, seed):
    state, bag, total = seed, [], 0
    mask = (1 << 64) - 1
    for index in range(k + n):
        state = (state * 6364136223846793005 + 1) & mask
        value = (state >> 32) % 1024
        if index < k:
            bag.append(value)
            bag.sort()
            continue
        if mode == 0:
            bag.append(value)
            bag.sort()
            returned = bag.pop(0)
        else:
            returned = bag.pop(0) if bag else 0
            bag.append(value)
            bag.sort()
        total = (total * 31 + returned) & mask
    for value in bag:
        total = (total * 31 + value) & mask
    return f'{total}\n'.encode()


def comparator_calls(path):
    lines = path.read_text().splitlines()
    symbols = {}
    for line in lines:
        match = re.match(r'(?:c?fn)=\((\d+)\) (.+)', line)
        if match:
            symbols[match[1]] = match[2]
    selected = {key for key, name in symbols.items() if 'pqcombined__bench' in name and 'compareI' in name}
    totals, target = {}, None
    for line in lines:
        if line.startswith('cfn='):
            match = re.match(r'cfn=\((\d+)\)', line)
            target = match[1] if match else None
        elif line.startswith('calls='):
            if target in selected:
                totals[target] = totals.get(target, 0) + int(line.split()[0].split('=')[1])
            target = None
    # At O0 the thunk calls the ordinary source entry. Count that entry once;
    # if emission folds it into the thunk, count thunk entries instead.
    direct = {key for key in selected if not symbols[key].startswith('_H1T')}
    direct_calls = sum(totals.get(key, 0) for key in direct)
    return direct_calls if direct_calls else sum(totals.values())


def profile_calls(path, wanted):
    symbols, totals, target = {}, {}, None
    for line in path.read_text().splitlines():
        match = re.match(r'(?:c?fn)=\((\d+)\) (.+)', line)
        if match:
            symbols[match[1]] = match[2]
    for line in path.read_text().splitlines():
        if line.startswith('cfn='):
            match = re.match(r'cfn=\((\d+)\)', line)
            target = match[1] if match else None
        elif line.startswith('calls='):
            name = symbols.get(target)
            if name in wanted:
                totals[name] = totals.get(name, 0) + int(line.split()[0].split('=')[1])
            target = None
    return {name: totals.get(name, 0) for name in wanted}


@contextmanager
def timing_lock(cpu):
    # Host rules 2026-10-22: never wait for a long campaign. Instructions and
    # separate allocation profiles are sufficient; wall-clock timing follows up.
    with TIMING_LOCK.open('a+') as lock:
        exclusive = False
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            exclusive = True
        except BlockingIOError:
            pass
        yield exclusive


def sample(participant, cpu, timeout, directory):
    # Adapt bench/micro/measure.py:79-113; retain all raw evidence per cell.
    directory.mkdir(parents=True, exist_ok=False)
    counters = directory / 'perf.csv'
    before_path, after_path = directory / 'before.stat', directory / 'after.stat'
    env = os.environ | participant.get('env', {}) | {'LC_ALL': 'C'}
    command = [str(ROOT / 'tools/capped'), '--timeout', str(timeout),
        *perf_command(participant['command'], cpu, counters, before_path, after_path)]
    with (directory / 'stdout').open('wb') as stdout, (directory / 'stderr').open('wb') as stderr:
        result = subprocess.run(command, cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr)
    status = {'command': command, 'exit': result.returncode, 'accepted': False}
    status_file = directory / 'status.json'
    status_file.write_text(json.dumps(status, indent=2) + '\n')
    if result.returncode:
        raise ValueError(f'participant failed; raw evidence: {directory}')
    counts, running = perf_counts(counters.read_text())
    if any(percent != 100 for percent in running.values()):
        raise ValueError(f'partial counters {running}; raw evidence: {directory}')
    before, after = cpu_snapshot(siblings(cpu), before_path), cpu_snapshot(siblings(cpu), after_path)
    return {'counts': counts, 'running_percent': running, 'stdout_sha256': sha(directory / 'stdout'),
        'sibling_before': before, 'sibling_after': after, 'sibling_busy_share': busy_share(before, after),
        'raw': str(directory)}


def main():
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = native_environment()
    bundle = Path(buck2_build('--target-platforms', '//tools/buck2:release', '//:hirsel-toolchain')['hirsel-toolchain'])
    # Freeze the compiler/std image for every participant in this session.
    import shutil
    frozen = output / 'toolchain'
    if frozen.exists():
        originals = {p.relative_to(bundle): sha(p) for p in bundle.rglob('*') if p.is_file()}
        retained = {p.relative_to(frozen): sha(p) for p in frozen.rglob('*') if p.is_file()}
        if originals != retained:
            raise ValueError('output directory belongs to a different compiler/std bundle')
    else:
        shutil.copytree(bundle, frozen)
    cli = frozen / 'hirsel'
    env['HIRSEL_PRECHECK'] = str(frozen / 'hirsel-std.precheck')
    (output / 'build-environment.json').write_text(json.dumps({key: value for key, value in env.items()
        if key.startswith(('HIRSEL_', 'LLVM_')) or key in ('PATH', 'LD_LIBRARY_PATH')}, indent=2) + '\n')
    binary = output / 'hirsel-topk'
    run([cli, 'build', '--release', HERE / 'topk.hir', '-o', binary], env=env)
    # O0 diagnostic of the exact same source: Callgrind counts comparator
    # entries, including the guarded baseline call that O2 can inline.
    run([cli, 'build', '--debug', HERE / 'topk.hir', '-o', output / 'hirsel-count'], env=env)
    grants = output / 'grants.json'
    grants.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1', 'dirs': []}}) + '\n')
    rustc = ROOT / 'tools/buck2/toolchains/rust-files/bin/rustc'
    for name, flags in [('rust-topk', []), ('rust-count', ['--cfg', 'pqcombined_count'])]:
        run([rustc, '--edition=2024', '-O', *flags, HERE / 'control.rs', '-o', output / name])
    identities = {'source_base': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'rustc': subprocess.check_output([rustc, '-Vv'], text=True),
        'files': {str(p.relative_to(ROOT)): sha(p) for p in [HERE / 'topk.hir', HERE / 'control.rs',
            ROOT / 'stdlib/std@1/priority_queue.hir', ROOT / 'stdlib/std@1/heap_ops.hir']},
        'toolchain': {str(p.relative_to(output)): sha(p) for p in frozen.rglob('*') if p.is_file()},
        'executables': {name: sha(output / name) for name in ['hirsel-topk', 'hirsel-count', 'rust-topk', 'rust-count']},
        'runtime_archives': {str(p.relative_to(Path(env['HIRSEL_RUNTIME_ARCHIVES']))): sha(p)
            for p in Path(env['HIRSEL_RUNTIME_ARCHIVES']).rglob('*.a')}}
    (output / 'identities.json').write_text(json.dumps(identities, indent=2) + '\n')
    sessions = []
    for operation in (0, 2):
        for k in (1, 16, 64):
            data = {'operation': operation, 'k': k, 'n': 8192, 'seed': 20261022, 'samples': [], 'comparators': [], 'profiles': []}
            for round_number in range(10):
                seed = round_number + 1
                expected = oracle(operation, k, 8192, seed)
                expected_hash = hashlib.sha256(expected).hexdigest()
                commands = {'combined': [str(binary), '--hirsel-grants', str(grants), str(operation)],
                    'before': [str(binary), '--hirsel-grants', str(grants), str(operation + 1)],
                    'rust': [str(output / 'rust-topk'), str(operation)]}
                for name, command in commands.items():
                    result = run(command + list(map(str, [k, 8192, seed])), env=env, capture_output=True)
                    cell = output / 'preflight-raw' / f'{operation}-{k}-{round_number}-{name}'
                    cell.mkdir(parents=True, exist_ok=True)
                    (cell / 'stdout').write_bytes(result.stdout)
                    (cell / 'stderr').write_bytes(result.stderr)
                    (cell / 'status.json').write_text(json.dumps({'exit': result.returncode, 'command': command + list(map(str, [k, 8192, seed]))}) + '\n')
                    assert result.stdout == expected, (name, k, seed, result.stdout, expected)
                for name, mode in [('combined', operation), ('before', operation + 1), ('rust', operation)]:
                    executable = output / ('rust-count' if name == 'rust' else 'hirsel-count')
                    if name == 'rust':
                        result = run([executable, mode, k, 8192, seed], env=env, capture_output=True)
                        assert result.stdout == expected
                        calls = int(result.stderr.split(b'calls=')[1].splitlines()[0])
                    else:
                        trace = output / f'callgrind-{operation}-{k}-{round_number}-{name}.out'
                        profile_env = env | {'VALGRIND_LIB': str(Path(env['HIRSEL_VALGRIND']).parents[1] / 'libexec/valgrind'),
                            'LD_LIBRARY_PATH': env['HIRSEL_VALGRIND_RUNTIME'] + ':' + env.get('LD_LIBRARY_PATH', '')}
                        result = run([env['HIRSEL_VALGRIND'], '--tool=callgrind', '--callgrind-out-file=' + str(trace),
                            executable, '--hirsel-grants', grants, mode, k, 8192, seed], env=profile_env, capture_output=True)
                        assert result.stdout == expected
                        calls = comparator_calls(trace)
                        if operation == 0 and k == 1 and name == 'combined':
                            assert calls == 8192, ('singleton comparator-count control', calls)
                        if operation == 2 and k == 1:
                            assert calls == 0, ('singleton replacement-count control', calls)
                    diagnostic_cell = output / 'diagnostic-raw' / f'{operation}-{k}-{round_number}-{name}'
                    diagnostic_cell.mkdir(parents=True, exist_ok=True)
                    (diagnostic_cell / 'stdout').write_bytes(result.stdout)
                    (diagnostic_cell / 'stderr').write_bytes(result.stderr)
                    (diagnostic_cell / 'status.json').write_text(json.dumps({'exit': result.returncode, 'calls': calls}) + '\n')
                    data['comparators'].append({'round': round_number, 'name': name, 'calls': calls})
                # Production allocation/instruction profiles are separate from perf.
                for name, command in commands.items():
                    trace = output / f'production-callgrind-{operation}-{k}-{round_number}-{name}.out'
                    profile_env = env | {'VALGRIND_LIB': str(Path(env['HIRSEL_VALGRIND']).parents[1] / 'libexec/valgrind'),
                        'LD_LIBRARY_PATH': env['HIRSEL_VALGRIND_RUNTIME'] + ':' + env.get('LD_LIBRARY_PATH', '')}
                    result = run([env['HIRSEL_VALGRIND'], '--tool=callgrind', '--callgrind-out-file=' + str(trace),
                        *command, k, 8192, seed], env=profile_env, capture_output=True)
                    diagnostic_cell = output / 'production-profile-raw' / f'{operation}-{k}-{round_number}-{name}'
                    diagnostic_cell.mkdir(parents=True, exist_ok=False)
                    (diagnostic_cell / 'stdout').write_bytes(result.stdout)
                    (diagnostic_cell / 'stderr').write_bytes(result.stderr)
                    (diagnostic_cell / 'status.json').write_text(json.dumps({'exit': result.returncode}) + '\n')
                    assert result.stdout == expected
                    ir = int(next(line.split(':', 1)[1] for line in trace.read_text().splitlines() if line.startswith('summary:')))
                    data['profiles'].append({'round': round_number, 'name': name, 'instructions': ir,
                        'allocation_calls': profile_calls(trace, ['hirsel_rt_v2_alloc', 'malloc', 'calloc', 'realloc']),
                        'raw': str(trace)})
            (output / f'preflight-{operation}-{k}.json').write_text(json.dumps(data, indent=2) + '\n')
            for round_number in range(10):
                seed = round_number + 1
                expected = oracle(operation, k, 8192, seed)
                expected_hash = hashlib.sha256(expected).hexdigest()
                commands = {'combined': [str(binary), '--hirsel-grants', str(grants), str(operation)],
                    'before': [str(binary), '--hirsel-grants', str(grants), str(operation + 1)],
                    'rust': [str(output / 'rust-topk'), str(operation)]}
                # Release the shared CPU31 lock between complete rounds to yield.
                with timing_lock(31) as exclusive:
                    for _, name in schedule(list(commands), 1, 20261022 + round_number):
                        command = commands[name] + list(map(str, [k, 8192, seed]))
                        result = sample({'name': name, 'command': command, 'env': env}, 31, 300, output / 'raw' / f'{operation}-{k}-{round_number}-{name}')
                        assert result['stdout_sha256'] == expected_hash, (name, k, seed, result)
                        status_file = Path(result['raw']) / 'status.json'
                        status = json.loads(status_file.read_text())
                        status.update(accepted=True, expected_stdout_sha256=expected_hash)
                        status_file.write_text(json.dumps(status, indent=2) + '\n')
                        data['samples'].append({'round': round_number, 'name': name, 'command': command, 'timing_exclusive': exclusive, **result})
                        (output / f'session-{operation}-{k}.json').write_text(json.dumps(data, indent=2) + '\n')
                (output / f'session-{operation}-{k}.json').write_text(json.dumps(data, indent=2) + '\n')
                print(f'operation={operation} k={k} round={round_number + 1}/10', flush=True)
            data['medians'] = {name: {metric: statistics.median(s['counts'][metric] for s in data['samples'] if s['name'] == name)
                for metric in ('instructions', 'instructions:u', 'cycles', 'cycles:u', 'task-clock')}
                for name in commands}
            data['comparator_medians'] = {name: statistics.median(s['calls'] for s in data['comparators'] if s['name'] == name)
                for name in commands}
            (output / f'session-{operation}-{k}.json').write_text(json.dumps(data, indent=2) + '\n')
            sessions.append(data)
    (output / 'measurements.json').write_text(json.dumps(sessions, indent=2) + '\n')


if __name__ == '__main__':
    main()
