#!/usr/bin/env python3
"""Owned loopback server, independent HTTP vectors, three-engine offline replay.

Ports Go net/http/client_test.go:63–148 and httptest/server.go:266–310,
ebe2feefc400f813aefdc8a036e66d2619da6649 (Go Authors, BSD-3-Clause;
repository third-party/NOTICES.md). Adaptations: explicit capabilities, scripted
HTTP/1.1 peers, bounded reads, and record/replay instead of Go client helpers.
"""
import argparse
import copy
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import threading
import time


# Exact-name lookup follows CPython Lib/unittest/loader.py:125-215;
# method/nonexistent-method vectors: test_loader.py:518-553. No prefix matching.
ENGINES = ('h', 'llvm', 'cranelift')
SCENARIOS = (
    'ordered-methods', 'mismatch-continues', '404-exact', 'body-cap', 'steps-cap',
    'bad-json', 'utf8', 'redirect', 'body-over', 'chunk-over', 'eof-over',
    'head-over', 'headers-over', 'truncated', 'deadline', 'transport-stops',
    'request-body-cap', 'request-budget-under', 'scenario-byte-cap',
    *('invalid-' + str(i) for i in range(29)),
    'denied', 'missing-net', 'missing-tasks', 'missing-monotonic_clock',
    'missing-stdout', 'outside-grant',
    'record-pass', 'record-assertion', 'record-transport',
)
SELECTIONS = {engine + '.' + label: (engine, label)
              for engine in ENGINES for label in SCENARIOS}
SELECTIONS['h-body-cap'] = ('h', 'body-cap')


def resolve_case(name):
    if name is None:
        return None, None
    try:
        return SELECTIONS[name]
    except KeyError:
        raise ValueError('unknown exact HTTP case: ' + name) from None


class Server(socketserver.TCPServer):
    allow_reuse_address = False

    def __init__(self, port=0):
        self.requests = []
        self.request_started = []
        self.errors = []
        self.stop = threading.Event()
        super().__init__(('127.0.0.1', port), Handler)
        self.base = f'http://127.0.0.1:{self.server_address[1]}'


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            started = time.monotonic()
            self.request.settimeout(5)
            data = b''
            while b'\r\n\r\n' not in data:
                chunk = self.request.recv(4096)
                if not chunk:
                    return
                data += chunk
                assert len(data) <= 32768, 'request head limit'
            head, body = data.split(b'\r\n\r\n', 1)
            lines = head.decode('ascii').split('\r\n')
            method, route, version = lines[0].split(' ')
            headers = [tuple(line.split(':', 1)) for line in lines[1:]]
            headers = [(k.lower(), v.strip()) for k, v in headers]
            lengths = [int(v) for k, v in headers if k == 'content-length']
            assert len(lengths) <= 1
            length = lengths[0] if lengths else 0
            assert length <= 16384
            while len(body) < length:
                chunk = self.request.recv(length - len(body))
                assert chunk, 'truncated request'
                body += chunk
            assert len(body) == length
            assert version == 'HTTP/1.1'
            assert ('host', f'127.0.0.1:{self.server.server_address[1]}') in headers
            assert not any(k == 'transfer-encoding' for k, _ in headers)
            custom = [(k, v) for k, v in headers if k in ('x-duplicate', 'content-type')]
            self.server.requests.append((method, route, body, custom))
            self.server.request_started.append(started)
            status, extra = 200, b''
            payload = b'User-agent: *\nDisallow: /secret\n'
            if route == '/json':
                payload = b'{"a/b":{"~key":[null,true,{"x":1}]},"text":"snow"}'
                extra = b'X-Duplicate: first\r\nx-duplicate: second\r\n'
            elif route == '/bad-json':
                payload = b'{broken'
            elif route == '/utf8':
                payload = b'\xff'
            elif route == '/404':
                status, payload = 404, b'missing'
            elif route == '/redirect':
                status, extra = 302, b'Location: http://127.0.0.1:1/forbidden\r\n'
            elif route == '/body-cap':
                payload = b'a' * 65536
            elif route in ('/body-over', '/chunk-over', '/eof-over'):
                payload = b'a' * 65537
            elif route == '/head-over':
                extra = b'X-Large: ' + b'a' * 17000 + b'\r\n'
            elif route == '/headers-over':
                extra = b'X-H: v\r\n' * 65
            elif route == '/deadline':
                # A trickle fits each read timeout; the whole exchange must expire.
                self.request.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n')
                for _ in range(40):
                    if self.server.stop.wait(0.25):
                        return
                    self.request.sendall(b'a')
                return
            elif route not in ('/', '/head', '/post', '/put', '/patch', '/delete', '/options', '/truncated'):
                raise AssertionError('unexpected route: ' + route)
            if method == 'HEAD':
                extra += b'Last-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\n'
            response = f'HTTP/1.1 {status} Fixture\r\n'.encode() + extra
            if route == '/chunk-over':
                response += b'Transfer-Encoding: chunked\r\n'
                payload = f'{len(payload):x}\r\n'.encode() + payload + b'\r\n0\r\n\r\n'
            elif route != '/eof-over':
                response += f'Content-Length: {len(payload) + (10 if route == "/truncated" else 0)}\r\n'.encode()
            response += b'Connection: close\r\n\r\n'
            self.request.sendall(response + (b'' if method == 'HEAD' else payload))
        except (BrokenPipeError, ConnectionResetError):
            pass  # The bounded client intentionally closes on error/deadline.
        except Exception as error:
            self.server.errors.append(repr(error))


def deny_network():
    """libseccomp's allow-default/errno-rule pattern; inherited across exec.

    libseccomp doc/man/man3/seccomp_rule_add.3 and seccomp_load.3. Resolve
    syscall numbers by name so the same policy works on both Linux hosts.
    Called only after the server thread has joined; no live Python threads.
    """
    library = ctypes.CDLL('libseccomp.so.2', use_errno=True)
    library.seccomp_init.argtypes = [ctypes.c_uint32]
    library.seccomp_init.restype = ctypes.c_void_p
    library.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
    library.seccomp_syscall_resolve_name.restype = ctypes.c_int
    library.seccomp_rule_add.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int, ctypes.c_uint]
    library.seccomp_load.argtypes = [ctypes.c_void_p]
    library.seccomp_release.argtypes = [ctypes.c_void_p]
    context = library.seccomp_init(0x7fff0000)  # SCMP_ACT_ALLOW
    if not context:
        os._exit(97)
    for name in (b'socket', b'connect'):
        number = library.seccomp_syscall_resolve_name(name)
        if number < 0 or library.seccomp_rule_add(context, 0x50000 | errno.EPERM, number, 0) != 0:
            os._exit(97)
    if library.seccomp_load(context) != 0:
        os._exit(97)
    library.seccomp_release(context)
    try:
        probe = socket.socket()
    except PermissionError:
        return
    probe.close()
    os._exit(97)


def step(base, method='GET', route='/', **expected):
    return {'method': method, 'url': base + route, 'headers': [],
            'expect': {'status': 200, **expected}}


def rendered(rows, passed=0, failed=0, errors=0, skipped=0):
    # Independent expected wire format. Never obtain goldens from the program.
    values = [{'step': i, 'result': state, 'failures': failures}
              for i, (state, failures) in enumerate(rows)]
    values.append({'summary': {'passed': passed, 'failed': failed, 'errors': errors, 'skipped': skipped}})
    return b''.join((json.dumps(v, separators=(',', ':')) + '\n').encode() for v in values)


def verify(hirsel, root, output, case=None):
    selected_engine, selected_label = resolve_case(case)
    record_phase = selected_label is not None and selected_label.startswith('record-')
    active_engines = ENGINES if case is None else (selected_engine,)
    needed_binaries = ('llvm', 'cranelift') if case is None or record_phase else tuple(
        engine for engine in active_engines if engine != 'h')
    output.mkdir(parents=True, exist_ok=False)
    commands, tapes = [], []
    counts = {'h': 0, 'llvm': 0, 'cranelift': 0}
    builds = {'llvm': 0, 'cranelift': 0}
    result = {'state': 'setup', 'observations': 0, 'records': 0, 'replays': 0,
              'executed': counts, 'builds': builds, 'case': case,
              'cells': {engine + '.' + label: 'unrun' for engine in active_engines
                        for label in SCENARIOS if selected_label is None or label == selected_label}}

    for source in active_engines:
        for label in ('pass', 'assertion', 'transport'):
            if selected_label is None or selected_label == 'record-' + label:
                for target in ENGINES:
                    result['cells'][target + '.replay-' + source + '-' + label] = 'unrun'

    def save():
        (output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')

    def run(label, argv, offline=False):
        entry = {'label': label, 'argv': list(map(str, argv)), 'state': 'running', 'network_disabled': offline}
        commands.append(entry)
        save()
        start = time.monotonic()
        try:
            done = subprocess.run(entry['argv'], capture_output=True,
                                  stdin=subprocess.DEVNULL, timeout=180,
                                  preexec_fn=deny_network if offline else None)
            entry.update(state='finished', exit=done.returncode, seconds=time.monotonic() - start)
            stdout, stderr = done.stdout, done.stderr
        except subprocess.TimeoutExpired as error:
            entry.update(state='timeout', seconds=time.monotonic() - start)
            stdout, stderr = error.stdout or b'', error.stderr or b''
            raise
        finally:
            if 'stdout' in locals():
                (output / (label + '.stdout')).write_bytes(stdout)
                (output / (label + '.stderr')).write_bytes(stderr)
            save()
        return done.returncode, stdout, stderr

    save()
    server = Server()
    thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.02}, daemon=True)
    thread.start()
    # Refuse occupied ports; never terminate or replace the incumbent listener.
    try:
        probe = Server(server.server_address[1])
    except OSError:
        pass
    else:
        probe.server_close()
        raise AssertionError('occupied port was accepted')
    grants = output / 'grants.json'
    policy = {'io': {'stdout': 'stdout.bytes@1', 'tasks': 'tasks.cooperative@1',
        'monotonic_clock': 'clock.monotonic.ns@1', 'dirs': [],
        'net': {'provider': 'net.stream@1', 'rules': [{'address': '127.0.0.1',
            'service': str(server.server_address[1]), 'operation': 'connect'}]}}, 'args': []}
    grants.write_text(json.dumps(policy))
    denied = output / 'denied.json'
    denied_policy = copy.deepcopy(policy)
    denied_policy['io']['net']['rules'] = []
    denied.write_text(json.dumps(denied_policy))
    base, manifest = server.base, root / 'hirsel.json'
    binaries = {b: output / ('httpcheck-' + b) for b in needed_binaries}

    def selected(engine, label):
        return case is None or (engine, label) == (selected_engine, selected_label)

    def command(engine, raw=None, tape=None, replay=False, authority=None):
        if engine == 'h':
            cmd = [hirsel, 'run', manifest, '--max-steps', 'unbounded']
            if replay:
                return cmd + ['--replay', tape]
            return cmd + ['--grants', authority or grants, *(['--record', tape] if tape else []),
                          '--', *([] if raw is None else [raw])]
        cmd = [binaries[engine]]
        if replay:
            return cmd + ['--hirsel-replay', tape]
        return cmd + [*(['--hirsel-record', tape] if tape else []),
                      '--hirsel-grants', authority or grants, *([] if raw is None else [raw])]

    def expect(label, engine, scenario, triple, requests, tape=None, authority=None):
        raw = scenario if isinstance(scenario, str) else json.dumps(scenario, separators=(',', ':'))
        (output / (label + '.scenario.json')).write_text(raw)
        cell = engine + '.' + label.removeprefix(engine + '-')
        result['cells'][cell] = 'running'
        save()
        before = len(server.requests)
        actual = run(label, command(engine, raw, tape, authority=authority))
        counts[engine] += 1
        assert actual == triple, (label, actual, triple, str(output))
        assert server.requests[before:] == requests, (label, server.requests[before:], requests)
        assert not server.errors, server.errors
        if label.endswith('-deadline'):
            # CLI startup includes compilation. The transport deadline starts
            # at this exchange, witnessed by the fixture accepting its socket.
            request_seconds = time.monotonic() - server.request_started[before]
            commands[-1]['request_seconds'] = request_seconds
            assert request_seconds < 8, ('whole-request deadline did not bound trickle', commands[-1])
        result['cells'][cell] = 'passed'
        result['observations'] += 1
        save()

    def request(verb='GET', route='/', body=b'', headers=None):
        return verb, route, body, headers or []

    transport = (2, rendered([('transport_error', [])], errors=1), b'')
    invalid = (2, b'{"result":"scenario_error"}\n{"summary":{"passed":0,"failed":0,"errors":1,"skipped":0}}\n', b'')
    try:
        for backend, binary in binaries.items():
            actual = run('build-' + backend, [hirsel, 'build', manifest, '--backend', backend, '-o', binary])
            assert actual[0] == 0, ('build', backend, actual, str(output))
            builds[backend] += 1
        result['binary_hashes'] = {b: hashlib.sha256(p.read_bytes()).hexdigest() for b, p in binaries.items()}
        frozen_binaries = dict(result['binary_hashes'])
        result['state'] = 'running'
        good = [step(base, contains='User-agent:'),
                step(base, 'HEAD', '/head', contains='', headers=[{'name': 'LAST-modified', 'value': 'Wed, 21 Oct 2015 07:28:00 GMT'}]),
                step(base, 'POST', '/post'), step(base, 'PUT', '/put'),
                step(base, 'PATCH', '/patch'), step(base, 'DELETE', '/delete'),
                step(base, 'OPTIONS', '/options'),
                step(base, route='/json', headers=[{'name': 'X-DUPLICATE', 'value': 'second'}, {'name': 'x-duplicate', 'value': 'first'}],
                     json=[{'pointer': '/a~1b/~0key/2', 'equals': {'x': 1}}, {'pointer': '/a~1b/~0key/0', 'equals': None},
                           {'pointer': '', 'equals': {'a/b': {'~key': [None, True, {'x': 1}]}, 'text': 'snow'}}])]
        good[2]['body'] = '{"key":"value"}'  # Go TestPostRequestFormat vector.
        good[2]['headers'] = [{'name': 'Content-Type', 'value': 'application/json'},
                              {'name': 'X-Duplicate', 'value': 'a'}, {'name': 'x-duplicate', 'value': 'b'}]
        good[3]['body'], good[4]['body'] = '', 'patch'
        good_requests = [request(), request('HEAD', '/head'),
            request('POST', '/post', b'{"key":"value"}', [('content-type', 'application/json'), ('x-duplicate', 'a'), ('x-duplicate', 'b')]),
            request('PUT', '/put'), request('PATCH', '/patch', b'patch'),
            request('DELETE', '/delete'), request('OPTIONS', '/options'), request(route='/json')]
        positive = (0, rendered([('pass', [])] * len(good), passed=len(good)), b'')
        mismatches = [step(base, route='/404', status=200), step(base, contains='absent'),
            step(base, route='/json', headers=[{'name': 'X-Duplicate', 'value': 'FIRST'}],
                 json=[{'pointer': '/missing', 'equals': None}, {'pointer': '/text', 'equals': 'wrong'}, {'pointer': '/a~1b/~0key/2/x', 'equals': 1.0}, {'pointer': '', 'equals': {'text': 'snow', 'a/b': {'~key': [None, True, {'x': 1}]}}}]), step(base)]
        negative = (1, rendered([('assertion_failure', ['status']), ('assertion_failure', ['contains']), ('assertion_failure', ['header', 'json', 'json', 'json', 'json']), ('pass', [])], passed=1, failed=3), b'error: Assertion\n')
        cases = [('ordered-methods', {'steps': good}, positive, good_requests),
                 ('mismatch-continues', {'steps': mismatches}, negative,
                  [request(route='/404'), request(), request(route='/json'), request()]),
                 ('404-exact', {'steps': [step(base, route='/404', status=404)]},
                  (0, rendered([('pass', [])], passed=1), b''), [request(route='/404')]),
                 ('body-cap', {'steps': [step(base, route='/body-cap', contains='aaa')]},
                  (0, rendered([('pass', [])], passed=1), b''), [request(route='/body-cap')]),
                 ('steps-cap', {'steps': [step(base)] * 16},
                  (0, rendered([('pass', [])] * 16, passed=16), b''), [request()] * 16)]
        for route in ('bad-json', 'utf8', 'redirect', 'body-over', 'chunk-over', 'eof-over', 'head-over', 'headers-over', 'truncated', 'deadline'):
            predicates = {'json': [{'pointer': '', 'equals': None}]} if route == 'bad-json' else {'contains': 'a'}
            cases.append((route, {'steps': [step(base, route='/' + route, **predicates)]}, transport, [request(route='/' + route)]))
        stopped = {'steps': [step(base), step(base, route='/truncated'), step(base)]}
        stopped_triple = (2, rendered([('pass', []), ('transport_error', []), ('skipped', [])], passed=1, errors=1, skipped=1), b'')
        cases.append(('transport-stops', stopped, stopped_triple, [request(), request(route='/truncated')]))
        malformed = ['{', '{"steps":[],"steps":[]}', {'steps': []}, {'steps': [step(base)] * 17},
                     {'steps': [step(base)], 'unknown': True}, {'steps': [step(base), {}]}]
        for path, value in [('method', 'TRACE'), ('url', 'not-a-url'), ('url', 'ftp://127.0.0.1/'),
                            ('url', base + '/a b'), ('url', 'http://u:secret@127.0.0.1/'),
                            ('url', base + '/#fragment'), ('headers', [{'name': 'x', 'value': 'bad\r\nInjected: yes'}]),
                            ('headers', [{'name': 'Host', 'value': 'other'}]), ('headers', [{'name': 'Content-Length', 'value': '0'}]),
                            ('body', 'a' * 16385), ('body', 7), ('unexpected', True), ('expect', {'status': 600}),
                            ('expect', {'status': 200, 'json': [{'pointer': '/~2', 'equals': 1}]}),
                            ('expect', {'status': 200, 'regex': '.*'}), ('expect', {'status': 200, 'headers': {}})]:
            bad = step(base)
            bad[path] = value
            malformed.append({'steps': [step(base), bad]})  # Later invalid step => zero traffic.
        malformed.append(' ' * 65537)
        # Independent boundaries for the request and scenario budgets.
        request_cap = step(base, 'POST', '/post')
        request_cap['body'] = 'a' * 16384
        cases.append(('request-body-cap', {'steps': [request_cap]},
                      (0, rendered([('pass', [])], passed=1), b''),
                      [request('POST', '/post', b'a' * 16384)]))
        header_cap = step(base)
        header_cap['headers'] = [{'name': 'X-Budget', 'value': 'a' * 4096}] * 7
        cases.append(('request-budget-under', {'steps': [header_cap]},
                      (0, rendered([('pass', [])], passed=1), b''), [request()]))
        header_over = copy.deepcopy(header_cap)
        header_over['headers'].append({'name': 'X-Budget', 'value': 'a' * 4096})
        malformed.append({'steps': [step(base), header_over]})
        raw_cap = json.dumps({'steps': [step(base)]}, separators=(',', ':'))
        raw_cap += ' ' * (65536 - len(raw_cap.encode()))
        cases.append(('scenario-byte-cap', raw_cap,
                      (0, rendered([('pass', [])], passed=1), b''), [request()]))
        malformed.extend([{'steps': [dict(step(base), headers=[{'name': 'X', 'value': ''}] * 33)]},
                          {'steps': [dict(step(base), expect={'status': 200, 'json': [{'pointer': '', 'equals': None}] * 33})]},
                          {'steps': [dict(step(base), method='HEAD', body='')]},
                          {'steps': [dict(step(base), headers=[{'name': '', 'value': 'x'}])]},
                          {'steps': [dict(step(base), expect={'status': '200'})]}])
        missing = {}
        for capability in ('net', 'tasks', 'monotonic_clock', 'stdout'):
            missing_policy = copy.deepcopy(policy)
            del missing_policy['io'][capability]
            missing[capability] = output / ('missing-' + capability + '.json')
            missing[capability].write_text(json.dumps(missing_policy))
        for engine in active_engines:
            for label, scenario, triple, requests in cases:
                if selected(engine, label):
                    expect(engine + '-' + label, engine, scenario, triple, requests)
            for i, scenario in enumerate(malformed):
                if selected(engine, 'invalid-' + str(i)):
                    expect(engine + '-invalid-' + str(i), engine, scenario, invalid, [])
            if selected(engine, 'denied'):
                expect(engine + '-denied', engine, {'steps': [step(base)]}, transport, [], authority=denied)
            for capability, no_grant in missing.items():
                triple = (1, b'', b'error: Authority\n') if capability == 'stdout' else transport
                if selected(engine, 'missing-' + capability):
                    expect(engine + '-missing-' + capability, engine, {'steps': [step(base)]}, triple, [], authority=no_grant)
            if selected(engine, 'outside-grant'):
                expect(engine + '-outside-grant', engine, {'steps': [step('http://127.0.0.1:1')]}, transport, [])
            for label, scenario, triple, requests in [('pass', {'steps': good}, positive, good_requests),
                    ('assertion', {'steps': mismatches}, negative, [request(route='/404'), request(), request(route='/json'), request()]),
                    ('transport', stopped, stopped_triple, [request(), request(route='/truncated')])]:
                if not selected(engine, 'record-' + label):
                    continue
                tape = output / (engine + '-' + label + '.tape')
                expect(engine + '-record-' + label, engine, scenario, triple, requests, tape)
                tapes.append((tape, triple))
                result['cells'][engine + '.record-' + label] = 'recorded'
                for replay_engine in ENGINES:
                    result['cells'][replay_engine + '.replay-' + tape.stem] = 'unrun'
                result['records'] += 1
        # Stop and close only this owned server; seccomp disables networking
        # in every replay child, independent of the tape's admitted grants.
        server.stop.set()
        server.shutdown()
        server.server_close()
        thread.join(10)
        assert not thread.is_alive()
        tape_hashes = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p, _ in tapes}
        result['tape_hashes'] = tape_hashes
        grants.rename(output / 'grants.offline.json')
        denied.unlink()
        for no_grant in missing.values():
            no_grant.unlink()
        for tape, triple in tapes:
            for engine in counts:
                cell = engine + '.replay-' + tape.stem
                result['cells'][cell] = 'running'
                actual = run(engine + '-replay-' + tape.stem, command(engine, tape=tape, replay=True), offline=True)
                assert actual == triple, (engine, tape, actual, triple)
                result['cells'][cell] = 'passed'
                result['replays'] += 1
                save()
        assert not server.errors, server.errors
        expected_records = 9 if case is None else int(record_phase)
        assert result['records'] == expected_records, result
        assert result['replays'] == expected_records * 3, result
        expected_counts = {engine: (57 if case is None else int(engine == selected_engine))
                           for engine in ENGINES}
        assert counts == expected_counts, result
        assert result['observations'] == (171 if case is None else 1), result
        assert frozen_binaries == {b: hashlib.sha256(p.read_bytes()).hexdigest() for b, p in binaries.items()}
        assert tape_hashes == {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p, _ in tapes}
        for cell, state in result['cells'].items():
            if state == 'recorded':
                result['cells'][cell] = 'passed'
        assert all(state == 'passed' for state in result['cells'].values()), result
        result['state'] = 'passed'
        result['requests'] = len(server.requests)
        result['engines'] = {engine: 1 for engine, count in counts.items() if count > 0}
        save()
        if case == 'h-body-cap':
            print("case=h-body-cap engines={'h': 1} observations=1", flush=True)
        print(f"case={case} engines={result['engines']} executed={counts} builds={builds} observations={result['observations']} records={result['records']} replays={result['replays']}")
    except BaseException as error:
        for cell, state in result['cells'].items():
            if state == 'running':
                result['cells'][cell] = 'failed'
        result.update(state='failed', error=repr(error))
        save()
        print(json.dumps(result, sort_keys=True), flush=True)
        raise
    finally:
        server.stop.set()
        server.shutdown()
        server.server_close()
        thread.join(10)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--case', choices=tuple(SELECTIONS))
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(), args.case)
