#!/usr/bin/env python3
"""Independent uuid/JSON oracle and three-engine bounded event exports.

UUID vectors: google/uuid 2d3c2a9, uuid_test.go:20-58,129-135; BSD-3-Clause,
Copyright Google Inc. See examples/eventmerge/fixtures/LICENSE.
The byte vectors are RFC-order bytes, not Go's TestBytes string-parser input.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

HELP = (b'Usage: eventmerge LEFT.ndjson RIGHT.ndjson\n'
        b'Merge event exports by canonical UUID; reject conflicting payloads.\n'
        b'Limits: 32768 bytes/file, 4096 bytes/record, 64 records total.\n'
        b'Exit: 0 success/help, 1 conflict, 2 invalid input or I/O.\n')
A = 'f47ac10b-58cc-4372-8567-0e02b2c3d479'
B = '00000000-0000-0000-0000-000000000001'


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode()


def ndjson(rows):
    return b''.join(encoded(row) + b'\n' for row in rows)


def event(identifier=A, payload=None):
    return dict(id=identifier, payload=payload)


def error(kind, source='', line=0, first_source='', first_line=0, identifier=''):
    return encoded(dict(**{'class': 'conflict' if kind == 'conflict' else 'invalid_input'},
                        error=kind, source=source, line=line, first_source=first_source,
                        first_line=first_line, id=identifier)) + b'\n'


# Keep number tokens tagged, so Python's numeric equality cannot normalize them.
class Number(str):
    pass


def structural(value):
    if isinstance(value, Number): return ('number', str(value))
    if isinstance(value, dict): return ('object', frozenset((k, structural(v)) for k, v in value.items()))
    if isinstance(value, list): return ('array', tuple(map(structural, value)))
    return (type(value).__name__, value)


def oracle(left, right):
    seen, events, duplicates, records = {}, [], 0, 0
    for name, blob in [('left.ndjson', left), ('right.ndjson', right)]:
        for line, raw in enumerate(blob.splitlines(), 1):
            row = json.loads(raw, parse_int=Number, parse_float=Number)
            identifier = str(uuid.UUID(row['id'].lower()))
            payload = structural(row['payload'])
            records += 1
            if identifier in seen:
                old, first_name, first_line = seen[identifier]
                if old != payload:
                    return 1, error('conflict', name, line, first_name, first_line, identifier), b''
                duplicates += 1
            else:
                seen[identifier] = payload, name, line
                # Preserve first record's number spellings and object order in output.
                original = raw.decode()
                decoder = json.JSONDecoder()
                # Input fixtures use compact top-level fields id then payload.
                marker = original.index('"payload":') + len('"payload":')
                _, end = decoder.raw_decode(original[marker:])
                payload_text = original[marker:marker + end]
                events.append(b'{"id":' + encoded(identifier) + b',"payload":' + payload_text.encode() + b'}')
    out = (b'{"class":"ok","events":[' + b','.join(events) +
           f'],"unique":{len(events)},"duplicates":{duplicates},"records":{records}}}\n'.encode())
    return 0, out, b''


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    selection = root / 'hirsel.json'
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    commands = []

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout, stderr, status = [output / (label + suffix) for suffix in ('.stdout', '.stderr', '.exit')]
        if runtime:
            request = output / (label + '.request.json')
            request.write_text(json.dumps(dict(argv=argv, stdout=str(stdout), stderr=str(stderr), status=str(status))))
            done = subprocess.run([sys.executable, root / 'capped', '--timeout', '60', sys.executable, __file__, '--child', request],
                                  stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=90)
            (output / (label + '.cap.log')).write_bytes(done.stdout + done.stderr)
            assert done.returncode == 0, (label, done, str(output))
            actual = int(status.read_text()), stdout.read_bytes(), stderr.read_bytes()
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=600)
            stdout.write_bytes(done.stdout); stderr.write_bytes(done.stderr); status.write_text(str(done.returncode))
            actual = done.returncode, done.stdout, done.stderr
        commands.append(dict(label=label, argv=argv, exit=actual[0]))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return actual

    for label, args in [('fmt', ['fmt', '--check']), ('check', ['check', '--json']), ('export', ['export', '--tir'])]:
        cmd = [hirsel, *args, selection]
        if label == 'export': cmd += ['-o', output / 'app.tir']
        actual = run(label, cmd)
        assert actual == (0, b'', b''), (label, actual)
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = exe

    cases = []
    def pair(label, left, right=b'', expected=None):
        l, r = label + '-left.ndjson', label + '-right.ndjson'
        (data / l).write_bytes(left); (data / r).write_bytes(right)
        if expected is None:
            expected = oracle(left, right)
            # Adapt independent source-location names to the actual fixture pair.
            expected = expected[0], expected[1].replace(b'"left.ndjson"', encoded(l)).replace(b'"right.ndjson"', encoded(r)), expected[2]
        elif isinstance(expected, str):
            expected = 2, error(expected, l, 1), b''
        cases.append((label, [l, r], expected, None))

    fixtures = root / 'fixtures'
    pair('realistic', (fixtures / 'job-a.ndjson').read_bytes(), (fixtures / 'job-b.ndjson').read_bytes())
    pair('realistic-repeat', (fixtures / 'job-a.ndjson').read_bytes(), (fixtures / 'job-b.ndjson').read_bytes())
    pair('realistic-conflict', (fixtures / 'job-a.ndjson').read_bytes(), (fixtures / 'conflict.ndjson').read_bytes())
    vectors = json.loads((fixtures / 'uuid-vectors.json').read_text())
    # Each batch has <=64 records; independently verify expected canonical bytes.
    for i, vector in enumerate(vectors):
        assert str(uuid.UUID(vector['input'].lower())) == vector['canonical']
        assert list(uuid.UUID(vector['input'].lower()).bytes) == vector['bytes']
    for start in range(0, len(vectors), 16):
        group = vectors[start:start + 16]
        pair('upstream-' + str(start), ndjson([event(v['input'], v['canonical']) for v in group]),
             ndjson([event(v['input'].upper(), v['canonical']) for v in group]))
    pair('spellings', ndjson([event(A, {'nested': [1, {'a': True, 'b': None}]})]),
         ndjson([event(s, {'nested': [1, {'b': None, 'a': True}]}) for s in [A.upper(), A.replace('-', ''), '{' + A + '}', 'URN:UUID:' + A]]))
    pair('exact-duplicate', ndjson([event(A, 'same')] * 2), ndjson([event(A, 'same')]))
    pair('first-seen', ndjson([event(B, False), event(A, True)]))
    pair('array-conflict', ndjson([event(A, [1, 2])]), ndjson([event(A, [2, 1])]))
    pair('type-conflict', ndjson([event(A, True)]), ndjson([event(A, 1)]))
    pair('number-conflict', ndjson([event(A, 1)]), b'{"id":"' + A.encode() + b'","payload":1.0}\n')
    pair('number-exponent', b'{"id":"' + A.encode() + b'","payload":1e0}\n', ndjson([event(A, 1)]))
    pair('string-decoding', b'{"id":"' + A.encode() + b'","payload":"\\u0061"}\n', ndjson([event(A, 'a')]),
         (0, b'{"class":"ok","events":[{"id":"' + A.encode() + b'","payload":"a"}],"unique":1,"duplicates":1,"records":2}\n', b''))
    pair('empty', b'')
    pair('crlf-no-final-newline', ndjson([event(A, None)]).replace(b'\n', b'\r\n'), ndjson([event(B, '羊')]).rstrip(b'\n'))
    for i, spelling in enumerate(['bad', A[:-1] + 'g', '[' + A + ']', 'urn:nope:' + A, A + ' ', A.replace('-', '_')]):
        pair('invalid-id-' + str(i), ndjson([event(spelling)]), expected='id')
    for label, blob, kind in [('malformed', b'{', 'json'), ('duplicate-field', b'{"id":"x","id":"y","payload":0}', 'json'),
                              ('blank', b'\n', 'json'), ('not-object', b'null', 'fields'),
                              ('missing-field', b'{"id":"x"}', 'fields'), ('wrong-id-type', b'{"id":1,"payload":0}', 'fields'),
                              ('extra-field', ndjson([event() | {'extra': 0}]), 'fields'), ('utf8', b'\xff', 'utf8')]:
        pair(label, blob, expected=kind)
    pair('unique-capacity', ndjson([event(f'00000000-0000-0000-0000-{i:012x}', i) for i in range(32)]),
         ndjson([event(f'00000000-0000-0000-0000-{i:012x}', i) for i in range(32, 64)]))
    pair('same-file-conflict', ndjson([event(A, 'done'), event(A, 'failed')]))
    pair('capacity-exact', ndjson([event()]*32), ndjson([event()]*32))
    pair('capacity-over', ndjson([event()]*32), ndjson([event()]*33),
         (2, error('record_limit', 'capacity-over-right.ndjson', 33), b''))
    def sized_record(n):
        base = encoded(event(A, ''))
        return encoded(event(A, 'x' * (n - len(base))))
    pair('record-exact', sized_record(4096))
    pair('record-over', sized_record(4097), expected='record_bytes')
    pair('input-exact', (sized_record(4095) + b'\n') * 8)
    pair('input-over', (sized_record(4095) + b'\n') * 8 + b'x',
         expected=(2, error('input_bytes', 'input-over-left.ndjson'), b''))
    cases += [('help', ['--help'], (0, HELP, b''), 'dir'), ('usage', [], (2, error('usage'), b''), None),
              ('missing-grant', ['unused', 'unused'], (2, error('directory_grant'), b''), 'dir'),
              ('missing-stdout', ['unused', 'unused'], (2, b'', b''), 'stdout'),
              ('missing-file', ['absent.ndjson', 'absent.ndjson'], (2, error('file_open', 'absent.ndjson'), b''), None),
              ('path', ['../escape', 'unused'], (2, error('path', '../escape'), b''), None)]
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    grants = output / 'grants.json'
    observations = 0
    for label, args, expected, missing in cases:
        policy = dict(io=dict(stdout='stdout.bytes@1', dirs=[dict(path='input', host=str(data), access='read')]), args=args)
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
    data.rmdir(); grants.unlink()
    assert not data.exists() and not grants.exists(), 'cleanup'
    result = dict(cases=len(cases), observations=observations, vectors=len(vectors), engines=['h', 'llvm', 'cranelift'], cleanup=True)
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
    if args.child: child(args.child)
    else: verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
