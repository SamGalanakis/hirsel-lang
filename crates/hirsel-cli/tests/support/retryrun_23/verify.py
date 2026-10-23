#!/usr/bin/env python3
"""Independent finite child scripts and boundary expectations across three engines."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import sys


HELP = (b'Usage: retryrun FILE\n       retryrun --help\n'
        b'Read one bounded JSON command from the read-only fixtures grant.\n'
        b'At most 5 attempts, 80 ms scheduled delay; each child has its own deadline.\n'
        b'Exit: 0 success/help, 1 command failed, 2 invalid input, 3 missing runtime grant.\n')


def canonical(value):
    return json.dumps(value, separators=(',', ':')).encode() + b'\n'


def report(statuses):
    return b''.join(canonical(dict(attempt=i + 1, status=s)) for i, s in enumerate(statuses)) + canonical(
        dict(attempts=len(statuses), terminal=statuses[-1]))


def child(request):
    request = json.loads(request.read_text())
    resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
    with open(request['stdout'], 'wb') as out, open(request['stderr'], 'wb') as err:
        done = subprocess.run(request['argv'], stdin=subprocess.DEVNULL, stdout=out, stderr=err,
                              timeout=60)
    Path(request['status']).write_text(str(done.returncode))


def verify(hirsel, root, output, shipped_only=False):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    state = output / 'child-state'
    selection = root / 'hirsel.json'
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    commands = []

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout, stderr, status = [output / (label + s) for s in ('.stdout', '.stderr', '.exit')]
        if runtime:
            request = output / (label + '.request.json')
            request.write_text(json.dumps(dict(argv=argv, stdout=str(stdout), stderr=str(stderr), status=str(status))))
            done = subprocess.run([sys.executable, root / 'capped', '--timeout', '60',
                                   sys.executable, __file__, '--child', request],
                                  stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=90)
            (output / (label + '.cap.log')).write_bytes(done.stdout + done.stderr)
            assert done.returncode == 0, (label, done, str(output))
            actual = int(status.read_text()), stdout.read_bytes(), stderr.read_bytes()
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=600)
            actual = done.returncode, done.stdout, done.stderr
            stdout.write_bytes(done.stdout)
            stderr.write_bytes(done.stderr)
            status.write_text(str(done.returncode))
        commands.append(dict(label=label, argv=argv, exit=actual[0]))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return actual

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        built = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert built == (0, b'', b''), (backend, built, str(output))
        binaries[backend] = exe

    base = dict(argv=[sys.executable, str(root / 'fixtures/child.py'), 'flaky', str(state)],
                retryable=[75], max_attempts=3, base_ns=500000, cap_ns=5000000,
                max_elapsed_ns=30000000000, timeout_ms=5000)
    policy = dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
                         monotonic_clock='clock.monotonic.ns@1', random='random.splitmix64@1',
                         tasks='tasks.cooperative@1', dirs=[dict(path='fixtures', host=str(data), access='read')],
                         process=dict(provider='process.spawn@1', rules=[dict(program=sys.executable)])),
                  args=['command.json'])
    cases = []

    def command(label, mode, statuses, **changes):
        record = copy.deepcopy(base)
        record['argv'][2] = mode
        record.update(changes)
        cases.append((label, record, policy, (0 if statuses[-1] == 'success' else 1, report(statuses), b''), len(statuses)))

    command('fails-twice-then-success', 'flaky', ['exit:75', 'exit:75', 'success'])
    command('same-output-again', 'flaky', ['exit:75', 'exit:75', 'success'])
    command('always-fails', 'fail', ['exit:75'] * 3)
    command('one-attempt', 'fail', ['exit:75'], max_attempts=1)
    command('five-attempts', 'fail', ['exit:75'] * 5, max_attempts=5, base_ns=0, cap_ns=0)
    command('permanent-exit', 'permanent', ['exit:2'])
    command('empty-retry-set', 'fail', ['exit:75'], retryable=[])
    command('slow-child-counts-in-budget', 'slow', ['exit:75'], max_elapsed_ns=1)
    command('child-deadline', 'hang', ['timeout'], timeout_ms=1000)
    command('both-pipes-drained', 'streams', ['success'])
    command('zero-cap', 'flaky', ['exit:75', 'exit:75', 'success'], cap_ns=0)
    command('cap-before-growth', 'flaky', ['exit:75', 'exit:75', 'success'], base_ns=5000000, cap_ns=1)
    missing = str(output / 'absent-child')
    p = copy.deepcopy(policy)
    p['io']['process']['rules'] = [dict(program=missing)]
    rec = base | dict(argv=[missing])
    cases.append(('spawn-not-found', rec, p, (1, report(['spawn_not_found']), b''), 0))
    p = copy.deepcopy(policy)
    p['io']['process']['rules'] = []
    cases.append(('exact-program-not-granted', base, p, (1, report(['process_not_granted']), b''), 0))

    def invalid(label, payload, kind):
        cases.append((label, payload, policy, (2, b'', canonical(dict(kind=kind))), 0))

    invalid('malformed', b'{', 'json_object')
    invalid('oversized', b' ' * 8193, 'input_bytes_limit')
    invalid('utf8', b'\xff', 'utf8')
    invalid('not-object', b'[]', 'json_object')
    invalid('missing-fields', {}, 'fields')
    invalid('extra-field', base | dict(extra=0), 'fields')
    invalid('duplicate-key', canonical(base)[:-2] + b',"cap_ns":1}', 'json_object')
    for key, value, kind in [
        ('argv', [], 'argv'), ('argv', ['relative'], 'program_absolute'),
        ('argv', ['/abs'] * 17, 'argv'), ('argv', ['/abs', 'x' * 257], 'argv'),
        ('argv', ['/abs', '\0'], 'argv'), ('argv', ['/abs', 0], 'argv'),
        ('retryable', [1] * 9, 'retryable'), ('retryable', [0], 'retryable'),
        ('retryable', [-1], 'retryable'), ('retryable', [256], 'retryable'),
        ('max_attempts', 0, 'positive_limits'), ('max_attempts', 6, 'max_attempts'),
        ('base_ns', 20000001, 'base_ns'), ('cap_ns', 20000001, 'cap_ns'),
        ('max_elapsed_ns', 0, 'positive_limits'), ('max_elapsed_ns', 30000000001, 'max_elapsed_ns'),
        ('timeout_ms', 0, 'positive_limits'), ('timeout_ms', 5001, 'timeout_ms'),
        ('max_attempts', 1.5, 'max_attempts'), ('max_attempts', '3', 'max_attempts')]:
        invalid('invalid-' + key + '-' + str(len(cases)), base | {key: value}, kind)
    for missing_grant in ('process', 'monotonic_clock', 'random', 'tasks', 'stdout', 'stderr'):
        p = copy.deepcopy(policy)
        del p['io'][missing_grant]
        cases.append(('missing-' + missing_grant, base, p, (3, b'', b''), 0))
    p = copy.deepcopy(policy)
    p['io']['dirs'] = []
    cases.append(('missing-directory', base, p, (2, b'', canonical(dict(kind='directory_grant'))), 0))
    for label, args, expected in [('help', ['--help'], (0, HELP, b'')),
                                  ('short-help', ['-h'], (0, HELP, b'')),
                                  ('usage', [], (2, b'', canonical(dict(kind='usage')))),
                                  ('traversal', ['../command.json'], (2, b'', canonical(dict(kind='filename')))),
                                  ('no-file', ['absent.json'], (2, b'', canonical(dict(kind='file_open'))))]:
        p = dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=policy['io']['dirs']), args=args)
        cases.append((label, base, p, expected, 0))

    if shipped_only:
        shipped_policy = json.loads((root / 'grants.json').read_text())
        good = copy.deepcopy(shipped_policy)
        bad = copy.deepcopy(shipped_policy)
        good['args'] = ['success.json']
        bad['args'] = ['invalid.json']
        cases = [
            ('shipped-success', json.loads((root / 'fixtures/success.json').read_text()),
             good, (0, report(['success']), b''), 0),
            ('shipped-rejection', json.loads((root / 'fixtures/invalid.json').read_text()),
             bad, (2, b'', canonical(dict(kind='max_attempts'))), 0)]

    original_fixtures = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in (root / 'fixtures').iterdir()}
    observations = 0
    for label, record, grant, expected, children in cases:
        payload = record if isinstance(record, bytes) else canonical(record)
        file = data / 'command.json'
        file.write_bytes(payload)
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
        grants = root / 'active-grants.json' if shipped_only else output / 'grants.json'
        grants.write_text(json.dumps(grant))
        for engine in ('h', 'llvm', 'cranelift'):
            argv = ([hirsel, 'run', selection, '--grants', grants] if engine == 'h' else
                    [binaries[engine], '--hirsel-grants', grants, *grant['args']])
            actual = run(label + '-' + engine, argv, runtime=True)
            assert actual == expected, (label, engine, actual, expected, str(output))
            pids = state.read_text().splitlines() if state.exists() else []
            assert len(pids) == children, (label, engine, pids, children)
            for pid in pids:
                try:
                    os.kill(int(pid), 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError(('unreaped child', label, engine, pid))
            state.unlink(missing_ok=True)
            assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
            observations += 1
    assert original_fixtures == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in (root / 'fixtures').iterdir()}
    for p in data.iterdir():
        p.unlink()
    data.rmdir()
    grants.unlink()
    assert not data.exists() and not state.exists()
    result = dict(cases=len(cases), observations=observations, engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path)
    parser.add_argument('--root', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--child', type=Path)
    parser.add_argument('--shipped-only', action='store_true')
    args = parser.parse_args()
    if args.child:
        child(args.child)
    else:
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(), args.shipped_only)
