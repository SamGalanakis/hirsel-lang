#!/usr/bin/env python3
"""Single-file application oracle; no expected answer calls Hirsel's diff.

Replace and identical vectors adapt imara-diff tests/integration/main.rs:26-94,
f02e46a737c43c0d9c65a42e5e08046bd10fd739, Apache-2.0, Copyright Pascal Kuthe
and imara-diff contributors (third-party/NOTICES.md). Changes: retain inputs
verbatim but validate applicable output rather than one legal edit script;
exercise the shipped default Myers with file headers and fixed context=3.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import subprocess
import sys

HELP = (b'patchctl diff OLD NEW\npatchctl apply SOURCE PATCH\n'
        b'Read UTF-8 leaf filenames in the fixtures grant; write result to stdout.\n'
        b'Myers, context=3; rejected hunk indexes are zero-based.\n'
        b'Limits: 65536 bytes/input, 64 text lines, 256 patch lines.\n'
        b'Exit: 0 success/help, 1 rejected hunks, 2 invalid input/grant/I/O.\n')


def lf_lines(blob):
    # bytes.splitlines would also split CR, VT and FF, unlike std's LF policy.
    pieces = blob.split(b'\n')
    return [p + b'\n' for p in pieces[:-1]] + ([pieces[-1]] if pieces[-1] else [])


def apply_oracle(source, patch):
    """Tiny independently authored strict unified-patch oracle, for exact positions.

    Generated diffs must consume unchanged gaps and old lines exactly, and their
    new positions/counts must agree. The app's offset-search policy is tested
    separately with explicit inputs and manually specified expected bytes.
    """
    if not patch:
        return source
    lines = lf_lines(patch)
    assert lines[:2] == [b'--- old\n', b'+++ new\n'], lines[:2]
    original, result, done, i = lf_lines(source), [], 0, 2
    while i < len(lines):
        match = re.fullmatch(rb'@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@\n', lines[i])
        assert match, lines[i]
        a, n, b, m = (int(x) if x is not None else 1 for x in match.groups())
        start = a - 1 if n else a
        new_start = b - 1 if m else b
        assert start >= done
        result.extend(original[done:start])
        assert len(result) == new_start
        old, new = [], []
        i += 1
        while i < len(lines) and not lines[i].startswith(b'@@ '):
            line = lines[i]
            assert line[:1] in (b' ', b'-', b'+'), line
            content = line[1:]
            i += 1
            if i < len(lines) and lines[i] == b'\\ No newline at end of file\n':
                assert content.endswith(b'\n')
                content = content[:-1]
                i += 1
            if line[:1] != b'+':
                old.append(content)
            if line[:1] != b'-':
                new.append(content)
        assert (len(old), len(new)) == (n, m)
        assert original[start:start + n] == old
        result.extend(new)
        done = start + n
    result.extend(original[done:])
    return b''.join(result)


def runtime_limits():
    resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
    # RLIMIT_DATA bounds allocations while permitting LLVM's shared mappings.
    resource.setrlimit(resource.RLIMIT_DATA, (2 << 30, 2 << 30))


def verify(hirsel, root, output, capped=None):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'fixtures'
    shutil.copytree(root / 'fixtures', data)
    selection = root / 'hirsel.json'
    commands = []
    env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_RESULTS': 'none@1',
                        'HIRSEL_JOBS': '1', 'HIRSEL_MEMORY_LIMIT': '2G'}

    def run(label, argv, setup=False):
        argv = list(map(str, argv))
        stdout_file, stderr_file = output / (label + '.stdout'), output / (label + '.stderr')
        if capped and not setup:
            redirect = ('import os,sys; '
                        'os.dup2(os.open(sys.argv[1],os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600),1); '
                        'os.dup2(os.open(sys.argv[2],os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600),2); '
                        'os.execv(sys.argv[3],sys.argv[3:])')
            wrapped = [str(capped), '--timeout', '60', sys.executable, '-c', redirect,
                       str(stdout_file), str(stderr_file), *argv]
            done = subprocess.run(wrapped, stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=90)
            (output / (label + '.capped')).write_bytes(done.stderr)
            assert stdout_file.exists() and stderr_file.exists(), (label, done)
            status = done.returncode, stdout_file.read_bytes(), stderr_file.read_bytes()
            commands.append(dict(label=label, argv=argv, exit=status[0]))
            (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
            return status
        done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=600 if setup else 60, env=env,
                              preexec_fn=None if setup else runtime_limits)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        commands.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return done.returncode, done.stdout, done.stderr

    assert run('fmt', [hirsel, 'fmt', '--check', selection], True) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection], True) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir'], True) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe], True)
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = exe

    policy = json.loads((root / 'grants.json').read_text())
    assert policy == dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[
        dict(path='fixtures', host='fixtures', access='read')]), args=['diff', 'old.txt', 'new.txt'])
    policy['io']['dirs'][0]['host'] = str(data)
    grants = output / 'grants.json'
    observations = 0

    def invoke(label, engine, args, expected=None, omit=None):
        nonlocal observations
        current = json.loads(json.dumps(policy))
        if omit == 'dir':
            current['io']['dirs'] = []
        elif omit:
            current['io'].pop(omit)
        current['args'] = args
        grants.write_text(json.dumps(current))
        command = ([hirsel, 'run', selection, '--max-steps', 'unbounded', '--grants', grants]
                   if engine == 'h' else [binaries[engine], '--hirsel-grants', grants, *args])
        frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else 'directory'
                  for p in data.iterdir()}
        actual = run(label + '-' + engine, command)
        assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else 'directory'
                          for p in data.iterdir()}, (label, engine, 'host writes')
        observations += 1
        if expected is not None:
            assert actual == expected, (label, engine, actual, expected, str(output))
        return actual

    before = b'fn foo() -> Bar{\n    let mut foo = 2.0;\n    foo *= 100 / 2;\n    println!("hello world")\n}\n'
    after = b'const TEST: i32 = 0;\nfn foo() -> Bar{\n    let mut foo = 2.0;\n    foo *= 100 / 2;\n    println!("hello world");\n    println!("hello foo {TEST}");\n}\n\n'
    identical = b'fn foo() -> Bar{\n    let mut foo = 2.0;\n    foo *= 100 / 2;\n}'
    vectors = [
        ('replace', before, after), ('identical', identical, identical),
        ('empty', b'', b''), ('insert', b'', b'new\n'), ('delete', b'old\n', b''),
        ('repeated', b'a\nx\na\nx\n', b'a\nx\nx\na\n'),
        ('no-final', b'alpha\nold', b'alpha\nnew'),
        ('add-final', b'x', b'x\n'), ('remove-final', b'x\n', b'x'),
        ('crlf', b'alpha\r\nold\r\n', b'alpha\r\nnew\r\n'),
        ('unicode', '羊\nold\n'.encode(), '羊\n新\n'.encode()),
        ('two-hunks', b'a\n' + b'middle\n' * 8 + b'z\n', b'b\n' + b'middle\n' * 8 + b'y\n'),
        ('realistic', (data / 'old.txt').read_bytes(), (data / 'new.txt').read_bytes()),
        ('exact-lines', b'x\n' * 64, b'x\n' * 64),
        ('exact-bytes', b'x' * 65536, b'x' * 65536),
    ]
    cases = 0
    for label, old, new in vectors:
        (data / 'a.txt').write_bytes(old)
        (data / 'b.txt').write_bytes(new)
        generated = []
        for engine in ('h', 'llvm', 'cranelift'):
            actual = invoke(label + '-diff', engine, ['diff', 'a.txt', 'b.txt'])
            assert actual[0] == 0 and actual[2] == b'', (label, engine, actual)
            assert apply_oracle(old, actual[1]) == new, (label, engine)
            if old == new:
                assert actual[1] == b''
            generated.append(actual[1])
            (data / 'generated.patch').write_bytes(actual[1])
            invoke(label + '-roundtrip', engine, ['apply', 'a.txt', 'generated.patch'], (0, new, b''))
        assert len(set(generated)) == 1, (label, 'determinism across engines')
        repeated = invoke(label + '-repeat', 'h', ['diff', 'a.txt', 'b.txt'])
        assert repeated == (0, generated[0], b'')
        cases += 2

    def case(label, args, code=0, text=b'', error=None, omit=None):
        nonlocal cases
        stderr = b'' if error is None else ('patchctl: ' + error + '\n').encode()
        for engine in ('h', 'llvm', 'cranelift'):
            invoke(label, engine, args, (code, text, stderr), omit)
        cases += 1

    case('help', ['--help'], text=HELP, omit='dir')
    case('authored', ['apply', 'old.txt', 'review.patch'], text=(data / 'new.txt').read_bytes())
    case('stale', ['apply', 'old.txt', 'stale.patch'], 1, error='rejected_hunks [0]')
    (data / 'a.txt').write_bytes(b'a\nb\nc\n')
    (data / 'partial.patch').write_bytes(b'--- old\n+++ new\n@@ -1 +1 @@\n-a\n+A\n@@ -3 +3 @@\n-wrong\n+C\n')
    case('partial', ['apply', 'a.txt', 'partial.patch'], 1, error='rejected_hunks [1]')
    (data / 'both.patch').write_bytes(b'--- old\n+++ new\n@@ -1 +1 @@\n-wrong\n+A\n@@ -3 +3 @@\n-wrong\n+C\n')
    case('both-rejected', ['apply', 'a.txt', 'both.patch'], 1, error='rejected_hunks [0,1]')
    (data / 'offset.patch').write_bytes(b'--- old\n+++ new\n@@ -1 +1 @@\n-b\n+B\n')
    case('offset', ['apply', 'a.txt', 'offset.patch'], text=b'a\nB\nc\n')
    bad = [
        ('headerless', b'garbage\n', 'patch_envelope'),
        ('malformed', b'--- old\n+++ new\n@@ -1 +1 @@\n?bad\n', 'malformed_patch line=4'),
        ('truncated', b'--- old\n+++ new\n@@ -1 +1 @@\n-a\n', 'malformed_patch line=5'),
        ('multi-file', b'--- old\n+++ new\n@@ -1 +1 @@\n-a\n+A\n--- second\n+++ second\n', 'malformed_patch line=6'),
        ('huge-position', b'--- old\n+++ new\n@@ -18446744073709551615 +1 @@\n-a\n+A\n', 'patch_range'),
        ('huge-count', b'--- old\n+++ new\n@@ -1,65 +1 @@\n-a\n+A\n', 'patch_range'),
        ('patch-lines', b'x\n' * 257, 'line_count_limit'),
    ]
    for label, patch, error in bad:
        (data / (label + '.patch')).write_bytes(patch)
        case(label, ['apply', 'a.txt', label + '.patch'], 2, error=error)
    for label, blob, error in [('utf8', b'\xff', 'utf8'), ('over-bytes', b'x' * 65537, 'input_bytes_limit'),
                               ('over-lines', b'x\n' * 65, 'line_count_limit'),
                               ('over-tail', b'x\n' * 64 + b'x', 'line_count_limit')]:
        (data / (label + '.txt')).write_bytes(blob)
        case(label, ['diff', label + '.txt', 'a.txt'], 2, error=error)
    case('missing-grant', ['diff', 'old.txt', 'new.txt'], 2, error='grant', omit='dir')
    case('missing-stdout', ['diff', 'old.txt', 'new.txt'], 2, error='grant', omit='stdout')
    case('missing-stderr', ['apply', 'old.txt', 'stale.patch'], 1, omit='stderr')
    case('missing-file', ['diff', 'absent', 'old.txt'], 2, error='file_not_found')
    (data / 'directory').mkdir()
    case('directory-input', ['diff', 'directory', 'old.txt'], 2, error='file_error')
    (data / 'directory').rmdir()
    case('usage', [], 2, error='usage')
    case('unknown', ['wat', 'old.txt', 'new.txt'], 2, error='usage')
    for i, name in enumerate(['../outside', '/absolute', 'a/b', 'a\\b', 'x\nheader', 'x' * 256]):
        case('filename-' + str(i), ['diff', name, 'old.txt'], 2, error='filename')

    # Every process above is read-only: snapshot around a fresh realistic run,
    # covering generated files as well as shipped inputs and filename labels.
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    case('effects', ['apply', 'old.txt', 'review.patch'], text=(data / 'new.txt').read_bytes())
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    assert not (output / 'outside').exists()
    shutil.rmtree(data)
    grants.unlink()
    for exe in binaries.values():
        exe.unlink()
    assert not data.exists() and not grants.exists() and not any(p.exists() for p in binaries.values())
    result = dict(cases=cases, observations=observations, vectors=len(vectors), engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--capped', type=Path, help='host tools/capped for each runtime process')
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(),
           args.capped.resolve() if args.capped else None)
