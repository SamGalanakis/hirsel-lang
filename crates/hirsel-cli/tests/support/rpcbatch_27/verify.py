#!/usr/bin/env python3
"""Literal application vectors, exact host frames, readiness and joined cleanup.

Go io.ReadFull and Unix EOF tests (BSD-3-Clause), Tokio capped header decoder and
join-set cancellation (MIT). Pins and Hirsel policy differences: app README.
No startup sleeps. Bounded peer concurrency never exceeds four.
"""
import argparse
import concurrent.futures
import hashlib
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


def frame(payload):
    return struct.pack('!I', len(payload)) + payload


def exact(peer, size):
    output = b''
    while len(output) < size:
        part = peer.recv(size - len(output))
        assert part, ('unexpected EOF', output, size)
        output += part
    return output


def receive(peer):
    length, = struct.unpack('!I', exact(peer, 4))
    assert 0 < length <= 2048
    return exact(peer, length)


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
    resource.setrlimit(resource.RLIMIT_DATA, (2 * 1024**3, 2 * 1024**3))


def verify(hirsel, selection, output):
    output.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    observed_env = {k: v for k, v in env.items() if k.startswith('HIRSEL_') or k.startswith('LLVM_') or k in ('PATH', 'LD_LIBRARY_PATH')}
    (output / 'environment.json').write_text(json.dumps(observed_env, indent=2))
    records = []
    passes = 0
    only_reverse = os.environ.get('RPCBATCH_27_ONLY_REVERSE') == '1'

    def save(label, argv, completed, expected=None):
        nonlocal passes
        code, stdout, stderr = completed
        (output / (label + '.stdout')).write_bytes(stdout)
        (output / (label + '.stderr')).write_bytes(stderr)
        record = dict(id=label, argv=list(map(str, argv)), code=code, stdout_sha256=hashlib.sha256(stdout).hexdigest())
        if expected is not None:
            record['expected'] = dict(code=expected[0], stdout=expected[1].decode(), stderr=expected[2].decode())
            record['pass'] = completed == expected
            if record['pass']:
                passes += 1
        records.append(record)
        (output / 'commands.json').write_text(json.dumps(records, indent=2))
        assert expected is None or completed == expected, (label, completed, expected)
        return completed

    def run(label, argv, expected=None, setup=False):
        p = subprocess.run(list(map(str, argv)), capture_output=True, stdin=subprocess.DEVNULL,
                           timeout=240 if setup else 60, env=env, preexec_fn=None if setup else limits)
        return save(label, argv, (p.returncode, p.stdout, p.stderr), expected)

    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        if os.environ.get('RPCBATCH_27_BINARIES'):
            binaries[backend] = Path(os.environ['RPCBATCH_27_BINARIES']) / backend
        else:
            run('build-' + backend, [hirsel, 'build', '--backend', backend, selection, '-o', exe], (0, b'', b''), True)
            binaries[backend] = exe

    vectors = json.loads((selection.parent / 'workflows.json').read_text())

    def command(engine, label, grants, record=False):
        grant = output / (label + '.grants.json')
        grant.write_text(json.dumps(grants))
        if engine == 'interpreter':
            argv = [hirsel, 'run', selection, '--max-steps', '1000000', '--grants', grant]
            if record:
                argv += ['--record', output / (label + '.tape')]
        else:
            argv = [binaries[engine]]
            if record:
                argv += ['--hirsel-record', output / (label + '.tape')]
            argv += ['--hirsel-grants', grant, *grants['args']]
        return argv

    for engine in binaries:
        for vector in ([] if only_reverse else vectors):
            if vector['disposition'] != 'RUN':
                continue
            label = engine + '-' + vector['id'].replace('/', '-')
            grants = vector['grants'] | dict(placement=dict(cores=1, commit='shared', reorder='auto'))
            argv = command(engine, label, grants, True)
            run(label, argv, (vector['code'], vector['stdout'].encode(), b''))
            # Host effects are impossible here; replay consumes the frozen tape,
            # including every provider event and task scheduling observation.
            replay = [binaries[engine], '--hirsel-replay', output / (label + '.tape')]
            run(label + '-replay', replay, (vector['code'], vector['stdout'].encode(), b''))

    # Host peers supplement the provider matrix and use one readiness barrier
    # before all four requests, then return responses in reverse input order.
    with tempfile.TemporaryDirectory(prefix='rb27-') as private:
        private = Path(private)
        path = private / 'rpc.sock'
        sentinel = private / 'keep'
        sentinel.write_bytes(b'identity protected\0')

        def grants(args, operation, width=1):
            return dict(args=args, io=dict(stdout='stdout.bytes@1', tasks='tasks.cooperative@1',
                monotonic_clock='clock.monotonic.ns@1', dirs=[], net=dict(provider='net.stream@1',
                rules=[dict(address=str(path), service='0', operation=operation)])),
                placement=dict(cores=width, commit='shared', reorder='auto'))

        for engine in ('interpreter', *binaries):
            if not only_reverse:
                label = engine + '-idle-accept'
                g = grants(['serve', str(path), '1', '100'], 'unix_listen')
                argv = command(engine, label, g, True)
                p = subprocess.Popen(list(map(str, argv)), stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, preexec_fn=limits)
                try:
                    assert select.select([p.stdout], [], [], 60)[0], label
                    ready = p.stdout.readline()
                    assert ready == b'{"listening":true}\n', (label, ready)
                    stdout, stderr = p.communicate(timeout=60)
                    save(label, argv, (p.returncode, ready + stdout, stderr), (0, b'{"listening":true}\n{"served":0,"expired":true}\n', b''))
                finally:
                    if p.poll() is None:
                        p.kill()
                        p.communicate()
                assert not path.exists(), (label, 'listener leaked')
                assert sentinel.read_bytes() == b'identity protected\0'

                # No arbitrary stale unlink: regular file bind fails and is preserved.
                path.write_bytes(b'not a socket')
                label = engine + '-identity'
                argv = command(engine, label, g)
                run(label, argv, (3, b'{"error":"io"}\n', b''))
                assert path.read_bytes() == b'not a socket'
                path.unlink()

            label = engine + '-reverse'
            requests = [dict(id=str(i), method='ping') for i in range(4)]
            expected = [dict(index=i, id=str(i), status='ok', response=dict(id=str(i), result='pong')) for i in range(4)]
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(path))
            listener.listen(4)
            listener.settimeout(30)
            seen = []
            active = []
            closed = []

            def peer():
                connections = []
                try:
                    for _ in range(4):
                        connection, _ = listener.accept()
                        connection.settimeout(30)
                        connections.append(connection)
                    # Read all literal frames first. This is the active-call barrier.
                    indexed = {}
                    for connection in connections:
                        payload = receive(connection)
                        request = json.loads(payload)
                        assert payload == compact(requests[int(request['id'])])
                        indexed[int(request['id'])] = connection
                        seen.append(int(request['id']))
                    active.append(len(indexed))
                    for i in reversed(range(4)):
                        indexed[i].sendall(frame(compact(dict(id=str(i), result='pong'))))
                        indexed[i].shutdown(socket.SHUT_WR)
                        waiter = select.poll()
                        waiter.register(indexed[i], 0)
                        events = waiter.poll(30000)
                        assert events and events[0][1] & select.POLLHUP
                        closed.append(True)
                finally:
                    for connection in connections:
                        connection.close()
                    listener.close()

            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(peer)
                g = grants(['batch', str(path), '4', '2000', *[compact(r).decode() for r in requests]], 'unix_connect')
                argv = command(engine, label, g, True)
                run(label, argv, (0, compact(expected) + b'\n', b''))
                future.result(timeout=35)
            assert sorted(seen) == list(range(4)) and active == [4] and len(closed) == 4
            path.unlink()  # Host fixture owns this listener; Hirsel never unlinks it.
            if engine in binaries:
                # Replay with the original socket absent and no peer thread.
                for width in (1, 2, 4):
                    replay_env = env | dict(HIRSEL_RT_WORKERS=str(width))
                    argv = [binaries[engine], '--hirsel-replay', output / (label + '.tape')]
                    p = subprocess.run(list(map(str, argv)), capture_output=True, timeout=60, env=replay_env, preexec_fn=limits)
                    save(label + '-replay-' + str(width), argv, (p.returncode, p.stdout, p.stderr), (0, compact(expected) + b'\n', b''))
            assert not path.exists()
    print(json.dumps(dict(passes=passes, recorded=len(records), engines=['interpreter', 'llvm', 'cranelift'], only_reverse=only_reverse, scripted_idle_accept='INCOMPLETE: exhausted script accept returns ConnectionAborted; tested with host sockets')))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--selection', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.hirsel, args.selection, args.output)
