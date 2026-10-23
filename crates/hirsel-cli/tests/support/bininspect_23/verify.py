#!/usr/bin/env python3
"""Wire vectors from RFC 8949 Appendix A and rmpv decode.rs:50-90.

CBOR: cbor/test-vectors aba89b65 (Simplified BSD, IETF Trust).
rmpv: cf880019 (MIT, Copyright (c) 2017 Evgeny Safronov).
Licenses: third-party/NOTICES.md. Only JSON-domain vectors are successful;
binary/tag/ext are precise Unsupported controls. No expected bytes use Hirsel.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def invalid(kind, offset=None):
    return 2, b'', canonical(dict(error='invalid_input', kind=kind, offset=offset))


def wire(value, fmt, deterministic=False):
    """Small independent encoder for the fixture domain; lengths/ints are exact.

    This is a test oracle, not a decoder or production implementation. Struct
    supplies IEEE-754 bytes. Deterministic CBOR vectors use exactly representable
    half floats; other floats use binary64 as the application documents.
    """
    def head(major, n):
        if n < 24:
            return bytes([major * 32 + n])
        for info, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
            if n < 1 << (size * 8):
                return bytes([major * 32 + info]) + n.to_bytes(size, 'big')
        raise ValueError(n)

    if value is None:
        return b'\xf6' if fmt == 'cbor' else b'\xc0'
    if isinstance(value, bool):
        return bytes([(0xf4 if fmt == 'cbor' else 0xc2) + value])
    if isinstance(value, int):
        if fmt == 'cbor':
            return head(0, value) if value >= 0 else head(1, -1 - value)
        if 0 <= value < 128 or -32 <= value < 0:
            return bytes([value & 255])
        if value >= 0:
            for marker, size in ((0xcc, 1), (0xcd, 2), (0xce, 4), (0xcf, 8)):
                if value < 1 << (size * 8):
                    return bytes([marker]) + value.to_bytes(size, 'big')
        else:
            for marker, size in ((0xd0, 1), (0xd1, 2), (0xd2, 4), (0xd3, 8)):
                if value >= -(1 << (size * 8 - 1)):
                    return bytes([marker]) + value.to_bytes(size, 'big', signed=True)
        raise ValueError(value)
    if isinstance(value, float):
        if fmt == 'cbor' and deterministic:
            assert struct.unpack('>e', struct.pack('>e', value))[0] == value
            return b'\xf9' + struct.pack('>e', value)
        return (b'\xfb' if fmt == 'cbor' else b'\xcb') + struct.pack('>d', value)
    if isinstance(value, str):
        data = value.encode()
        if fmt == 'cbor':
            return head(3, len(data)) + data
        if len(data) < 32:
            return bytes([0xa0 + len(data)]) + data
        if len(data) <= 255:
            return b'\xd9' + bytes([len(data)]) + data
        return b'\xda' + len(data).to_bytes(2, 'big') + data
    if isinstance(value, list):
        prefix = head(4, len(value)) if fmt == 'cbor' else (
            bytes([0x90 + len(value)]) if len(value) < 16 else b'\xdc' + len(value).to_bytes(2, 'big'))
        return prefix + b''.join(wire(v, fmt, deterministic) for v in value)
    assert isinstance(value, dict)
    keys = list(value)
    if fmt == 'cbor' and deterministic:
        keys.sort(key=lambda k: wire(k, fmt))
    prefix = head(5, len(keys)) if fmt == 'cbor' else (
        bytes([0x80 + len(keys)]) if len(keys) < 16 else b'\xde' + len(keys).to_bytes(2, 'big'))
    return prefix + b''.join(wire(k, fmt, deterministic) + wire(value[k], fmt, deterministic) for k in keys)


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'input'
    data.mkdir()
    grants = output / 'grants.json'
    selection = root / 'hirsel.json'
    commands = []
    policy = json.loads((root / 'grants.json').read_text())
    assert policy == dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
                                 dirs=[dict(path='input', host='fixtures', access='read')]), args=[])
    policy['io']['dirs'][0]['host'] = str(data.resolve())

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout, stderr = (output / (label + suffix) for suffix in ('.stdout', '.stderr'))
        env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                                HIRSEL_MEMORY_LIMIT='2G', HIRSEL_JOBS='1')
        if runtime:
            wrapper = ('import subprocess,sys; '
                       'o=open(sys.argv[1],"wb"); e=open(sys.argv[2],"wb"); '
                       'p=subprocess.run(sys.argv[3:],stdin=subprocess.DEVNULL,stdout=o,stderr=e); '
                       'sys.exit(p.returncode)')
            done = subprocess.run([sys.executable, str(root / 'capped'), '--timeout', '60',
                                   sys.executable, '-c', wrapper, str(stdout), str(stderr), *argv],
                                  capture_output=True, timeout=90, env=env)
            (output / (label + '.scope')).write_bytes(done.stdout + done.stderr)
            actual = done.returncode, stdout.read_bytes(), stderr.read_bytes()
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=300, env=env)
            stdout.write_bytes(done.stdout)
            stderr.write_bytes(done.stderr)
            actual = done.returncode, done.stdout, done.stderr
        commands.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return actual

    for label, argv in [
        ('fmt', [hirsel, 'fmt', '--check', selection]),
        ('check', [hirsel, 'check', '--json', selection]),
        ('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']),
    ]:
        actual = run(label, argv)
        assert actual == (0, b'', b''), (label, actual)
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = exe

    cases = []

    def add(label, fmt, raw, expected, options=()):
        name = label + '.dat'
        (data / name).write_bytes(raw)
        cases.append((label, [fmt, name, *options], expected, None))

    for v in json.loads((root / 'fixtures/vectors.json').read_text()):
        expected = (0, v['json'].encode() + b'\n', b'') if 'json' in v else invalid(v['kind'], v['offset'])
        add(v['format'] + '-' + v['label'], v['format'], bytes.fromhex(v['hex']), expected)

    cache = json.loads((root / 'fixtures/cache.json').read_text())
    roundtrips = []
    for fmt in ('cbor', 'msgpack'):
        cache_wire = (root / ('fixtures/cache.' + fmt)).read_bytes()
        assert cache_wire == wire(cache, fmt), 'checked-in wire differs from independent oracle'
        add('cache-' + fmt, fmt, cache_wire, (0, canonical(cache), b''))
        add('encode-cache-' + fmt, fmt, canonical(cache), (0, cache_wire, b''), ['--encode'])
        roundtrips.append((fmt, 'encode-cache-' + fmt))
        # Collection caps check both exact and over-limit at nested sites.
        for size in (64, 65):
            value = [None] * size
            expected = (0, canonical(value), b'') if size == 64 else invalid('collection_items_limit')
            add(fmt + '-array-' + str(size), fmt, wire(value, fmt), expected)
        for size in (64, 65):
            value = {str(i): None for i in range(size)}
            expected = (0, canonical(value), b'') if size == 64 else invalid('collection_items_limit')
            add(fmt + '-map-' + str(size), fmt, wire(value, fmt), expected)
        for size in (31, 32, 255, 256):
            value = 'x' * size
            add(fmt + '-string-' + str(size), fmt, wire(value, fmt), (0, canonical(value), b''))
        add(fmt + '-encode-over', fmt, canonical([list(range(65))]), invalid('collection_items_limit'), ['--encode'])
        # An exact, modest integer/float-domain encode control.
        numeric = [0, 24, 255, 256, 65535, 65536, 4294967295, 4294967296, 18446744073709551615, -33, 1.5, -0.0]
        add(fmt + '-encode-numeric', fmt, canonical(numeric), (0, wire(numeric, fmt), b''), ['--encode'])
        add(fmt + '-invalid-number', fmt, b'18446744073709551616', invalid('InvalidNumber'), ['--encode'])
        add(fmt + '-oversized', fmt, b'\x00' * 65537, invalid('input_bytes_limit'))

    add('realistic-invalid', 'cbor', (root / 'fixtures/invalid.cbor').read_bytes(), invalid('Unsupported', 6))
    deterministic = {'zz': 1.5, 'a': -0.0, 'b': [1, None]}
    for label, value in [('first', deterministic), ('reordered', dict(reversed(list(deterministic.items()))))]:
        add('deterministic-' + label, 'cbor', canonical(value),
            (0, wire(deterministic, 'cbor', True), b''), ['--encode', '--deterministic'])
    add('json-utf8', 'cbor', b'\xff', invalid('json_utf8'), ['--encode'])
    # Parser offsets are independently counted byte positions.
    add('json-malformed', 'cbor', b'[', invalid('JsonSyntax', 1), ['--encode'])
    add('json-duplicate', 'msgpack', b'{"a":1,"a":2}', invalid('JsonDuplicateKey', 7), ['--encode'])

    help_bytes = (b'bininspect cbor|msgpack FILE [--encode [--deterministic]]\n'
                  b'Decode one binary value to compact JSON, or encode JSON to raw binary stdout.\n'
                  b'FILE is relative to the read-only input grant.\n'
                  b'Limits: 65536 input bytes, 64 items per array/map, 128 open containers.\n'
                  b'Exit: 0 success/help, 2 invalid input, usage or I/O.\n')
    no_dir = dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[]), args=[])
    cases.extend([
        ('help', ['--help'], (0, help_bytes, b''), no_dir),
        ('missing-directory', ['cbor', 'cache-cbor.dat'], invalid('directory_grant'), no_dir),
        ('missing-file', ['cbor', 'absent'], invalid('file_open'), None),
        ('escape', ['cbor', '../sentinel'], invalid('file_open'), None),
        ('usage', [], invalid('usage'), None),
        ('format', ['bson', 'cache-cbor.dat'], invalid('format'), None),
        ('bad-option', ['cbor', 'cache-cbor.dat', '--decode'], invalid('usage'), None),
        ('msgpack-deterministic', ['msgpack', 'cache-msgpack.dat', '--encode', '--deterministic'], invalid('usage'), None),
        ('missing-stdout', ['cbor', 'cache-cbor.dat'], (2, b'', b''), dict(io=dict(stderr='stderr.bytes@1', dirs=[]), args=[])),
        ('missing-stderr', ['cbor', 'realistic-invalid.dat'], (2, b'', b''),
         dict(io=dict(stdout='stdout.bytes@1', dirs=policy['io']['dirs']), args=[])),
    ])
    (output / 'sentinel').write_bytes(b'outside grant\n')

    def snapshot():
        return {str(p.relative_to(data)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in data.rglob('*') if p.is_file()}

    before = snapshot()
    executed = 0
    for engine in ('interpreter', 'llvm', 'cranelift'):
        def app(label, args, grant=None):
            grants.write_text(json.dumps(policy if grant is None else grant))
            command = ([hirsel, 'run', selection, '--grants', grants, '--', *args]
                       if engine == 'interpreter' else [binaries[engine], '--hirsel-grants', grants, *args])
            return run(engine + '-' + label, command, runtime=True)

        for label, args, expected, grant in cases:
            actual = app(label, args, grant)
            assert actual == expected, (engine, label, actual, expected)
            assert snapshot() == before, (engine, label, 'fixture mutation')
            assert (output / 'sentinel').read_bytes() == b'outside grant\n'
            executed += 1
        assert app('repeat', ['cbor', 'cache-cbor.dat']) == (0, canonical(cache), b'')
        executed += 1
        for fmt, label in roundtrips:
            # Feed the actual encoded output back into the decoder, preserving
            # the original cell's logs; this checks the JSON-domain roundtrip.
            actual_wire = (output / (engine + '-' + label + '.stdout')).read_bytes()
            path = data / 'roundtrip.dat'
            path.write_bytes(actual_wire)
            assert app('roundtrip-' + fmt, [fmt, path.name]) == (0, canonical(cache), b'')
            path.unlink()
            assert snapshot() == before
            executed += 1
    for file in data.iterdir():
        file.unlink()
    data.rmdir()
    grants.unlink()
    (output / 'sentinel').unlink()
    assert not data.exists() and not grants.exists() and not (output / 'sentinel').exists()
    print(json.dumps(dict(cases=len(cases), engines=3, executed=executed, cleanup=True)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
