#!/usr/bin/env python3
"""Private E2E helper: independent fixtures, ephemeral HTTP server, offline replay.

Local server/request/read/close structure adapts Go client_test.go:63–81,
ebe2feefc400f813aefdc8a036e66d2619da6649, BSD-3-Clause (Go Authors).
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import threading
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Server(socketserver.TCPServer):
    allow_reuse_address = False

    def __init__(self, fixtures, output):
        self.fixtures = fixtures
        self.requests = []
        self.unexpected = []
        super().__init__(('127.0.0.1', 0), Handler)
        self.base = f'http://127.0.0.1:{self.server_address[1]}'
        output.mkdir(parents=True, exist_ok=True)
        (output / 'base.txt').write_text(self.base + '\n')
        self.grants = output / 'grants.json'
        self.grants.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1', 'dirs': [],
            'net': {'provider': 'net.stream@1', 'rules': [{'address': '127.0.0.1',
            'service': str(self.server_address[1]), 'operation': 'connect'}]}}, 'args': []}) + '\n')


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(5)
        data = b''
        while b'\r\n\r\n' not in data and len(data) < 8192:
            part = self.request.recv(4096)
            if not part:
                return
            data += part
        first = data.split(b'\r\n')[0].decode('ascii')
        self.server.requests.append(first)
        if not first.startswith('GET /') or not first.endswith(' HTTP/1.1'):
            self.server.unexpected.append(first)
        route = first.split(' ')[1].lstrip('/')
        status, extra, body = 200, b'', b''
        fixtures = self.server.fixtures
        files = {'first': 'first.json', 'second': 'second.json', 'empty': 'empty.json',
                 'duplicate': 'duplicate.json', 'missing': 'missing.json',
                 'unknown': 'unknown.json', 'type': 'type.json', 'bad-json': 'bad-json.json'}
        if route in files:
            body = (fixtures / files[route]).read_bytes()
        elif route in ('204', '404', '500'):
            status = int(route)
        elif route == 'redirect':
            # Port 1 is outside the endpoint grant; following even once is a failure.
            status, extra = 302, b'Location: http://127.0.0.1:1/escape\r\n'
        elif route == 'compressed':
            body, extra = b'not gzip', b'Content-Encoding: gzip\r\n'
        elif route in ('body-cap', 'body-over', 'chunk-over', 'eof-over'):
            base = (fixtures / 'empty.json').read_bytes()
            body = base + b' ' * (65536 - len(base) + (route != 'body-cap'))
        elif route in ('items-cap', 'items-over'):
            body = b'{"items":[' + b','.join([b'{"name":"a","value":1}'] *
                    (1024 if route == 'items-cap' else 1025)) + b']}'
        elif route in ('name-cap', 'name-over'):
            # 128 two-byte characters, plus one byte only in the rejected case.
            name = 'é' * 128 + ('a' if route == 'name-over' else '')
            body = ('{"items":[{"name":"' + name + '","value":0}]}').encode()
        elif route == 'invalid-utf8':
            body = b'{"items":[{"name":"\xff","value":1}]}'
        elif route == 'truncated':
            body = b'{"items":[]}'
        else:
            self.server.unexpected.append(route)
            status = 500
        head = f'HTTP/1.1 {status} Fixture\r\n'.encode() + extra
        if route == 'chunk-over':
            head += b'Transfer-Encoding: chunked\r\n'
            body = f'{len(body):x}\r\n'.encode() + body + b'\r\n0\r\n\r\n'
        elif route != 'eof-over':
            head += f'Content-Length: {len(body) + (10 if route == "truncated" else 0)}\r\n'.encode()
        head += b'Connection: close\r\n\r\n'
        try:
            self.request.sendall(head + body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Expected when the app rejects a status or an over-limit body.


def verify(hirsel, root, output, prebuilt=None):
    output.mkdir(parents=True, exist_ok=False)
    fixtures, selection = root / 'fixtures', root / 'hirsel.json'
    records, binaries = [], {}
    frozen_paths = [*root.rglob('*'), hirsel, Path(__file__)]
    if prebuilt:
        frozen_paths.extend(prebuilt / ('httpaggregate-' + b) for b in ('llvm', 'cranelift'))
    manifest = json.loads(selection.read_text())
    frozen_paths.extend((root / manifest['dependencies'][0]['manifest']).resolve().parent.rglob('*'))
    for key in ('HIRSEL_IMAGE', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX'):
        if os.environ.get(key):
            path = Path(os.environ[key])
            frozen_paths.extend(path.rglob('*') if path.is_dir() else [path])
    frozen = {str(p): digest(p) for p in frozen_paths if p.is_file()}
    (output / 'frozen.json').write_text(json.dumps(frozen, indent=2) + '\n')
    result = {'status': 'running', 'observations': 0, 'recordings': 0, 'replays': 0,
              'builds': {'llvm': 0, 'cranelift': 0}}
    server = Server(fixtures, output)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    stopped = False

    def run(label, argv):
        start = time.monotonic()
        command = list(map(str, argv))
        done = subprocess.run(command, capture_output=True, stdin=subprocess.DEVNULL, timeout=150)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        records.append({'label': label, 'argv': command, 'cwd': str(Path.cwd()),
                        'exit': done.returncode, 'seconds': time.monotonic() - start})
        (output / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        return done.returncode, done.stdout, done.stderr

    def expect(label, argv, expected, routes=None):
        before = len(server.requests)
        actual = run(label, argv)
        assert actual == expected, (label, actual, expected, str(output))
        if routes is not None:
            assert server.requests[before:] == [f'GET /{r} HTTP/1.1' for r in routes], (label, server.requests[before:], routes)
        result['observations'] += 1

    def command(engine, args, grants=None, tape=None):
        grants = grants or server.grants
        if engine == 'interpreted':
            return [hirsel, 'run', selection, '--grants', grants,
                    *(['--record', tape] if tape else []), '--', *args]
        return [binaries[engine], *(['--hirsel-record', tape] if tape else []),
                '--hirsel-grants', grants, *args]

    def failure(error):
        return 1, b'', f'error: {error}\n'.encode()

    def expected(name):
        return 0, (fixtures / (name + '.out')).read_bytes(), b''

    try:
        for engine in result['builds']:
            binary = (prebuilt or output) / ('httpaggregate-' + engine)
            if prebuilt:
                assert binary.is_file(), binary
                binaries[engine] = binary
                continue
            actual = run('build-' + engine, [hirsel, 'build', selection, '--backend', engine, '-o', binary])
            assert actual[0] == 0, ('build-' + engine, actual)
            binaries[engine] = binary
            result['builds'][engine] += 1
        binary_hashes = {str(p): digest(p) for p in binaries.values()}
        denied = output / 'denied.json'
        denied.write_text('{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}\n')
        no_stdout = output / 'no-stdout.json'
        grants = json.loads(server.grants.read_text())
        del grants['io']['stdout']
        no_stdout.write_text(json.dumps(grants))
        policies = {str(p): digest(p) for p in (denied, no_stdout, server.grants)}
        base = server.base
        # Exact expected bytes are independent literal goldens, not model output.
        cases = [('combined', ['first', 'second'], expected('combined')),
                 ('reverse', ['second', 'first'], expected('combined')),
                 ('empty', ['empty'], expected('empty')),
                 ('sixteen', ['empty'] * 16, expected('empty')),
                 ('body-cap', ['body-cap'], expected('empty')),
                 ('body-over', ['body-over'], failure('Http(BodyTooLarge)')),
                 ('chunk-over', ['chunk-over'], failure('Http(BodyTooLarge)')),
                 ('eof-over', ['eof-over'], failure('Http(BodyTooLarge)')),
                 ('items-cap', ['items-cap'], (0, b'[{"name":"a","total":1024}]\n', b'')),
                 ('items-over', ['items-over'], failure('Input(ItemLimit)')),
                 ('name-cap', ['name-cap'], (0, ('[{"name":"' + 'é' * 128 + '","total":0}]\n').encode(), b'')),
                 ('name-over', ['name-over'], failure('Input(NameLimit)')),
                 ('invalid-utf8', ['invalid-utf8'], failure('Input(InvalidJson)')),
                 ('compressed', ['compressed'], failure('Encoding')),
                 ('truncated', ['truncated'], failure('Http(Truncated)')),
                 ('redirect', ['redirect'], failure('Http(TooManyRedirects)'))]
        cases += [(r, ['first', r], failure(e)) for r, e in [
            ('duplicate', 'Input(InvalidJson)'), ('missing', 'Input(InvalidShape)'),
            ('unknown', 'Input(InvalidShape)'), ('type', 'Input(InvalidShape)'),
            ('bad-json', 'Input(InvalidJson)'), ('204', 'Status(204)'),
            ('404', 'Status(404)'), ('500', 'Status(500)')]]
        engines = ('interpreted', 'llvm', 'cranelift')
        tapes = []
        for engine in engines:
            for flag in ('--help', '-h'):
                expect(engine + '-help-' + flag.lstrip('-'), command(engine, [flag]), expected('help'), [])
            for index, args in enumerate([[], ['--wat'], ['--help', '--help'], ['--help', base + '/first'], [base + '/empty'] * 17]):
                expect(engine + '-usage-' + str(index), command(engine, args), failure('Usage'), [])
            for label, routes, triple in cases:
                expect(engine + '-' + label, command(engine, [base + '/' + r for r in routes]), triple, routes)
            expect(engine + '-no-net', command(engine, [base + '/first'], denied), failure('Grant(NotGranted)'), [])
            expect(engine + '-no-stdout', command(engine, [base + '/first'], no_stdout), failure('Grant(NotGranted)'), [])
            expect(engine + '-stop-on-failure', command(engine, [base + '/first', base + '/bad-json', base + '/second']), failure('Input(InvalidJson)'), ['first', 'bad-json'])
            expect(engine + '-outside-grant', command(engine, ['http://127.0.0.1:1/escape']), failure('Http(Net(NotGranted))'), [])
            expect(engine + '-malformed-url', command(engine, ['not-a-url']), failure('Http(Url(InvalidFormat))'), [])
            # A success and deterministic failure recorded independently per engine.
            for prefix, routes, triple in [('', ['first', 'second'], expected('combined')),
                    ('failure-', ['first', 'bad-json'], failure('Input(InvalidJson)'))]:
                tape = output / (prefix + engine + '.tape')
                expect('record-' + tape.stem, command(engine, [base + '/' + r for r in routes], tape=tape), triple, routes)
                tapes.append((tape, triple))
                result['recordings'] += 1
        assert not server.unexpected, server.unexpected
        assert all(digest(Path(p)) == h for p, h in policies.items())
        # Shut down and close the socket before replay; deny live authority too.
        server.shutdown()
        thread.join()
        server.server_close()
        stopped = True
        try:
            with socket.create_connection(('127.0.0.1', server.server_address[1]), timeout=1):
                raise AssertionError('server still reachable')
        except ConnectionRefusedError:
            pass
        for engine in engines:
            expect(engine + '-server-stopped', command(engine, [base + '/first']), failure('Http(Net(ConnectionRefused))'), [])
        for p in (server.grants, denied, no_stdout):
            p.rename(p.with_suffix('.retired.json'))
        tape_hashes = {str(p): digest(p) for p, _ in tapes}
        for tape, triple in tapes:
            expect('replay-interpreted-' + tape.stem,
                   [hirsel, 'run', selection, '--replay', tape], triple, [])
            result['replays'] += 1
            for engine, binary in binaries.items():
                expect('replay-' + engine + '-' + tape.stem,
                       [binary, '--hirsel-replay', tape], triple, [])
                result['replays'] += 1
        assert all(digest(Path(p)) == h for p, h in {**binary_hashes, **tape_hashes}.items())
        (output / 'identities.json').write_text(json.dumps({**policies, **binary_hashes, **tape_hashes}, indent=2) + '\n')
        assert result['recordings'] == 6 and result['replays'] == 18, result
        assert result['observations'] == 135, result
        result['status'] = 'passed'
        print(f"{result['observations']} exact observations; 6 records; 18 replays; builds={result['builds']}")
    finally:
        if not stopped:
            server.shutdown()
            thread.join()
            server.server_close()
        (output / 'requests.txt').write_text('\n'.join(server.requests) + '\n')
        unchanged = all(Path(p).is_file() and digest(Path(p)) == h for p, h in frozen.items())
        result['frozen_inputs_unchanged'] = unchanged
        if result['status'] == 'running':
            result['status'] = 'failed'
        (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        assert unchanged, 'consumed source/toolchain changed'


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--serve', type=Path)
    parser.add_argument('--hirsel', type=Path)
    parser.add_argument('--root', type=Path)
    parser.add_argument('--prebuilt', type=Path, help='Reuse both already-built application binaries')
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args()
    os.environ.update(HIRSEL_JOBS='1', HIRSEL_DAEMON='off')
    if options.serve:
        with Server(options.serve.resolve(), options.output.resolve()) as server:
            print(server.base, flush=True)
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass
    else:
        verify(options.hirsel.resolve(), options.root.resolve(), options.output.resolve(),
               options.prebuilt.resolve() if options.prebuilt else None)
