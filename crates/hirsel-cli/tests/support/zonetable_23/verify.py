#!/usr/bin/env python3
"""Jiff New York vectors and explicit-zone workflows across H, LLVM, Cranelift.

Vectors: jiff fff640d47f72ca6d3a122678315bbd76488306fe,
crates/jiff/src/tz/ambiguous.rs:509-560 and timezone.rs:2656-2671.
Copyright (c) 2015 Andrew Gallant (MIT; examples/zonetable/LICENSE-jiff).
The oracle uses upstream offsets and Python calendar arithmetic, never Hirsel.
"""
import argparse
from datetime import datetime, timedelta
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def error(kind, row=0, detail='', offset=0):
    return canonical(dict(**{'class': 'invalid_input'}, error=kind, row=row, detail=detail, offset=offset))


HELP = (b'Usage: zonetable FILE.json [--format PATTERN]\n'
        b'Convert up to 64 local meetings using explicit TZif files.\n'
        b'Policies: Reject (default), Earlier, Later. Exit: 0 success/help, 2 invalid input or I/O.\n')

# Pinned upstream New York offsets before/after each civil boundary. Added
# center-gap/fold and summer examples are the ambiguous.rs doctest vectors.
VECTORS = [
    ('1969-12-31T19:00:00', 'single', -18000, -18000),
    ('2024-03-10T01:59:59.999999999', 'single', -18000, -18000),
    ('2024-03-10T02:00:00', 'gap', -18000, -14400),
    ('2024-03-10T02:30:00', 'gap', -18000, -14400),
    ('2024-03-10T02:59:59.999999999', 'gap', -18000, -14400),
    ('2024-03-10T03:00:00', 'single', -14400, -14400),
    ('2024-07-15T17:30:00', 'single', -14400, -14400),
    ('2024-11-03T00:59:59.999999999', 'single', -14400, -14400),
    ('2024-11-03T01:00:00', 'fold', -14400, -18000),
    ('2024-11-03T01:30:00', 'fold', -14400, -18000),
    ('2024-11-03T01:59:59.999999999', 'fold', -14400, -18000),
    ('2024-11-03T02:00:00', 'single', -18000, -18000),
]


def ny_offset(instant):
    # Independent literal UTC transition boundaries for 2024. Only fixtures
    # in 1969, February, July and around these two boundaries reach here.
    return -14400 if datetime(2024, 3, 10, 7) <= instant < datetime(2024, 11, 3, 6) else -18000


def row(local='2024-02-29T12:34:56.123456789', source='utc.tzif', destination='utc.tzif', **extra):
    return dict(local=local, source=source, destination=destination, **extra)


def answer(item, resolution='single', before=0, after=0, custom=False):
    policy = item.get('policy', 'Reject')
    if resolution == 'gap':
        chosen = after if policy == 'Earlier' else before
    else:
        chosen = after if policy == 'Later' else before
    whole, _, fraction = item['local'].partition('.')
    wall = datetime.fromisoformat(whole)
    utc = wall - timedelta(seconds=chosen)
    nanos = fraction.rstrip('0')
    civil_fraction = '.' + nanos if nanos else ''
    src_offset = ny_offset(utc) if item['source'] == 'new-york.tzif' else before
    dst_offset = (ny_offset(utc) if item['destination'] == 'new-york.tzif' else
                  18000 if item['destination'] == 'plus-five.tzif' else 0)
    shown = utc + timedelta(seconds=dst_offset)
    def civil(value):
        return value.isoformat(timespec='seconds') + civil_fraction
    magnitude = abs(dst_offset)
    zone = ('+' if dst_offset >= 0 else '-') + f'{magnitude // 3600:02}{magnitude // 60 % 60:02}'
    display = shown.isoformat(timespec='seconds') + '.' + (nanos or '0') + zone
    if custom:
        display = shown.strftime('%a %b %d %Y %j %H:%M:%S') + ' ' + zone + ' %'
    return dict(local=item['local'], source=item['source'], destination=item['destination'],
                policy=policy, resolution=resolution, instant=civil(utc) + 'Z',
                source_time=civil(utc + timedelta(seconds=src_offset)),
                destination_time=display, offset_seconds=dst_offset)


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
    for path in (root / 'fixtures').iterdir():
        (data / path.name).write_bytes(path.read_bytes())

    def fixture(label, blob, expected, code=0, args=None):
        (data / (label + '.json')).write_bytes(blob)
        cases.append((label, args or [label + '.json'], (code, expected, b''), None))

    for policy in ('Earlier', 'Later'):
        rows = [row(text, 'new-york.tzif', policy=policy) for text, *_ in VECTORS]
        expected = [answer(item, *vector[1:]) for item, vector in zip(rows, VECTORS)]
        fixture('vectors-' + policy, canonical(rows), canonical(expected))
    singles = [(text, kind, prior, after) for text, kind, prior, after in VECTORS if kind == 'single']
    rows = [row(text, 'new-york.tzif', policy='Reject') for text, *_ in singles]
    fixture('vectors-Reject-single', canonical(rows), canonical([
        answer(item, *vector[1:]) for item, vector in zip(rows, singles)]))
    for n, (text, kind, *_) in enumerate(VECTORS):
        if kind != 'single':
            item = row(text, 'new-york.tzif')
            fixture('reject-' + str(n), canonical([item]), error('resolve', 1, kind.title()), 2)
    success = json.loads((data / 'success.json').read_bytes())
    expected = canonical([answer(success[0], 'single', -14400, -14400),
                          answer(success[1], 'fold', -14400, -18000)])
    fixture('success', canonical(success), expected)
    cases.append(('deterministic-repeat', ['success.json'], (0, expected, b''), None))
    fixture('invalid', (data / 'invalid.json').read_bytes(), error('resolve', 1, 'Gap'), 2)
    identity = row()
    fixture('utc-leap-day', canonical([identity]), canonical([answer(identity)]))
    fixed = row(source='plus-five.tzif')
    fixture('fixed-source', canonical([fixed]), canonical([answer(fixed, before=18000, after=18000)]))
    destinations = [row(local=text, destination='new-york.tzif') for text in [
        '2024-03-10T06:59:59.999999999', '2024-03-10T07:00:00', '2024-03-10T07:00:00.000000001',
        '2024-11-03T05:59:59.999999999', '2024-11-03T06:00:00', '2024-11-03T06:00:00.000000001']]
    fixture('destination-boundaries', canonical(destinations), canonical([answer(x) for x in destinations]))
    fixture('custom-format', canonical([identity]), canonical([answer(identity, custom=True)]),
            args=['custom-format.json', '--format', '%a %b %d %Y %j %H:%M:%S %z %%'])
    fixture('empty', b'[]', b'[]\n')
    fixture('bytes-exact', b'[]' + b' ' * 65534, b'[]\n')
    fixture('oversized', b' ' * 65537, error('file_bytes'), 2)
    fixture('empty-file', b'', error('json', detail='Syntax'), 2)
    fixture('malformed', b'[', error('json', detail='Syntax', offset=1), 2)
    fixture('trailing', b'[]x', error('json', detail='Syntax', offset=2), 2)
    fixture('wrong-kind', b'[1]', error('json', detail='Syntax', offset=1), 2)
    fixture('duplicate-key', b'[{"local":"x","local":"y"}]', error('json', detail='DuplicateKey', offset=14), 2)
    fixture('utf8', b'["\xff"]', error('json', detail='Syntax', offset=1), 2)
    fixture('row-limit', canonical([identity] * 65), error('row_limit', 65), 2)
    fixture('rows-exact', canonical([identity] * 64), canonical([answer(identity)] * 64))
    for label, item, expected in [
        ('out-of-range', row(local='+010000-01-01T00:00:00'), error('local', 1, 'OutOfRange')),
        ('invalid-date', row(local='2023-02-29T12:00:00'), error('local', 1, 'Invalid')),
        ('wrong-policy', identity | {'policy': 'compatible'}, error('policy', 1)),
        ('path-traversal', identity | {'source': '../utc.tzif'}, error('path')),
        ('unknown-field', identity | {'name': 'meeting'}, error('fields', 1)),
        ('field-limit', identity | {'local': 'x' * 129}, error('field_bytes', 1)),
        ('source-tzif', identity | {'source': 'malformed.tzif'}, error('source_tzif', 1, 'Invalid')),
        ('destination-tzif', identity | {'destination': 'malformed.tzif'}, error('destination_tzif', 1, 'Invalid')),
        ('missing-zone', identity | {'source': 'absent.tzif'}, error('file_open')),
    ]:
        fixture(label, canonical([item]), expected, 2)
    fixture('missing-field', b'[{"local":"2024-01-01T00:00:00"}]', error('fields', 1), 2)
    # No partial successful table when a later row rejects.
    fixture('atomic-output', canonical([identity, row('2024-11-03T01:30:00', 'new-york.tzif')]),
            error('resolve', 2, 'Fold'), 2)
    (data / 'huge.tzif').write_bytes(b'x' * 65537)
    fixture('zone-size', canonical([identity | {'source': 'huge.tzif'}]), error('file_bytes'), 2)
    for label, pattern, expected in [
        ('invalid-format', '%Q', error('format', detail='Invalid', offset=0)),
        ('unterminated-format', 'a%', error('format', detail='Invalid', offset=1)),
        ('format-size', 'x' * 129, error('field_bytes')),
    ]:
        cases.append((label, ['success.json', '--format', pattern], (2, expected, b''), None))
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
    result = dict(cases=len(cases), observations=observations, vectors=len(VECTORS),
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
