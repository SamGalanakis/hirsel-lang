#!/usr/bin/env python3
"""Small fixed RFC6902 expectations, copied to a private temp package by Rust.

Upstream vectors: json-patch basic.rs:7-48 and lib.rs:440-464, revision
1b0f707f9f4aad1009f0364f6ca73c5e88cdd85c, MIT, Copyright 2017 Ivan Dubrov.
See docs/json-merge-patch-LICENSE-MIT.txt. All other answers are hand-derived
RFC6902 examples; no expected document is obtained from Hirsel.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def invoke(request):
    item = json.loads(request.read_text())
    done = subprocess.run(item['argv'], stdin=subprocess.DEVNULL, capture_output=True,
                          timeout=55, env=os.environ | {'HIRSEL_DAEMON': 'off'})
    stem = Path(item['stem'])
    stem.with_suffix('.stdout').write_bytes(done.stdout)
    stem.with_suffix('.stderr').write_bytes(done.stderr)
    stem.with_suffix('.status').write_text(str(done.returncode))


def verify(hirsel, root, output):
    output.mkdir(exist_ok=False)
    data = root / 'fixtures'
    selection = root / 'hirsel.json'
    commands = []
    env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_RESULTS': 'none@1', 'HIRSEL_JOBS': '1'}

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stem = output / label
        if runtime:
            request = output / (label + '.request.json')
            request.write_text(json.dumps(dict(argv=argv, stem=str(stem))))
            wrapper = subprocess.run(
                [sys.executable, root / 'capped', '--timeout', '60', sys.executable,
                 Path(__file__), '--invoke', request],
                stdin=subprocess.DEVNULL, capture_output=True, timeout=80,
                env=env | {'HIRSEL_MEMORY_LIMIT': '2G'})
            (output / (label + '.cap.stderr')).write_bytes(wrapper.stderr)
            assert wrapper.returncode == 0, (label, wrapper, str(output))
            answer = (int(stem.with_suffix('.status').read_text()),
                      stem.with_suffix('.stdout').read_bytes(),
                      stem.with_suffix('.stderr').read_bytes())
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                  timeout=300, env=env)
            stem.with_suffix('.stdout').write_bytes(done.stdout)
            stem.with_suffix('.stderr').write_bytes(done.stderr)
            answer = (done.returncode, done.stdout, done.stderr)
        commands.append(dict(label=label, argv=argv, exit=answer[0]))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return answer

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    tested = run('pure-tests', [hirsel, 'test', selection, '--seed', '1'])
    assert tested[0] == 0 and b'1 passed; 0 failed; 1 total' in tested[1], tested
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        binary = output / backend
        answer = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', binary])
        assert answer == (0, b'', b''), (backend, answer, str(output))
        binaries[backend] = binary

    cases = []

    def case(name, document, patch, expected=None, error=None):
        doc_name, patch_name = name + '-document.json', name + '-patch.json'
        (data / doc_name).write_bytes(document)
        (data / patch_name).write_bytes(patch)
        cases.append((name, [doc_name, patch_name],
                      (2, b'', encoded(error)) if error else (0, encoded(expected), b''), None))

    for vector in json.loads((data / 'vectors.json').read_text()):
        case(vector['name'], encoded(vector['document']), encoded(vector['patch']),
             vector.get('expected'), vector.get('error'))

    def rejection(stage, kind, detail='', operation=None, path=None):
        return dict(schema=1, **{'class': 'invalid_input'}, stage=stage, kind=kind,
                    operation=operation, path=path, detail=detail)

    case('malformed-document', b'{', b'[]', error=rejection('document', 'InvalidJson', 'Syntax(1)'))
    case('malformed-patch', b'{}', b'[', error=rejection('patch', 'InvalidJson', 'Syntax(1)', 0, ''))
    case('duplicate-document', b'{"a":1,"a":2}', b'[]',
         error=rejection('document', 'InvalidJson', 'DuplicateKey(7)'))
    case('utf8', b'\xff', b'[]', error=rejection('input', 'Utf8'))
    case('document-over', b' ' * 65537, b'[]', error=rejection('input', 'BytesLimit'))
    case('patch-over', b'{}', b' ' * 65537, error=rejection('input', 'BytesLimit'))
    case('document-exact', b'null' + b' ' * 65532, b'[]', dict(schema=1, present=True, document=None))
    case('patch-exact', b'null', b'[]' + b' ' * 65534, dict(schema=1, present=True, document=None))

    case('depth-limit', b'[' * 33 + b'0' + b']' * 33, b'[]',
         error=rejection('patch', 'DepthLimit', '', 0, ''))
    case('tokens-limit', b'{}', encoded([dict(op='remove', path='/x' * 33)]),
         error=rejection('patch', 'TokensLimit', '', 0, '/x' * 33))

    # The realistic checked-in files are exercised verbatim.
    expected = dict(schema=1, present=True, document={
        'service': 'search', 'revision': 8, 'replicas': 3,
        'labels': {'team/platform': 'search'}, 'checks': ['ready', 'canary'],
        'timeout': 30, 'previous': {'timeout': 30}})
    cases.append(('promote', ['deployment.json', 'promote.patch.json'], (0, encoded(expected), b''), None))
    cases.append(('stale', ['deployment.json', 'stale.patch.json'],
                  (2, b'', encoded(rejection('patch', 'TestFailed', '', 1, '/revision'))), None))
    help_text = (b'Usage: jsonpatchctl DOCUMENT PATCH\n\n'
                 b'Apply a bounded RFC 6902 patch; publish only after complete success.\n'
                 b'Files are relative to the read-only input directory grant.\n'
                 b'Limits: 65536 bytes per file, 64 operations, depth 32, 32 pointer tokens.\n'
                 b'\nOptions:\n  --help  Print help\n')
    cases.append(('help', ['--help'], (0, help_text, b''), 'no-dir'))
    for name, args, error, grant in [
        ('usage', [], rejection('arguments', 'Usage', 'expected DOCUMENT PATCH or --help'), None),
        ('missing-file', ['missing.json', 'promote.patch.json'], rejection('input', 'FileOpen'), None),
        ('missing-grant', ['deployment.json', 'promote.patch.json'], rejection('input', 'DirectoryGrant', 'input'), 'no-dir'),
        ('escape', ['../deployment.json', 'promote.patch.json'],
         rejection('input', 'Filename', 'relative normalized path, at most 256 bytes'), None),
    ]:
        cases.append((name, args, (2, b'', encoded(error)), grant))
    cases.append(('deterministic-promote', ['deployment.json', 'promote.patch.json'], (0, encoded(expected), b''), None))

    # Nothing under data is writable by the program; content AND directory entries
    # must be unchanged after every cell, including failures following an edit.
    def snapshot():
        return {str(p.relative_to(data)): (hashlib.sha256(p.read_bytes()).hexdigest()
                                               if p.is_file() else 'directory')
                for p in data.rglob('*')}

    frozen = snapshot()
    grants = root / 'launch.json'
    observations = 0
    deterministic = {}
    for label, args, expected, grant in cases:
        policy = dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[]), args=args)
        if grant != 'no-dir':
            policy['io']['dirs'] = [dict(path='input', host=str(data), access='read')]
        grants.write_text(json.dumps(policy))
        for engine in ('h', 'llvm', 'cranelift'):
            argv = ([hirsel, 'run', selection, '--grants', grants] if engine == 'h' else
                    [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, argv, runtime=True)
            assert actual[0] == expected[0] and actual[2] == expected[2], (label, engine, actual, expected, str(output))
            if expected[0] == 0 and args != ['--help']:
                # RFC6902 object equality is independent of member order. The
                # shipped edit removes/reinserts replaced keys; check semantics
                # against fixed answers, compact encoding and byte determinism
                # separately, without imposing Python's replacement key order.
                got = json.loads(actual[1])
                want = json.loads(expected[1])
                assert json.dumps(got, sort_keys=True) == json.dumps(want, sort_keys=True), (label, engine, got, want)
                assert actual[1] == encoded(got), (label, engine, 'noncompact output')
                key = tuple(args)
                assert deterministic.setdefault(key, actual[1]) == actual[1], (label, engine, 'nondeterministic output')
            else:
                assert actual[1] == expected[1], (label, engine, actual, expected, str(output))
            assert snapshot() == frozen, (label, engine, 'input files changed')
            observations += 1
    grants.unlink()
    assert not grants.exists()
    # Runtime commands have all been joined, and all private fixture files can
    # be removed immediately; the Rust owner asserts complete tempdir cleanup.
    for p in data.iterdir():
        p.unlink()
    data.rmdir()
    assert not data.exists()
    result = dict(cases=len(cases), observations=observations, pure_tests=1,
                  engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--invoke', type=Path)
    parser.add_argument('--hirsel', type=Path)
    parser.add_argument('--root', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.invoke:
        invoke(args.invoke)
    else:
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
