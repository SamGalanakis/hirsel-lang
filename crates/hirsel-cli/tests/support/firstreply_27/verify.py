#!/usr/bin/env python3
"""Bounded independent Unix peers; no scheduler/engine implementation.

Framing: Go io.ReadFull and binary.Read (ebe2feef), Tokio length_delimited
(ce685ee6). Selection shapes: Tokio macros_select.rs:93-146 and
JoinSet::shutdown:381-384. Hirsel selects completed errors and uses FIFO.
"""
import argparse
import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import threading


def compact(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def environment():
    return {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}


def expected(result=None, error=None, endpoint=None):
    row = {} if endpoint is None else dict(endpoint=endpoint)
    row.update(response=result) if result is not None else row.update(error=error)
    return dict(status=0 if result is not None else 1,
                stdout=compact(row)+'\n', stderr='')


def run(argv, root, label, wanted, env):
    cell = root / label
    cell.mkdir()
    (cell/'expected.json').write_text(compact(wanted)+'\n')
    (cell/'argv.json').write_text(compact([str(a) for a in argv])+'\n')
    try:
        output = subprocess.run(argv, env=env, capture_output=True, timeout=60)
        result = dict(status=output.returncode,
                      stdout=output.stdout.decode(errors='replace'),
                      stderr=output.stderr.decode(errors='replace'))
    except subprocess.TimeoutExpired as error:
        result = dict(status='TIMEOUT', stdout=(error.stdout or b'').decode(errors='replace'),
                      stderr=(error.stderr or b'').decode(errors='replace'))
    (cell/'actual.json').write_text(compact(result)+'\n')
    return result == wanted, result


def exact_read(stream, count):
    result = b''
    while len(result) < count:
        part = stream.recv(count-len(result))
        if not part:
            raise EOFError('short frame')
        result += part
    return result


class Peer:
    def __init__(self, path, payload):
        self.path, self.payload = path, payload
        self.ready, self.release = threading.Event(), threading.Event()
        self.events = []
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.settimeout(10)
        self.listener.bind(str(path))
        self.listener.listen(1)  # bound/listening before the client starts
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        try:
            with self.listener.accept()[0] as stream:
                stream.settimeout(10)
                length = struct.unpack('!I', exact_read(stream, 4))[0]
                assert 0 < length <= 2048
                request = json.loads(exact_read(stream, length))
                assert request['method'] in ('ping', 'normalize')
                self.events.append(dict(request=request))
                self.ready.set()
                assert self.release.wait(10), 'release handshake missing'
                if self.payload is not None:
                    stream.sendall(self.payload)
                    self.events.append('response-issued')
                # EOF after result proves every opened client stream was closed.
                assert stream.recv(1) == b''
                self.events.append('client-eof')
        except Exception as error:
            self.events.append(dict(error=repr(error)))
            self.ready.set()
        finally:
            self.listener.close()

    def finish(self):
        self.release.set()
        self.thread.join(12)
        if self.thread.is_alive():
            self.events.append('thread-timeout')
        self.path.unlink(missing_ok=True)


def real(hirsel, executable, manifest, root, label, deadline, env, width=1, control=False,
         malformed=False, normalize=False):
    cell = root / label
    cell.mkdir()
    request = dict(id='a', method='normalize', text='  HELLO  ') if normalize else dict(id='a', method='ping')
    reply = compact(dict(id='a', result='hello' if normalize else 'pong')).encode()
    payload = b'\0\0\0\0' if malformed else struct.pack('!I', len(reply))+reply
    peers = [Peer(cell/f'{i}.sock', payload if i == 0 and not deadline else None) for i in range(2)]
    wanted = expected(error='timeout') if deadline else expected(error='empty_frame', endpoint=0) if malformed else expected(result=json.loads(reply), endpoint=0)
    (cell/'expected.json').write_text(compact(wanted)+'\n')
    grants = dict(args=['1000', compact(request)]+[str(p.path) for p in peers],
                  io=dict(stdout='stdout.bytes@1', tasks='tasks.cooperative@1',
                          monotonic_clock='clock.monotonic.ns@1', dirs=[],
                          net=dict(provider='net.stream@1', rules=[dict(address=str(p.path),service='0',operation='unix_connect') for p in peers])))
    if control:
        grants['placement'] = dict(cores=width, commit='shared', reorder='auto',
                                   control=str(cell/'control'), metrics=str(cell/'metrics'))
    grant_path = cell/'grants.json'
    grant_path.write_text(compact(grants)+'\n')
    tape = cell/'run.tape'
    argv = [str(executable), '--hirsel-record', str(tape), '--hirsel-grants', str(grant_path), *grants['args']] if executable else [str(hirsel), 'run', str(manifest), '--grants', str(grant_path), '--record', str(tape)]
    (cell/'argv.json').write_text(compact(argv)+'\n')
    try:
        child = subprocess.Popen(argv, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        handshakes = [p.ready.wait(15) for p in peers]
        (cell/'handshakes.json').write_text(compact(handshakes)+'\n')
        if control and all(handshakes):
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(5)
                connection.connect(str(cell/'control'))
                connection.sendall(b'{"metrics":true}\n')
                response = b''
                while b'\n' not in response:
                    part = connection.recv(65536)
                    if not part:
                        break
                    response += part
                (cell/'control.json').write_bytes(response)
        peers[0].release.set()
        # Pending peers are released to wait for EOF, never to manufacture a response.
        peers[1].release.set()
        stdout, stderr = child.communicate(timeout=15)
        actual = dict(status=child.returncode, stdout=stdout.decode(errors='replace'), stderr=stderr.decode(errors='replace'))
    except Exception as error:
        actual = dict(error=repr(error))
        if 'child' in locals() and child.poll() is None:
            child.kill()
            child.communicate()
    finally:
        for peer in peers:
            peer.finish()
    (cell/'actual.json').write_text(compact(actual)+'\n')
    (cell/'peers.json').write_text(compact([p.events for p in peers])+'\n')
    clean = all('client-eof' in p.events for p in peers)
    return actual == wanted and clean, actual, wanted, tape


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--production', action='store_true')
    parser.add_argument('--controls-only', action='store_true')
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True)
    env = environment()
    (root/'environment.json').write_text(compact({k:v for k,v in env.items() if k.startswith('HIRSEL_')})+'\n')
    rows = json.loads((args.manifest.parent/'fixtures'/('controls.json' if args.controls_only else 'workflows.json')).read_text())
    report = dict(scripted_completed=0, real_completed=0, failures=[], unrun=[])
    executables = {}
    for backend in ('llvm', 'cranelift'):
        executable = root/backend
        argv = [str(args.hirsel), 'build', '--backend', backend, str(args.manifest), '-o', str(executable)]
        ok, result = run(argv, root, f'build-{backend}', dict(status=0, stdout='', stderr=''), env)
        if not ok:
            report['failures'].append(dict(id=f'build-{backend}', actual=result))
            report['unrun'] += [f'{backend}/{r["id"]}' for r in rows]
            continue
        executables[backend] = executable
        (root/f'{backend}.sha256').write_text(digest(executable)+'\n')
        for width in ((1,2,4) if args.production else (1,)):
            for row in rows:
                label = f'{backend}-{width}-{row["id"]}'
                grants = json.loads(compact(row['grants']))
                grants['placement'] = dict(cores=width, commit='shared', reorder='auto')
                path = root/f'{label}.grants.json'
                path.write_text(compact(grants)+'\n')
                tape = root/f'{label}.tape'
                trace = root/f'{label}.jsonl'
                argv = [str(executable), '--hirsel-record', str(tape), '--hirsel-grants', str(path), *grants['args']]
                ok, result = run(argv, root, label, row['expected'], env)
                report['scripted_completed'] += 1
                if not ok:
                    report['failures'].append(dict(id=label, actual=result))
                    continue
                original = digest(tape)
                # Effects are replayed with no Unix sockets available. These
                # production tapes use a live clock even with scripted Net.
                replay_argv = [str(args.hirsel), 'run', str(args.manifest), '--replay', str(tape)]
                ok, result = run(replay_argv, root, label+'-replay-interpreter', row['expected'], env)
                if not ok:
                    report['failures'].append(dict(id=label+'-replay-interpreter', actual=result))
                assert digest(tape) == original
    for engine, executable in executables.items():
        if args.controls_only:
            continue
        for width in ((1,2,4) if args.production and executable else (1,)):
            scenarios = ('winner', 'deadline', 'malformed') if args.production and executable else ('winner','deadline')
            for scenario in scenarios:
                label = f'{engine}-{width}-real-{scenario}'
                ok, actual, wanted, tape = real(args.hirsel, executable, args.manifest, root, label,
                    scenario == 'deadline', env, width, args.production and executable is not None,
                    scenario == 'malformed', normalize=scenario == 'winner')
                report['real_completed'] += 1
                if not ok:
                    report['failures'].append(dict(id=label, actual=actual))
                    continue
                ok, result = run([str(args.hirsel), 'run', str(args.manifest), '--replay', str(tape)],
                                 root, label+'-replay-interpreter', wanted, env)
                if not ok:
                    report['failures'].append(dict(id=label+'-replay-interpreter', actual=result))
    # A remote action's /tmp is ephemeral. Keep exact small tapes/receipts in
    # its returned log; executables are identified by checksum, never bundled.
    artifacts = {str(p.relative_to(root)): base64.b64encode(p.read_bytes()).decode()
                 for p in root.rglob('*') if p.is_file()
                 and p.suffix in ('.json', '.jsonl', '.tape', '.sha256')}
    report['artifacts_gzip_base64'] = base64.b64encode(gzip.compress(compact(artifacts).encode(), mtime=0)).decode()
    (root/'report.json').write_text(compact(report)+'\n')
    print(compact(report))
    return 1 if report['failures'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
