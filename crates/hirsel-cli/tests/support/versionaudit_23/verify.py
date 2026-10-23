#!/usr/bin/env python3
"""Pinned semver vectors and bounded application workflows across three engines.

Vectors: dtolnay/semver 280ebcb6edac3aa4cdc545dbff8a26c5ac4861fe,
tests/test_version_req.rs:39-75. Copyright David Tolnay and contributors (MIT).
The checked-in MIT license accompanies the example. Added application cases
use literal expectations and Cargo's leftmost-nonzero/prerelease policy.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def report(rows, matches):
    failures = sorted([r for r, yes in zip(rows, matches) if not yes],
                      key=lambda r: r['name'].encode())
    return dict(mismatches=failures, summary=dict(
        **{'class': 'mismatch' if failures else 'ok'}, checked=len(rows), mismatched=len(failures)))


def error(kind, row=0, detail='', offset=0):
    return canonical(dict(**{'class': 'invalid_input'}, error=kind, row=row, detail=detail, offset=offset))


HELP = (b'Usage: versionaudit FILE.json\n'
        b'Audit up to 64 {name,requirement,version} rows using Cargo requirements.\n'
        b'Exit: 0 match/help, 1 mismatch, 2 invalid input or I/O.\n')


def vectors():
    # These lists are upstream answers, never calls to Hirsel's evaluator.
    groups = [
        ('^1.0.0', ['1.0.0', '1.1.0', '1.0.1'],
         ['0.9.9', '0.10.0', '0.1.0', '1.0.0-pre', '1.0.1-pre']),
        ('=1.0.0', ['1.0.0'], ['1.0.1', '0.9.9', '0.10.0', '0.1.0', '1.0.0-pre']),
        ('=0.9.0', ['0.9.0'], ['0.9.1', '1.9.0', '0.0.9', '0.9.0-pre']),
        ('=0.0.2', ['0.0.2'], ['0.0.1', '0.0.3', '0.0.2-pre']),
        ('=0.1.0-beta2.a', ['0.1.0-beta2.a'],
         ['0.9.1', '0.1.0', '0.1.1-beta2.a', '0.1.0-beta2']),
        ('=0.1.0+meta', ['0.1.0', '0.1.0+meta', '0.1.0+any'], []),
        # Added boundary examples: no patch/minor bump past the first nonzero.
        ('^0.2.3', ['0.2.3', '0.2.99'], ['0.2.2', '0.3.0', '0.2.4-pre']),
        ('^0.0.3', ['0.0.3', '0.0.3+vendor'], ['0.0.2', '0.0.4']),
        ('>=1.2.3-alpha', ['1.2.3-beta', '1.2.3', '2.0.0'], ['1.2.4-beta']),
    ]
    rows, answers = [], []
    for req, yes, no in groups:
        for good, versions in [(True, yes), (False, no)]:
            for version in versions:
                rows.append(dict(name=f'dep-{len(rows):02}', requirement=req, version=version))
                answers.append(good)
    return rows, answers


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    selection = root / 'hirsel.json'
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    commands = []

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout = output / (label + '.stdout')
        stderr = output / (label + '.stderr')
        status = output / (label + '.exit')
        if runtime:
            # Keep tools/capped's resource report separate from the application's
            # stderr, so all three program streams/status are compared exactly.
            request = output / (label + '.request.json')
            request.write_text(json.dumps(dict(argv=argv, stdout=str(stdout),
                                               stderr=str(stderr), status=str(status))))
            done = subprocess.run([sys.executable, root / 'capped', '--timeout', '60',
                                   sys.executable, __file__, '--child', request],
                                  stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=90)
            (output / (label + '.cap.log')).write_bytes(done.stdout + done.stderr)
            assert done.returncode == 0, (label, done, str(output))
            actual = (int(status.read_text()), stdout.read_bytes(), stderr.read_bytes())
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                  env=env, timeout=600)
            stdout.write_bytes(done.stdout)
            stderr.write_bytes(done.stderr)
            status.write_text(str(done.returncode))
            actual = (done.returncode, done.stdout, done.stderr)
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
        assert run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe]) == (0, b'', b'')
        binaries[backend] = exe

    cases = []

    def fixture(label, blob, expected, code=0):
        (data / (label + '.json')).write_bytes(blob)
        cases.append((label, [label + '.json'], (code, expected, b''), None))

    rows, answers = vectors()
    fixture('vectors', canonical(rows), canonical(report(rows, answers)), 1)
    for label in ('success', 'invalid'):
        blob = (root / 'fixtures' / (label + '.json')).read_bytes()
        expected = canonical(report(json.loads(blob), [True] * 3)) if label == 'success' else error('version', 1, 'Syntax', 0)
        fixture(label, blob, expected, 0 if label == 'success' else 2)
    fixture('empty', b'[]', canonical(report([], [])))
    fixture('empty-file', b'', error('json', detail='Syntax'), 2)
    fixture('malformed', b'[', error('json', detail='Syntax', offset=1), 2)
    fixture('oversized', b' ' * 65537, error('input_bytes'), 2)
    fixture('bytes-exact', b'[]' + b' ' * 65534, canonical(report([], [])))
    one = dict(name='a', requirement='^1.0.0', version='1.0.0')
    fixture('duplicate', canonical([one, one]), error('duplicate_name', 2), 2)
    for label, changes, expected in [
        ('bad-requirement', {'requirement': '||'}, error('requirement', 1, 'Syntax', 0)),
        ('empty-requirement', {'requirement': ''}, error('requirement', 1, 'Syntax', 0)),
        ('bad-version', {'version': '01.0.0'}, error('version', 1, 'LeadingZero', 0)),
        ('empty-version', {'version': ''}, error('version', 1, 'Empty', 0)),
        ('overflow-version', {'version': '18446744073709551616.0.0'}, error('version', 1, 'Overflow', 0)),
        ('empty-name', {'name': ''}, error('empty_name', 1)),
        ('name-over', {'name': 'x' * 129}, error('field_bytes', 1)),
        ('requirement-over', {'requirement': 'x' * 257}, error('field_bytes', 1)),
        ('version-over', {'version': 'x' * 257}, error('field_bytes', 1)),
        ('unknown-field', {'extra': ''}, error('fields', 1)),
    ]:
        fixture(label, canonical([one | changes]), expected, 2)
    boundary = [one | {'name': str(n)} for n in range(64)]
    fixture('rows-exact', canonical(boundary), canonical(report(boundary, [True] * 64)))
    fixture('rows-over', canonical(boundary + [one]), error('row_limit', 65), 2)
    fixture('missing-field', b'[{"name":"a","version":"1.0.0"}]', error('fields', 1), 2)
    fixture('duplicate-key', b'[{"name":"a","name":"b"}]', error('json', detail='DuplicateKey', offset=13), 2)
    fixture('wrong-type', b'[{"name":4}]', error('json', detail='Syntax', offset=9), 2)
    fixture('trailing', b'[]x', error('json', detail='Syntax', offset=2), 2)
    fixture('utf8', b'["\xff"]', error('json', detail='Syntax', offset=1), 2)
    # Explicit field boundaries; metadata is ignored in matching, preserved in output.
    field = [dict(name='x' * 128, requirement='=1.0.0+' + 'b' * 249,
                  version='1.0.0+' + 'b' * 250)]
    fixture('fields-exact', canonical(field), canonical(report(field, [True])))
    mismatch = [one | dict(name='z', version='2.0.0'), one | dict(name='a "羊"', version='1.0.1-pre')]
    expected = canonical(report(mismatch, [False, False]))
    fixture('order', canonical(mismatch), expected, 1)
    fixture('order-reversed', canonical(mismatch[::-1]), expected, 1)
    cases += [('help', ['--help'], (0, HELP, b''), 'dir'),
              ('usage', [], (2, error('usage'), b''), None),
              ('missing-file', ['absent.json'], (2, error('file_open'), b''), None),
              ('missing-grant', ['success.json'], (2, error('directory_grant'), b''), 'dir'),
              ('missing-stdout', ['success.json'], (2, b'', b''), 'stdout'),
              ('path', ['../success.json'], (2, error('path'), b''), None)]
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    grants = output / 'grants.json'
    observations = 0
    for label, args, expected, missing in cases:
        policy = dict(io=dict(stdout='stdout.bytes@1', dirs=[
            dict(path='input', host=str(data), access='read')]), args=args)
        if missing == 'dir': policy['io']['dirs'] = []
        if missing == 'stdout': del policy['io']['stdout']
        grants.write_text(json.dumps(policy))
        for engine in ('h', 'llvm', 'cranelift'):
            command = ([hirsel, 'run', selection, '--grants', grants] if engine == 'h' else
                       [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, command, runtime=True)
            assert actual == expected, (label, engine, actual, expected, str(output))
            observations += 1
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}, 'host writes'
    for p in data.iterdir(): p.unlink()
    data.rmdir()
    grants.unlink()
    assert not data.exists() and not grants.exists(), 'cleanup'
    result = dict(cases=len(cases), observations=observations, vectors=len(rows),
                  engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


def child(request):
    value = json.loads(request.read_text())
    with open(value['stdout'], 'wb') as stdout, open(value['stderr'], 'wb') as stderr:
        done = subprocess.run(value['argv'], stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, timeout=60)
    Path(value['status']).write_text(str(done.returncode))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path)
    parser.add_argument('--root', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--child', type=Path)
    args = parser.parse_args()
    if args.child:
        child(args.child)
    else:
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
