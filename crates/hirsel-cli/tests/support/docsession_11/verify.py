#!/usr/bin/env python3
"""Independent byte goldens; incremental stdin with response deadlines.

Go scan_test.go:175–213 (ebe2feef, BSD-3-Clause) supplies fragmented/boundary
line vectors. serde_json tests/test.rs:1830–1866 (20572a6d, MIT/Apache-2.0)
supplies RFC6901 expected values. Adapted to strict pointer grammar and bounded
Hirsel ownership APIs. Licenses are in this directory. No timing/parallel claim.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(hirsel, root, output, reuse=None):
    output.mkdir(parents=True, exist_ok=False)
    manifest = root / 'hirsel.json'
    grants = output / 'grants.json'
    grants.write_text(json.dumps({'io': {'stdin': 'stdin.bytes@1', 'stdout': 'stdout.bytes@1', 'dirs': []}, 'args': []}))
    frozen_paths = [hirsel, Path(__file__)] + [p for p in root.rglob('*') if p.is_file()]
    std = (root / json.loads(manifest.read_text())['dependencies'][0]['manifest']).resolve().parent
    frozen_paths += [p for p in std.rglob('*') if p.is_file()]
    for key in ('HIRSEL_PRODUCTS', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX'):
        if os.environ.get(key):
            p = Path(os.environ[key])
            frozen_paths += [q for q in p.rglob('*') if q.is_file()] if p.is_dir() else [p]
    frozen = {str(p): sha(p) for p in frozen_paths}
    (output / 'frozen.json').write_text(json.dumps(frozen, indent=2))
    commands, binaries = [], {}
    result = {'status': 'running', 'observations': 0, 'interactive_commands': 0,
              'recordings': 0, 'replays': 0, 'builds': {'llvm': 0, 'cranelift': 0}}

    def save():
        (output / 'commands.json').write_text(json.dumps(commands, indent=2))
        (output / 'result.json').write_text(json.dumps(result, indent=2))

    def run(label, argv, data=b'', expected=None):
        argv = list(map(str, argv))
        done = subprocess.run(argv, input=data, capture_output=True, timeout=300)
        (output / (label + '.stdin')).write_bytes(data)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        actual = done.returncode, done.stdout, done.stderr
        commands.append({'label': label, 'argv': argv, 'exit': done.returncode})
        save()
        if expected is not None:
            assert actual == expected, (label, actual, expected)
            result['observations'] += 1
        return actual

    def cmd(engine, record=None, replay=None):
        if engine == 'h':
            return [hirsel, 'run', manifest, '--max-steps', 'unbounded', *(['--replay', replay] if replay else
                    ['--grants', grants, *(['--record', record] if record else [])])]
        return [binaries[engine], *(['--hirsel-replay', replay] if replay else
                [*(['--hirsel-record', record] if record else []), '--hirsel-grants', grants])]

    def interactive(label, engine, lines, expected, fragment=False, eof=False):
        """No next command is written until the previous exact response arrives."""
        argv = list(map(str, cmd(engine)))
        proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, bufsize=0)
        received, sent = bytearray(), bytearray()
        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ)
        try:
            for index, (line, golden) in enumerate(zip(lines, expected, strict=True)):
                chunks = [line[i:i + 1] for i in range(len(line))] if fragment else [line]
                for chunk in chunks:
                    proc.stdin.write(chunk)
                    sent.extend(chunk)
                deadline, response = time.monotonic() + 30, bytearray()
                while not response.endswith(b'\n'):
                    assert selector.select(max(0, deadline - time.monotonic())), (label, index, 'response-before-next-input timeout')
                    chunk = os.read(proc.stdout.fileno(), 1)
                    assert chunk, (label, index, 'EOF before response', proc.stderr.read())
                    response.extend(chunk)
                    assert time.monotonic() < deadline, (label, index, 'response deadline')
                received.extend(response)
                assert bytes(response) == golden, (label, index, response, golden)
                result['interactive_commands'] += 1
            # With eof=True the final command is terminated, but transaction is
            # still open. Otherwise the final command is QUIT, stdin stays open
            # until wait() proves the program exits without asking for EOF.
            if eof:
                proc.stdin.close()
            assert proc.wait(timeout=30) == 0, (label, proc.stderr.read())
            assert proc.stdout.read() == b'', label
            assert proc.stderr.read() == b'', label
            result['observations'] += 1
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            selector.close()
            proc.stdin.close()
            (output / (label + '.stdin')).write_bytes(sent)
            (output / (label + '.stdout')).write_bytes(received)
            commands.append({'label': label, 'argv': argv, 'exit': proc.returncode,
                             'response_before_next_input': True, 'fragment_bytes': 1 if fragment else None})
            save()

    def io_faults(engine):
        # Linux directory fd forces a real stdin read error. Host faults use
        # normalized code 1 rather than leaking errno. Closing
        # the output pipe after a valid response forces EPIPE on the next write.
        argv = list(map(str, cmd(engine)))
        fd = os.open(output, os.O_RDONLY | os.O_DIRECTORY)
        try:
            done = subprocess.run(argv, stdin=fd, capture_output=True, timeout=30)
        finally:
            os.close(fd)
        (output / (engine + '-read-fault.stdout')).write_bytes(done.stdout)
        (output / (engine + '-read-fault.stderr')).write_bytes(done.stderr)
        commands.append({'label': engine + '-read-fault', 'argv': argv, 'exit': done.returncode,
                         'stdout_hex': done.stdout.hex(), 'stderr_hex': done.stderr.hex()})
        save()
        assert (done.returncode, done.stdout, done.stderr) == (1, b'', b'error: Input(Source(InputFault(1)))\n'), (engine, done.returncode, done.stdout, done.stderr)
        result['observations'] += 1
        proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ)
        try:
            proc.stdin.write(b'SHOW\n')
            assert selector.select(30), (engine, 'write-fault prefix timeout')
            prefix = proc.stdout.readline()
            assert prefix == b'{"ok":"value","value":null}\n'
            proc.stdout.close()
            proc.stdin.write(b'SHOW\n')
            code = proc.wait(timeout=30)
            error = proc.stderr.read()
            (output / (engine + '-write-fault.stdout')).write_bytes(prefix)
            (output / (engine + '-write-fault.stderr')).write_bytes(error)
            commands.append({'label': engine + '-write-fault', 'argv': argv, 'exit': code,
                             'stdout_hex': prefix.hex(), 'stderr_hex': error.hex()})
            save()
            assert (code, error) == (1, b'error: Output(OutputFault(1))\n'), (engine, code, error)
            result['observations'] += 1
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            selector.close()
            proc.stdin.close()

    ok = lambda arm: ('{"ok":"' + arm + '"}\n').encode()
    err = lambda arm: ('{"error":"' + arm + '"}\n').encode()
    value = lambda raw: b'{"ok":"value","value":' + raw + b'}\n'
    null = value(b'null')
    fail = lambda arm, prefix=b'': (1, prefix, ('error: ' + arm + '\n').encode())
    try:
        if reuse:
            previous = json.loads((reuse / 'frozen.json').read_text())
            # Reuse only the same compiler, std/runtime/toolchain, and exact
            # app source/manifest bytes. The checker itself may be corrected.
            assert all(sha(Path(p)) == h for p, h in previous.items())
            old_root = reuse.parent / 'package'
            for name in ('main.hir', 'model.hir', 'hirsel.json'):
                assert sha(root / name) == sha(old_root / name), name
            assert all(previous[p] == h for p, h in frozen.items() if p in previous)
            for engine in ('h', 'llvm', 'cranelift'):
                assert b'2 passed' in (reuse / ('package-' + engine + '.stdout')).read_bytes()
            identities = json.loads((reuse / 'binaries.json').read_text())
            for backend in ('llvm', 'cranelift'):
                exe = reuse / ('docsession-' + backend)
                assert sha(exe) == identities[str(exe)]
                binaries[backend] = exe
                result['builds'][backend] = 1
            result['reused_build_and_package_evidence'] = str(reuse)
        else:
            # Native test entry uses its supported debug profile. The app
            # itself is compiled once per backend and reused for all launches.
            for engine in ('h', 'llvm', 'cranelift'):
                args = [] if engine == 'h' else ['--native', '--backend', engine]
                code, out, error = run('package-' + engine, [hirsel, 'test', manifest, *args])
                assert code == 0 and b'2 passed' in out + error, (engine, code, out, error)
            for backend in ('llvm', 'cranelift'):
                exe = output / ('docsession-' + backend)
                built = run('build-' + backend, [hirsel, 'build', manifest, '--backend', backend, '-o', exe])
                assert built[0] == 0, (backend, built)
                result['builds'][backend] += 1
                binaries[backend] = exe
        binary_hashes = {str(p): sha(p) for p in binaries.values()}
        (output / 'binaries.json').write_text(json.dumps(binary_hashes, indent=2))
        tapes = []
        for engine in ('h', 'llvm', 'cranelift'):
            for name in ('transactions', 'errors'):
                data = (root / 'fixtures' / (name + '.in')).read_bytes()
                expected = (root / 'fixtures' / (name + '.out')).read_bytes()
                tape = output / (engine + '-' + name + '.tape')
                run(engine + '-record-' + name, cmd(engine, record=tape), data, (0, expected, b''))
                tapes.append((tape, expected))
                result['recordings'] += 1
                # transactions has deliberately unread commands following QUIT.
                lines = data.splitlines(keepends=True)[:len(expected.splitlines())]
                interactive(engine + '-interactive-' + name, engine, lines, expected.splitlines(keepends=True), fragment=True)
            interactive(engine + '-eof-rollback', engine, [b'BEGIN\n', b'SET 1\n'], [ok('begin'), ok('set')], eof=True)
            interactive(engine + '-utf8-fragments', engine,
                        [b'SET "\xc3\xa9\xf0\x9f\x99\x82"\n', b'SHOW\n', b'QUIT\n'],
                        [ok('set'), value(b'"\xc3\xa9\xf0\x9f\x99\x82"'), ok('quit')], fragment=True)
            io_faults(engine)
            run(engine + '-empty', cmd(engine), b'', (0, b'', b''))
            run(engine + '-unterminated', cmd(engine), b'SHOW', (0, null, b''))
            run(engine + '-crlf', cmd(engine), b'SHOW\r\nQUIT\r\n\xff', (0, null + ok('quit'), b''))
            run(engine + '-bare-cr', cmd(engine), b'SHOW\r', (0, err('UnknownCommand'), b''))
            run(engine + '-utf8', cmd(engine), b'SHOW\n\xff\nSHOW\n', fail('Input(InvalidUtf8)', null))
            # The logical line cap excludes CRLF/LF and is byte based.
            for ending, suffix in ((b'\n', 'lf'), (b'\r\n', 'crlf'), (b'', 'eof')):
                line = b'SET "' + b'a' * 8186 + b'"'
                assert len(line) == 8192
                run(engine + '-cap-' + suffix, cmd(engine), line + ending, (0, ok('set'), b''))
                run(engine + '-over-' + suffix, cmd(engine), line + b' ' + ending, fail('Input(Limit)' if suffix == 'crlf' else 'LineLimit'))
            run(engine + '-framing-over', cmd(engine), b'SHOW\n' + b'x' * 8195, fail('Input(Limit)', null))
            run(engine + '-commands-cap', cmd(engine), b'SHOW\n' * 1024, (0, null * 1024, b''))
            run(engine + '-commands-over', cmd(engine), b'SHOW\n' * 1025, fail('CommandLimit', null * 1024))
            # Escaped quote/backslash and structural bytes in text do not add depth.
            quoted = b'"' + b'[' * 40 + b'\\\"' + b'\\\\' + b']' * 40 + b'"'
            deep = b'[' * 32 + b'0' + b']' * 32
            too_deep = b'[' * 33 + b'0' + b']' * 33
            data = b'SET ' + quoted + b'\nSET ' + deep + b'\nBEGIN\nSET ' + too_deep + b'\nPATCH ' + too_deep + b'\nSHOW\nROLLBACK\nSHOW\n'
            expected = ok('set') * 2 + ok('begin') + err('DepthLimit') * 2 + value(deep) + ok('rollback') + value(deep)
            run(engine + '-depth', cmd(engine), data, (0, expected, b''))
            # 8 chunks of 8178 string bytes + 9th of 48 => exact compact 65536.
            members = [(str(i).encode(), b'a' * 8178) for i in range(8)] + [(b'8', b'a' * 48)]
            document = b'{' + b','.join(b'"' + k + b'":"' + v + b'"' for k, v in members) + b'}'
            assert len(document) == 65536
            patches = [b'PATCH {"' + k + b'":"' + v + b'"}\n' for k, v in members]
            assert all(len(p) - 1 <= 8192 for p in patches)
            data = b''.join(patches) + b'BEGIN\nPATCH {"8":"' + b'a' * 49 + b'"}\nSHOW\nROLLBACK\nSHOW\n'
            expected = ok('patch') * 9 + ok('begin') + err('DocumentLimit') + value(document) + ok('rollback') + value(document)
            run(engine + '-document-cap', cmd(engine), data, (0, expected, b''))
            # RFC6901 page-5 vectors retained independently from serde_json.
            pointer_doc = b'{"foo":["bar","baz"],"":0,"a/b":1,"c%d":2,"e^f":3,"g|h":4,"i\\\\j":5,"k\\\"l":6," ":7,"m~n":8}'
            pointers = [(b'', pointer_doc), (b'/foo', b'["bar","baz"]'), (b'/foo/0', b'"bar"'), (b'/', b'0'),
                        (b'/a~1b', b'1'), (b'/c%d', b'2'), (b'/e^f', b'3'), (b'/g|h', b'4'),
                        (b'/i\\j', b'5'), (b'/k"l', b'6'), (b'/ ', b'7'), (b'/m~0n', b'8')]
            data = b'SET ' + pointer_doc + b'\n' + b''.join(b'GET ' + p + b'\n' for p, _ in pointers)
            expected = ok('set') + b''.join(value(v) for _, v in pointers)
            run(engine + '-rfc6901', cmd(engine), data, (0, expected, b''))
            data = b'SET {"a":1}\nPATCH {"a":null}\nSHOW\nPATCH [1,null]\nGET /1\nPATCH null\nGET\nHELP\n'
            help_line = b'{"ok":"help","commands":["SET <json>","PATCH <json>","GET <pointer>","SHOW","BEGIN","COMMIT","ROLLBACK","HELP","QUIT"]}\n'
            run(engine + '-merge-help', cmd(engine), data, (0, ok('set') + ok('patch') + value(b'{}') + ok('patch') + null + ok('patch') + null + help_line, b''))
        # Physically retire grants and all original live stdin; replay receives
        # EOF and no grants flag. Recordings are checked in every consumer.
        grants.rename(output / 'grants.retired.json')
        for path in output.glob('*.stdin'):
            path.rename(path.with_suffix('.retired-stdin'))
        for index, (tape, expected) in enumerate(tapes):
            for engine in ('h', 'llvm', 'cranelift'):
                run('replay-' + str(index) + '-' + engine, cmd(engine, replay=tape), b'', (0, expected, b''))
                result['replays'] += 1
        assert all(sha(Path(p)) == h for p, h in (frozen | binary_hashes).items())
        result['status'] = 'passed'
        print(f"docsession: {result['observations']} observations; {result['interactive_commands']} response-before-next-input commands; 6 records; 18 replays; builds={result['builds']}")
    except BaseException as exc:
        result['status'] = 'failed'
        result['error'] = repr(exc)
        raise
    finally:
        save()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--reuse', type=Path)
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(), args.reuse.resolve() if args.reuse else None)
