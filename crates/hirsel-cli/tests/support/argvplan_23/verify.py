#!/usr/bin/env python3
"""CPython shlex oracle, bounded application controls, and three actual engines.

Vectors: CPython Lib/test/test_shlex.py:95-125, 036d309cf542986a39eeba5ca805b2ec7619731d.
Copyright (c) 2001 Python Software Foundation; All Rights Reserved.
PSF-2.0: examples/argvplan/fixtures/PSF-LICENSE.txt. The vectors are stored
as JSON pairs; the application adds physical-line boundaries and explicit caps.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shlex
import subprocess


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
    resource.setrlimit(resource.RLIMIT_DATA, (2 * 1024**3, 2 * 1024**3))


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, separators=(',', ':')) + '\n').encode()


def oracle(source, comments=False):
    commands = []
    # Physical LF only: quoted newlines and continuations are not application input.
    for number, line in enumerate(source.split('\n'), 1):
        argv = shlex.split(line, posix=True, comments=comments)
        if argv:
            commands.append(dict(line=number, argv=argv, joined=shlex.join(argv)))
    return dict(schema=1, commands=commands)


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'fixtures'
    data.mkdir()
    selection = root / 'hirsel.json'
    history = []
    environment = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                                    HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=75 if runtime else 300, env=environment,
                              cwd=output, preexec_fn=limits if runtime else None)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        history.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(history, indent=2) + '\n')
        return done.returncode, done.stdout, done.stderr

    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        binary = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', binary])
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = binary

    cases = []

    def success(label, source, comments=False):
        name = label + '.txt'
        blob = source.encode() if isinstance(source, str) else source
        (data / name).write_bytes(blob)
        expected = oracle(blob.decode(), comments)
        cases.append((label, ['--comments', name] if comments else [name], 0, expected, None))
        return expected

    def error(label, blob, kind, line=0, byte=0, args=None, grant=None):
        name = label + '.txt'
        if blob is not None:
            (data / name).write_bytes(blob)
        cases.append((label, [name] if args is None else args, 2,
                      dict(kind=kind, line=line, byte=byte), grant))

    vectors = json.loads((root / 'fixtures/cpython-posix.json').read_text())
    assert len(vectors) == 31
    for vector in vectors:
        assert shlex.split(vector['source'], posix=True) == vector['argv'], vector
    batch = '\n'.join(v['source'] for v in vectors) + '\n'
    expected = success('cpython', batch)
    success('join-split-roundtrip', '\n'.join(c['joined'] for c in expected['commands']) + '\n')
    success('review', (root / 'fixtures/review.txt').read_bytes())
    error('invalid-review', (root / 'fixtures/invalid.txt').read_bytes(), 'unclosed_quote', 2, 12)
    comment = 'echo ready#ignored "unterminated\n# comment\necho "#kept" \\#literal\n'
    success('comments-on', comment, True)
    # A separate valid twin checks both policies against identical bytes.
    twin = 'echo ready#tail # note\necho "#kept" \\#literal\n'
    success('policy-on', twin, True)
    success('policy-off', twin)
    error('comments-off-invalid', comment.encode(), 'unclosed_quote', 1, 19)
    success('metacharacters', 'echo $HOME `touch sentinel` *.hir | cat > sentinel ; && (x)\n')
    success('unicode', 'echo 羊 café "日本 語"\n')
    success('repeated', 'echo same\necho same\n\n\techo same\n')
    success('blank', '\n \t\r\n')
    success('empty', '')
    success('crlf', 'echo one\r\necho "two\r"\r\n')
    success('non-shell-whitespace', 'echo a\vb\fc\u00a0d\n')
    success('nul-is-data', 'echo "a\x00b"\n')
    error('single-quote', b"echo 'broken", 'unclosed_quote', 1, 5)
    error('unicode-offset', 'echo 羊 "broken'.encode(), 'unclosed_quote', 1, 9)
    error('escape', b'echo value\\', 'trailing_escape', 1, 10)
    error('quoted-escape', b'echo "value\\', 'trailing_escape', 1, 11)
    error('no-multiline', b'echo "a\nb"\n', 'unclosed_quote', 1, 5)
    error('no-continuation', b'echo a\\\nb\n', 'trailing_escape', 1, 6)
    error('utf8', b'echo \xff', 'invalid_utf8')
    success('line-exact', 'x' * 4096)
    error('line-over', b'x' * 4097, 'line_bytes_limit', 1, 4096)
    success('records-exact', 'x\n' * 64)
    error('records-over', b'\n' * 65, 'record_limit', 65)
    success('args-exact', ' '.join(['""'] * 64))
    error('args-over', (' '.join(['""'] * 65)).encode(), 'argument_limit', 1)
    success('total-exact', (' '.join(['x'] * 64) + '\n') * 8)
    error('total-over', ((' '.join(['x'] * 64) + '\n') * 8 + 'x').encode(), 'total_argument_limit', 9)
    success('input-exact', (' ' * 4095 + '\n') * 16)
    error('input-over', b' ' * 65537, 'input_bytes_limit', byte=65536)
    error('missing-file', None, 'file_open')
    error('no-directory', b'echo x', 'directory_grant', grant='dir')
    error('path-traversal', None, 'filename', args=['../sentinel'])
    error('usage', None, 'usage', args=[])
    error('unknown-option', None, 'filename', args=['--unknown'])
    cases.append(('help', ['--help'], 0, None, 'dir'))
    cases.append(('short-help', ['-h'], 0, None, 'dir'))
    cases.append(('no-stdout', ['review.txt'], 3, None, 'stdout'))
    cases.append(('no-stderr', ['review.txt'], 3, None, 'stderr'))
    # Same input, grants and output must be byte-for-byte deterministic on repeated launches.
    cases.append(('deterministic-repeat', ['review.txt'], 0,
                  oracle((root / 'fixtures/review.txt').read_text()), None))

    grants = output / 'grants.json'
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    help_text = None
    identical = {}
    observations = 0
    for label, args, status, value, denied in cases:
        io = dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[
            dict(path='fixtures', host=str(data), access='read')])
        if denied == 'dir': io['dirs'] = []
        elif denied: del io[denied]
        grants.write_text(json.dumps(dict(io=io, args=args)))
        for engine in ('h', 'llvm', 'cranelift'):
            argv = ([hirsel, 'run', selection, '--max-steps', 'unbounded', '--grants', grants]
                    if engine == 'h' else [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, argv, runtime=True)
            code, stdout, stderr = actual
            assert code == status, (label, engine, actual, status)
            if label in ('help', 'short-help'):
                assert stderr == b'' and stdout.startswith(b'Usage: argvplan '), actual
                if help_text is None: help_text = stdout
                assert stdout == help_text
            elif status == 3:
                assert stdout == stderr == b'', actual
            elif status == 2:
                assert stdout == b'' and json.loads(stderr) == value, (label, engine, actual, value)
                assert stderr == encoded(value), (label, engine, stderr)
            else:
                assert stderr == b'' and json.loads(stdout) == value, (label, engine, actual, value)
                for command in value['commands']:
                    assert shlex.split(command['joined'], posix=True) == command['argv']
                assert stdout == encoded(value), (label, engine, stdout[:500])
            previous = identical.setdefault(label, actual)
            assert actual == previous, (label, engine, 'engine mismatch')
            if label == 'deterministic-repeat':
                assert actual == identical['review']
            assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
            assert not (output / 'sentinel').exists(), 'command text was executed'
            observations += 1
    for file in data.iterdir(): file.unlink()
    data.rmdir()
    grants.unlink()
    assert not data.exists() and not grants.exists()
    result = dict(cases=len(cases), observations=observations, upstream_vectors=len(vectors),
                  engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
