#!/usr/bin/env python3
"""Ten locked, seeded interleaved rounds; per-sample evidence survives failures.
Run after the ignored artifact builder:
  python3 reports/hex-builder-5/measure.py /tmp/hex-builder-5-measure
Each run creates a unique attempt directory; prior attempts are never replaced.
"""
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import random
import resource
import statistics
import subprocess
import sys

HERE = Path(__file__).resolve().parent
BASE = Path(sys.argv[1]).resolve()
STAMP = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
DEST = BASE / f'attempt-{STAMP}-{os.getpid()}'
LOCK = Path('/workspace/code/hirsel-briefs/locks/cpu31-timing.lock')
SEED = 0x48455835
EVENTS = ['instructions', 'instructions:u', 'cycles', 'cycles:u']


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(name, value):
    (DEST / name).write_text(json.dumps(value, indent=2) + '\n')


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, timeout=120, **kwargs)
    if result.returncode:
        raise RuntimeError(f'{command}: exit {result.returncode}: {result.stderr.decode(errors="replace")}')
    return result


def capture(name, command, **kwargs):
    result = subprocess.run(command, capture_output=True, timeout=120, **kwargs)
    (DEST / f'{name}.stdout').write_bytes(result.stdout)
    (DEST / f'{name}.stderr').write_bytes(result.stderr)
    save(f'{name}.status.json', {'exit': result.returncode, 'sha256': sha(result.stdout), 'command': command})
    return result


def pinned(command):
    return ['nice', '-n', '10', 'taskset', '-c', '31', *command]


def main():
    # Tighten an inherited cap; never raise it. The linker also uses one worker.
    soft, hard = resource.getrlimit(resource.RLIMIT_DATA)
    cap = 512 * 1024**2
    resource.setrlimit(resource.RLIMIT_DATA, (min(soft if soft != resource.RLIM_INFINITY else cap, cap), hard))
    DEST.mkdir(parents=True)
    print(f'ATTEMPT {DEST}', flush=True)
    source = HERE / 'comparator.rs'
    for diagnostic in [False, True]:
        binary = DEST / ('rust-diagnostic' if diagnostic else 'rust')
        run(['rustc', '--edition=2024', '-O', '-Adead_code', '-C', 'link-arg=-Wl,--threads=1',
             *(['--cfg', 'diagnostic'] if diagnostic else []), str(source), '-o', str(binary)])
    grants = DEST / 'grants.json'
    grants.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1', 'dirs': []}}))
    result = {'seed': SEED, 'cpu': 31, 'rounds': [], 'output_hashes': {}, 'executables': {},
              'sources': {}, 'rustc': run(['rustc', '-Vv']).stdout.decode(), 'diagnostics': [], 'bounded_stops': []}
    for path in [source, Path(__file__), *BASE.glob('*.hir')]:
        result['sources'][str(path)] = sha(path.read_bytes())
    cases = []
    for size in [16, 32, 64, 65536]:
        count = 1 if size == 65536 else 2048
        expected = b'hex: ' + (bytes((i * 37 + 11) % 256 for i in range(size)).hex() * count).encode()
        result['output_hashes'][str(size)] = {'bytes': len(expected), 'sha256': sha(expected)}
        for variant in ['encode', 'into', 'rust']:
            binary = DEST / 'rust' if variant == 'rust' else BASE / f'{size}-{variant}-default/program'
            arguments = [str(size)] if variant == 'rust' else ['--hirsel-grants', str(grants)]
            result['executables'][f'{size}-{variant}'] = sha(binary.read_bytes())
            cases.append((size, variant, binary, arguments, expected))
    save('identity.json', result)
    print(f'WAITING {LOCK}', flush=True)
    with LOCK.open('a+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        print('LOCKED CPU31', flush=True)
        for size, variant, binary, arguments, expected in cases:
            actual = capture(f'warm-{size}-{variant}', pinned([str(binary), *arguments]))
            assert actual.returncode == 0 and actual.stdout == expected
        rng = random.Random(SEED)
        for round_index in range(10):
            order = cases.copy()
            rng.shuffle(order)
            for size, variant, binary, arguments, expected in order:
                name = f'round-{round_index:02}-{size}-{variant}'
                counter_file = DEST / f'{name}.perf.csv'
                actual = capture(name, pinned(['perf', 'stat', '-x,', '-o', str(counter_file), '-e', ','.join(EVENTS), '--', str(binary), *arguments]))
                assert actual.returncode == 0 and actual.stdout == expected
                counters = {}
                for line in counter_file.read_text().splitlines():
                    fields = line.split(',')
                    if len(fields) > 2 and fields[2] in EVENTS:
                        counters[fields[2]] = int(fields[0])
                assert counters.keys() == set(EVENTS), counter_file.read_text()
                row = {'round': round_index, 'size': size, 'variant': variant, **counters}
                save(f'{name}.sample.json', row)
                result['rounds'].append(row)
                save('results.json', result)
            print(f'round {round_index + 1}/10', flush=True)
        result['medians'] = [
            {'size': size, 'variant': variant, **{event: statistics.median(
                row[event] for row in result['rounds'] if row['size'] == size and row['variant'] == variant
            ) for event in EVENTS}}
            for size in [16, 32, 64, 65536] for variant in ['encode', 'into', 'rust']
        ]
        save('results.json', result)
        # Allocation diagnostics use separate executables, outside all perf runs.
        for size, variant, binary, arguments, expected in cases:
            measured = DEST / 'rust-diagnostic' if variant == 'rust' else BASE / f'{size}-{variant}-diagnostic/program'
            log = DEST / f'{size}-{variant}-allocations.txt'
            actual = capture(f'diagnostic-{size}-{variant}', pinned([str(measured), *arguments]),
                             env=os.environ | {'HIRSEL_RT_MEASUREMENT': str(log)})
            assert actual.returncode == 0 and actual.stdout == expected
            if log.exists():
                assert 'live_bytes 0\n' in log.read_text(), log.read_text()
            sites = Path(str(log) + '.sites.json')
            result['diagnostics'].append({'size': size, 'variant': variant, 'stderr': actual.stderr.decode(),
                'measurement': log.read_text() if log.exists() else None,
                'sites': json.loads(sites.read_text()) if sites.exists() else None})
            save('results.json', result)
        for size in [16, 32, 64, 65536]:
            cap = 131072 if size == 65536 else 4096
            limited = DEST / f'{size}-limited.json'
            limited.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1', 'dirs': []},
                'budgets': {'provider': 'budgets.limits@2', 'memory_bytes': cap}}))
            log = DEST / f'{size}-budget-allocations.txt'
            observation = DEST / f'{size}-budget.observation'
            binary = BASE / f'{size}-into-diagnostic/program'
            actual = capture(f'budget-{size}', pinned([str(binary), '--hirsel-grants', str(limited)]),
                env=os.environ | {'HIRSEL_RT_MEASUREMENT': str(log), 'HIRSEL_RT_OBSERVATION': str(observation)})
            # DET Budgets/programs.md:127: a tooling stop runs no destruction.
            # Completion checks above and H's injected failures retain cleanup checks.
            assert actual.returncode == 5 and not actual.stdout
            assert 'budget_exceeded memory\n' in observation.read_text(), observation.read_text()
            assert 'audit unavailable\n' in observation.read_text(), observation.read_text()
            result['bounded_stops'].append({'size': size, 'cap': cap, 'exit': actual.returncode,
                'measurement': log.read_text(), 'observation': observation.read_text()})
            save('results.json', result)
        fcntl.flock(lock, fcntl.LOCK_UN)
    print(json.dumps(result['medians'], indent=2))


if __name__ == '__main__':
    main()
