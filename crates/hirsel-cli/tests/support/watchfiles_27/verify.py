"""Run the shipped watcher and its scripted provider through public CLI adapters."""
import json
import os
from pathlib import Path
import subprocess
import sys
from oracle import NAMES, expected

hirsel, root = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
real_only = '--real-only' in sys.argv
scripted_only = '--scripted-only' in sys.argv or '--recordings-only' in sys.argv
recordings_only = '--recordings-only' in sys.argv
recordings = []
env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1')


def run(argv, code=0, stdout=None):
    proc = subprocess.run(list(map(str, argv)), env=env, capture_output=True, timeout=60)
    assert proc.returncode == code, (argv, proc.returncode, proc.stdout, proc.stderr)
    if stdout is not None:
        assert proc.stdout == stdout, (argv, proc.stdout, stdout, proc.stderr)
    return proc


scripted = real = handshake = 0
for backend in ([] if real_only else ['llvm', 'cranelift']):
    exe = root / ('scripted-' + backend)
    run([hirsel, 'build', '--backend', backend, root / 'hirsel.json', '-o', exe])
    for which, name in enumerate(NAMES):
        for width in ([1, 2, 4] if recordings_only else [1]):
            grant = root / f'{backend}-{name}-w{width}.json'
            tape = root / f'{backend}-{name}-w{width}.tape'
            grant.write_text(json.dumps({'inputs': {'hash_seed': 0, 'random_seed': 0, 'test_seed': which}, 'io': {
                'tasks': 'tasks.cooperative@1', 'dirs': []},
                'placement': {'cores': width, 'commit': 'shared', 'reorder': 'auto'}}))
            prefix = [exe, '--hirsel-record', tape] if recordings_only else [exe]
            run(prefix + ['--hirsel-grants', grant], stdout=expected(which))
            if recordings_only:
                recordings.append(dict(backend=backend, case=which, width=width, tape=str(tape)))
            scripted += 1

if scripted_only:
    print(json.dumps(dict(scripted=scripted, real=real, handshake=handshake, recordings=recordings, cleanup=True)))
    sys.exit(0)

# A separate source selection shares the application modules without mutating them.
manifest = json.loads((root / 'hirsel.json').read_text())
manifest['sources'][0]['path'] = 'app-main.hir'
app = root / 'app.json'
app.write_text(json.dumps(manifest))
data = root / 'data'
data.mkdir()
(data / 'a').write_bytes(b'abc')
(data / 'large').write_bytes(b'x' * 65537)
for backend in ['llvm', 'cranelift']:
    exe = root / ('app-' + backend)
    run([hirsel, 'build', '--backend', backend, app, '-o', exe])
    for names, wanted in [(['a', 'absent'], b'0\t0\tinitial\t3\n0\t1\tmissing\t0\ncomplete\n'),
                          (['large'], b'0\t0\tlimit\t0\ncomplete\n'),
                          (['a'], b'deadline\n')]:
        grant = root / f'{backend}-real-{real}.json'
        timeout = '0' if wanted == b'deadline\n' else '1000000000'
        grant.write_text(json.dumps({'args': ['1', '1', timeout] + names, 'io': {
            'stdout': 'stdout.bytes@1', 'monotonic_clock': 'clock.monotonic.ns@1',
            'tasks': 'tasks.cooperative@1', 'dirs': [dict(path='data', host=str(data), access='read')]}}))
        run([exe, '--hirsel-grants', grant, '1', '1', timeout] + names, code=1 if timeout == '0' else 0, stdout=wanted)
        real += 1

# Real temporary-directory observations use explicit request/ack handshakes.
# No elapsed-time assertion, and each read starts only after the fixture's write.
manifest['sources'][0]['path'] = 'handshake.hir'
selection = root / 'handshake.json'
selection.write_text(json.dumps(manifest))
grant = root / 'handshake-grants.json'
grant.write_text(json.dumps({'io': {
    'stdout': 'stdout.bytes@1', 'stdin': 'stdin.bytes@1',
    'monotonic_clock': 'clock.monotonic.ns@1', 'tasks': 'tasks.cooperative@1',
    'dirs': [dict(path='data', host=str(data), access='read')]}}))
for backend in ['H', 'llvm', 'cranelift']:
    if backend == 'H':
        argv = [hirsel, 'run', selection, '--grants', grant]
    else:
        exe = root / ('handshake-' + backend)
        run([hirsel, 'build', '--backend', backend, selection, '-o', exe])
        argv = [exe, '--hirsel-grants', grant]
    p = subprocess.Popen(list(map(str, argv)), env=env, stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        for tick, contents, status in [(0, b'abc', 'initial'), (1, b'xyz', 'changed')]:
            assert p.stdout.readline() == f'read:{tick}\n'.encode()
            (data / 'artifact').write_bytes(contents)
            p.stdin.write(b'x')
            p.stdin.flush()
            assert p.stdout.readline() == f'{tick}:0:{status}:3\n'.encode()
        stdout, stderr = p.communicate(timeout=10)
        assert p.returncode == 0 and stdout == b'', (backend, p.returncode, stdout, stderr)
    finally:
        if p.poll() is None:
            p.kill()
            p.wait()
    handshake += 1
print(json.dumps(dict(scripted=scripted, real=real, handshake=handshake, recordings=recordings, cleanup=True)))
