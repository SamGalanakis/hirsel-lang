#!/usr/bin/env python3
"""Independent struct/json Unix peers; exact stdout/stderr/status/effects per cell.

Tokio framing vectors are checked separately in fixtures/vectors.hir. This oracle
uses Python struct's fixed !I and ASCII-only translation; no expected result is
computed by Hirsel. Every fixture process has CPU, data and wall limits.
"""
import argparse
import json
import os
from pathlib import Path
import resource
import select
import socket
import struct
import subprocess
import tempfile


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode()


def frame(data):
    return struct.pack('!I', len(data)) + data


def oracle(request):
    if request['method'] == 'ping':
        return dict(id=request['id'], result='pong')
    if request['method'] == 'normalize':
        text = request['text'].strip(' \t\n\r\v\f')
        return dict(id=request['id'], result=text.translate(str.maketrans('ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')))
    return dict(id=request['id'], error=dict(code='unknown_method', method=request['method']))


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
    resource.setrlimit(resource.RLIMIT_DATA, (2 * 1024**3, 2 * 1024**3))


def verify(hirsel, root, output):
    output.mkdir()
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    records = []
    executions = 0

    def retain(label, argv, result):
        code, stdout, stderr = result
        (output / (label + '.stdout')).write_bytes(stdout)
        (output / (label + '.stderr')).write_bytes(stderr)
        records.append(dict(label=label, argv=list(map(str, argv)), code=code))
        (output / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        return result

    def run(label, argv, setup=False):
        nonlocal executions
        p = subprocess.run(list(map(str, argv)), capture_output=True, stdin=subprocess.DEVNULL,
                           timeout=240 if setup else 60, env=env, preexec_fn=None if setup else limits)
        if not setup:
            executions += 1
        return retain(label, argv, (p.returncode, p.stdout, p.stderr))

    selection = root / 'hirsel.json'
    assert run('check', [hirsel, 'check', '--json', selection], True) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir'], True) == (0, b'', b'')
    # Distinct reachable entry for the upstream raw-payload vectors.
    package = json.loads(selection.read_text())
    package['sources'][0]['path'] = 'vectors.hir'
    model_selection = root / 'vectors.json'
    model_selection.write_text(json.dumps(package))
    binaries = {}
    model_binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        assert run('build-' + backend, [hirsel, 'build', '--backend', backend, selection, '-o', exe], True) == (0, b'', b'')
        binaries[backend] = exe
        exe = output / ('vectors-' + backend)
        assert run('build-vectors-' + backend, [hirsel, 'build', '--backend', backend, model_selection, '-o', exe], True) == (0, b'', b'')
        model_binaries[backend] = exe

    # Short absolute socket paths are also private on pool workers with long roots.
    with tempfile.TemporaryDirectory(prefix='lr23-') as private:
        private = Path(private)
        path = private / 'rpc.sock'
        sentinel = private / 'untouched'
        sentinel.write_bytes(b'keep\0\xff')
        original_fixtures = {p.name: p.read_bytes() for p in (root / 'normalize.json', root / 'invalid.json')}
        policy = dict(io={}, args=[])

        def command(engine, args, operations=('unix_connect', 'unix_listen'), stdout=True, net=True):
            policy['args'] = args
            policy['io'] = dict(dirs=[])
            if stdout:
                policy['io']['stdout'] = 'stdout.bytes@1'
            if net:
                policy['io']['net'] = dict(provider='net.stream@1', rules=[dict(address=str(path), service='0', operation=op) for op in operations])
            grants = output / ('grant-' + str(len(records)) + '-' + engine + '.json')
            grants.write_text(json.dumps(policy))
            if engine == 'h':
                return [hirsel, 'run', selection, '--max-steps', '1000000', '--grants', grants]
            return [binaries[engine], '--hirsel-grants', grants, *args]

        def expect(label, argv, code=0, stdout=b''):
            result = run(label, argv)
            assert result == (code, stdout, b''), (label, result, (code, stdout, b''))
            assert not path.exists(), (label, 'socket leaked')
            assert sentinel.read_bytes() == b'keep\0\xff'

        def start(engine, label):
            argv = command(engine, ['serve', str(path)], ('unix_listen',))
            p = subprocess.Popen(list(map(str, argv)), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, preexec_fn=limits)
            try:
                assert select.select([p.stdout], [], [], 60)[0], (label, 'no readiness')
                ready = p.stdout.readline()
                assert ready == b'{"listening":true}\n', (label, ready, p.communicate(timeout=5))
            except BaseException:
                p.kill()
                p.communicate()
                raise
            return p, argv, ready

        def finish(label, started, code, stdout):
            nonlocal executions
            p, argv, ready = started
            try:
                out, err = p.communicate(timeout=60)
            except BaseException:
                p.kill()
                p.communicate()
                raise
            executions += 1
            result = retain(label, argv, (p.returncode, ready + out, err))
            assert result == (code, ready + stdout, b''), (label, result)
            assert not path.exists(), (label, 'socket leaked')

        def recv_exact(peer, count):
            result = b''
            while len(result) < count:
                part = peer.recv(count - len(result))
                assert part, ('truncated peer response', result)
                result += part
            return result

        def recv(peer):
            length, = struct.unpack('!I', recv_exact(peer, 4))
            assert 0 < length <= 2048
            return recv_exact(peer, length)

        success = json.loads((root / 'normalize.json').read_text())
        invalid = (root / 'invalid.json').read_text().strip()
        ping = dict(id='a', method='ping')
        unknown = dict(id='draft-45', method='summarize')
        escaped = dict(id='draft\"\\羊', method='normalize', text='\tStraße \"羊\"\n')
        cases = [success, ping, unknown, escaped]
        invalid_requests = [
            ('json', '{'), ('shape', invalid), ('shape', '{}'),
            ('shape', '{"id":"a","id":"b","method":"ping"}', 'json'),
            ('shape', compact(dict(id='', method='ping')).decode()),
            ('shape', compact(dict(id='x'*65, method='ping')).decode()),
            ('shape', compact(dict(id='a', method='x'*33)).decode()),
            ('shape', compact(dict(id='a', method='ping', extra=1)).decode()),
            ('text_limit', compact(dict(id='a', method='normalize', text='x'*513)).decode()),
            ('frame_limit', 'x'*2049),
        ]
        for engine in ('h', 'llvm', 'cranelift'):
            if engine == 'h':
                argv = [hirsel, 'run', model_selection, '--max-steps', '1000000']
            else:
                argv = [model_binaries[engine]]
            expect(engine + '-vectors', argv)
            expect(engine + '-help', command(engine, ['--help'], ()), stdout=b'usage: localrpc serve SOCKET | call SOCKET REQUEST\n4-byte big-endian length + UTF-8 JSON; max 2048 bytes, 8 requests.\n')
            expect(engine + '-missing-stdout', command(engine, ['--help'], (), stdout=False, net=False), 3)
            expect(engine + '-missing-net', command(engine, ['serve', str(path)], (), net=False), 3, b'{"error":"io"}\n')
            expect(engine + '-usage', command(engine, [], ()), 2, b'{"error":"usage"}\n')
            expect(engine + '-missing-listen', command(engine, ['serve', str(path)], ()), 3, b'{"error":"io"}\n')
            expect(engine + '-missing-connect', command(engine, ['call', str(path), compact(ping).decode()], ()), 3, b'{"error":"io"}\n')
            for index, item in enumerate(invalid_requests):
                error, text, *override = item
                error = override[0] if override else error
                expect(f'{engine}-invalid-{index}', command(engine, ['call', str(path), text], ()), 2, compact(dict(error=error)) + b'\n')

            # Independent Python client: coalesced frames, split header/body and
            # the eight-request limit, with no EOF needed at that limit.
            for label, requests, split in [('one', [success], False), ('coalesced', cases, False), ('split', cases, True), ('limit', [ping]*8, False), ('exact-text', [dict(id='a', method='normalize', text='x'*512)], False), ('exact-frame', [ping], False)]:
                label = engine + '-serve-' + label
                started = start(engine, label)
                try:
                    with socket.socket(socket.AF_UNIX) as peer:
                        peer.settimeout(5)
                        peer.connect(str(path))
                        packets = [frame(compact(r)) for r in requests]
                        if label.endswith('exact-frame'):
                            payload = compact(ping)
                            packets = [frame(payload + b' '*(2048-len(payload)))]
                        wire = b''.join(packets)
                        if split:
                            for byte in wire:
                                peer.sendall(bytes([byte]))
                        else:
                            peer.sendall(wire)
                        if len(requests) < 8:
                            peer.shutdown(socket.SHUT_WR)
                        for request in requests:
                            assert recv(peer) == compact(oracle(request)), label
                        assert peer.recv(1) == b'', label
                    finish(label, started, 0, compact(dict(served=len(requests))) + b'\n')
                finally:
                    if started[0].poll() is None:
                        started[0].kill(); started[0].communicate()

            label = engine + '-valid-then-invalid'
            started = start(engine, label)
            try:
                with socket.socket(socket.AF_UNIX) as peer:
                    peer.settimeout(5); peer.connect(str(path))
                    peer.sendall(frame(compact(ping)) + frame(b'{}'))
                    peer.shutdown(socket.SHUT_WR)
                    assert recv(peer) == compact(oracle(ping))
                    assert peer.recv(1) == b''
                finish(label, started, 2, b'{"error":"shape"}\n')
            finally:
                if started[0].poll() is None:
                    started[0].kill(); started[0].communicate()

            # EOF at EVERY boundary, including empty stream and complete frame.
            wire = frame(compact(ping))
            rejections = [(f'eof-{i}', wire[:i], None if i in (0, len(wire)) else 'truncated_header' if i < 4 else 'truncated_body') for i in range(len(wire)+1)]
            rejections += [('zero', struct.pack('!I', 0), 'empty_frame'), ('over', struct.pack('!I', 2049), 'frame_limit'), ('u32-max', b'\xff'*4, 'frame_limit'), ('utf8', frame(b'\xff'), 'utf8'), ('json', frame(b'{'), 'json'), ('shape', frame(b'{}'), 'shape')]
            for suffix, wire, error in rejections:
                label = engine + '-serve-' + suffix
                started = start(engine, label)
                try:
                    with socket.socket(socket.AF_UNIX) as peer:
                        peer.settimeout(5)
                        peer.connect(str(path)); peer.sendall(wire); peer.shutdown(socket.SHUT_WR)
                        if error is None and wire:
                            assert recv(peer) == compact(oracle(ping))
                        assert peer.recv(1) == b''
                    expected = dict(error=error) if error else dict(served=int(bool(wire)))
                    finish(label, started, 2 if error else 0, compact(expected) + b'\n')
                finally:
                    if started[0].poll() is None:
                        started[0].kill(); started[0].communicate()

            # Independent Python server: client validates framing, JSON, request
            # ID and typed result; no thread/process is needed for the peer.
            responses = [(str(i), request, frame(compact(oracle(request))), None) for i, request in enumerate(cases)]
            responses += [('no-frame', ping, b'', 'truncated_header'), ('wrong-id', ping, frame(b'{"id":"other","result":"pong"}'), 'response_mismatch'), ('bad-json', ping, frame(b'{'), 'json'), ('bad-utf8', ping, frame(b'\xff'), 'utf8'), ('short-head', ping, b'\0\0', 'truncated_header'), ('short-body', ping, b'\0\0\0\x04{}', 'truncated_body'), ('over', ping, struct.pack('!I', 2049), 'frame_limit'), ('zero', ping, b'\0'*4, 'empty_frame')]
            for suffix, request, wire, error in responses:
                label = engine + '-call-' + suffix
                with socket.socket(socket.AF_UNIX) as listener:
                    listener.settimeout(60); listener.bind(str(path)); listener.listen(1)
                    argv = command(engine, ['call', str(path), compact(request).decode()], ('unix_connect',))
                    p = subprocess.Popen(list(map(str, argv)), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, preexec_fn=limits)
                    try:
                        peer, _ = listener.accept()
                        with peer:
                            peer.settimeout(5)
                            assert recv(peer) == compact(request)
                            assert peer.recv(1) == b''
                            peer.sendall(wire)
                        out, err = p.communicate(timeout=60)
                    finally:
                        if p.poll() is None:
                            p.kill(); p.communicate()
                        path.unlink()
                executions += 1
                result = retain(label, argv, (p.returncode, out, err))
                expected = dict(error=error) if error else oracle(request)
                assert result == (2 if error else 0, compact(expected)+b'\n', b''), (label, result)

            for index, request in enumerate(cases):
                label = f'{engine}-hirsel-pair-{index}'
                started = start(engine, label)
                try:
                    result = run(label + '-client', command(engine, ['call', str(path), compact(request).decode()], ('unix_connect',)))
                    assert result == (0, compact(oracle(request))+b'\n', b''), (label, result)
                    finish(label, started, 0, b'{"served":1}\n')
                finally:
                    if started[0].poll() is None:
                        started[0].kill(); started[0].communicate()
            assert sentinel.read_bytes() == b'keep\0\xff'
            assert {p.name for p in private.iterdir()} == {'untouched'}
            assert {p.name: p.read_bytes() for p in (root / 'normalize.json', root / 'invalid.json')} == original_fixtures
    assert not private.exists(), 'private socket directory leaked'
    print(json.dumps(dict(engines=['h', 'llvm', 'cranelift'], executions=executions)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.hirsel, args.root, args.output)
