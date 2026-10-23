#!/usr/bin/env python3
"""Bounded real CLI matrix. Expectations are literals from application policy,
Python tomllib for valid TOML values, and the cited upstream reference vectors.
No output from Hirsel supplies an expected answer.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

HELP = (b'Usage: configcheck <toml|yaml> FILE\n       configcheck --help\n\n'
        b'Validate one service; print normalized JSON. FILE is relative to the input grant.\n'
        b'Exit: 0 success/help, 2 invalid input/usage/grant, 3 stdout unavailable.\n')


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def error(kind, path='$', line=None, column=None):
    return canonical(dict(error=dict(kind=kind, path=path, line=line, column=column)))


def normalized(name='checkout-api', port=8443, retries=3, upstreams=()):
    return canonical(dict(name=name, port=port, retries=retries, upstreams=list(upstreams)))


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'input'
    data.mkdir()
    selection = root / 'hirsel.json'
    commands = []
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')

    def run(label, argv, setup=False):
        argv = list(map(str, argv))
        prefix = output / label
        if setup:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                  timeout=900, env=env)
            code, stdout, stderr = done.returncode, done.stdout, done.stderr
            prefix.with_suffix('.stdout').write_bytes(stdout)
            prefix.with_suffix('.stderr').write_bytes(stderr)
        else:
            # The cap's telemetry has its own file; application stderr is compared
            # byte for byte, including when it is empty. Builds are setup.
            wrapped = [sys.executable, root / 'capped', '--timeout', '60', sys.executable,
                       Path(__file__).resolve(), '--cell', json.dumps(argv), str(prefix)]
            done = subprocess.run(list(map(str, wrapped)), stdin=subprocess.DEVNULL,
                                  capture_output=True, timeout=90, env=env)
            prefix.with_suffix('.cap').write_bytes(done.stdout + done.stderr)
            assert done.returncode == 0, (label, done, str(output))
            code = json.loads(prefix.with_suffix('.status').read_text())
            stdout = prefix.with_suffix('.stdout').read_bytes()
            stderr = prefix.with_suffix('.stderr').read_bytes()
        commands.append(dict(label=label, argv=argv, exit=code))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return code, stdout, stderr

    assert run('fmt', [hirsel, 'fmt', '--check', selection], True) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        built = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe], True)
        assert built == (0, b'', b''), (backend, built, str(output))
        binaries[backend] = exe

    # Execute upstream values, including nested record sequences and literal
    # versus dotted TOML keys, through each engine as an independent pure entry.
    vector_manifest = json.loads(selection.read_text())
    vector_manifest['entry'] = dict(module=['model'], name='reference_vectors')
    vector_selection = root / 'vectors.json'
    vector_selection.write_text(json.dumps(vector_manifest))
    empty_grants = output / 'empty-grants.json'
    empty_grants.write_text('{}')
    vector_observations = 0
    for engine in ('h', 'llvm', 'cranelift'):
        if engine == 'h':
            command = [hirsel, 'run', vector_selection, '--grants', empty_grants]
        else:
            exe = output / ('vectors-' + engine)
            assert run('build-vectors-' + engine,
                       [hirsel, 'build', vector_selection, '--backend', engine, '-o', exe], True) == (0, b'', b'')
            command = [exe, '--hirsel-grants', empty_grants]
        actual = run('vectors-' + engine, command)
        assert actual == (0, b'', b''), (engine, actual, str(output))
        vector_observations += 1
    empty_grants.unlink()
    vector_selection.unlink()
    cases = []

    def case(label, fmt, blob, expected=None, kind=None, path='$', position=None):
        filename = label + '.' + fmt
        (data / filename).write_bytes(blob)
        if kind:
            expected = error(kind, path, *(position or (None, None)))
        cases.append((label, [fmt, filename], (2 if kind else 0, expected, b''), None))

    good = normalized(upstreams=['catalog', 'payments'])
    for fmt in ('toml', 'yaml'):
        blob = (root / 'fixtures' / ('service.' + fmt)).read_bytes()
        case('service-' + fmt, fmt, blob, good)
    # Python's separate TOML parser is an independent value oracle for all valid
    # TOML successes; the YAML equivalent has identical explicit expected fields.
    for label, src in [
            ('defaults', "name='checkout-api'\nport=8443\n"),
            ('minimum', "name='a'\nport=1\nretries=0\nupstreams=[]\n"),
            ('maximum', "name='a'\nport=65535\nretries=10\nupstreams=['db']\n"),
            ('escaped', 'name="api \\"blue\\""\nport=80\nupstreams=["日本"]\n'),
            ('input-exact', "name='checkout-api'\nport=8443\n" + '#' + 'x' * (8192 - 31)),
    ]:
        # input-exact is padded below to keep this length independent of source edits.
        if label == 'input-exact':
            src = "name='checkout-api'\nport=8443\n"
            src += '#' + 'x' * (8192 - len(src.encode()) - 1)
            assert len(src) == 8192
        value = tomllib.loads(src)
        expected = normalized(value['name'], value['port'], value.get('retries', 3), value.get('upstreams', []))
        case(label, 'toml', src.encode(), expected)
    case('yaml-explicit', 'yaml', b'name: a\nport: 65535\nretries: 10\nupstreams: [db]\n', normalized('a', 65535, 10, ['db']))
    case('yaml-alias', 'yaml', b'name: &n checkout-api\nport: 8443\nupstreams: [*n]\n', normalized(upstreams=['checkout-api']))
    case('realistic-invalid', 'yaml', (root / 'fixtures/invalid.yaml').read_bytes(), kind='port_range', path='$["port"]')
    for fmt, sep, name in [('toml', '=', "'x'"), ('yaml', ': ', 'x')]:
        base = f'name{sep}{name}\nport{sep}80\n'
        for label, src, kind, path in [
                ('missing-name', f'port{sep}80\n', 'missing_field', '$["name"]'),
                ('missing-port', f'name{sep}{name}\n', 'missing_field', '$["port"]'),
                ('extra', base + f'timeout{sep}1\n', 'unknown_field', '$["timeout"]'),
                ('bool-port', f'name{sep}{name}\nport{sep}true\n', 'expected_integer', '$["port"]'),
                ('text-port', f'name{sep}{name}\nport{sep}"80"\n', 'expected_integer', '$["port"]'),
                ('low-port', f'name{sep}{name}\nport{sep}0\n', 'port_range', '$["port"]'),
                ('high-port', f'name{sep}{name}\nport{sep}65536\n', 'port_range', '$["port"]'),
                ('retry-bool', base + f'retries{sep}false\n', 'expected_integer', '$["retries"]'),
                ('retry-low', base + f'retries{sep}-1\n', 'retries_range', '$["retries"]'),
                ('retry-high', base + f'retries{sep}11\n', 'retries_range', '$["retries"]'),
                ('upstream-kind', base + f'upstreams{sep}["db", 7]\n', 'expected_text', '$["upstreams"][1]'),
                ('upstream-text', base + f'upstreams{sep}"db"\n', 'expected_array', '$["upstreams"]'),
                ('name-kind', f'name{sep}7\nport{sep}80\n', 'expected_text', '$["name"]'),
                ('name-empty', f'name{sep}""\nport{sep}80\n', 'name_length', '$["name"]'),
                ('name-long', f'name{sep}"' + 'x' * 129 + f'"\nport{sep}80\n', 'name_length', '$["name"]'),
                ('collection-over', base + f'upstreams{sep}[' + ','.join(['"db"'] * 65) + ']\n', 'collection_limit', '$["upstreams"]'),
        ]:
            case(fmt + '-' + label, fmt, src.encode(), kind=kind, path=path)
        case(fmt + '-duplicate', fmt, (base + f'port{sep}81\n').encode(), kind='duplicate_key', position=(3, 1))
        case(fmt + '-cap', fmt, b'#' + b'x' * 8192, kind='input_bytes_limit')
        case(fmt + '-utf8', fmt, b'\xff', kind='utf8')
        exact = base + f'upstreams{sep}[' + ','.join(['"db"'] * 64) + ']\n'
        case(fmt + '-collection-exact', fmt, exact.encode(), normalized('x', 80, 3, ['db'] * 64))
    case('quoted-dot', 'toml', b'name="x"\nport=80\n"a.b"=1\n', kind='unknown_field', path='$["a.b"]')
    case('dotted-key', 'toml', b'name="x"\nport=80\na.b=1\n', kind='unknown_field', path='$["a"]')
    case('upstream-quoted-dots', 'toml', (root / 'fixtures/quoted-dots.toml').read_bytes(), kind='unknown_field', path='$["plain"]')
    case('upstream-nested', 'yaml', (root / 'fixtures/nested.yaml').read_bytes(), kind='expected_object')
    case('documents', 'yaml', b'--- a\n--- b\n', kind='documents', position=(2, 1))
    case('toml-syntax', 'toml', b'=\n', kind='toml_syntax', position=(1, 1))
    case('yaml-syntax', 'yaml', b'\x00', kind='yaml_syntax', position=(1, 1))
    case('yaml-tag', 'yaml', b'name: !Ref foo\nport: 80\n', kind='unsupported_value', path='$["name"]')
    case('yaml-null', 'yaml', b'name: null\nport: 80\n', kind='unsupported_value', path='$["name"]')
    case('yaml-merge', 'yaml', b'name: x\nport: 80\n<<: {retries: 1}\n', kind='unknown_field', path='$["<<"]')
    case('toml-date', 'toml', b'name=2000-01-01\nport=80\n', kind='unsupported_value', path='$["name"]')
    case('yaml-depth', 'yaml', b'name: x\nport: 80\nupstreams: ' + b'[' * 17 + b'0' + b']' * 17,
         kind='structure_limit', path='$["upstreams"]' + '[0]' * 16)
    # 1 root + name + port + upstream array + 63 arrays * 4 scalar children
    # reaches the node budget while each collection stays within its own cap.
    case('yaml-nodes', 'yaml', b'name: x\nport: 80\nupstreams: [' + b','.join([b'[0,0,0,0]'] * 63) + b']',
         kind='structure_limit', path='$["upstreams"][50][1]')
    cases.extend([
        ('help', ['--help'], (0, HELP, b''), 'dir'),
        ('usage', [], (2, error('usage'), b''), None),
        ('format', ['json', 'file'], (2, error('usage'), b''), None),
        ('missing-grant', ['toml', 'service-toml.toml'], (2, error('directory_grant'), b''), 'dir'),
        ('missing-file', ['toml', 'absent'], (2, error('file_read'), b''), None),
        ('no-stdout', ['--help'], (3, b'', b''), 'stdout'),
        # Re-run the representative success to assert deterministic bytes.
        ('deterministic', ['toml', 'service-toml.toml'], (0, good, b''), None),
    ])
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    grants = output / 'grants.json'
    observations = 0
    for label, args, expected, omit in cases:
        policy = dict(io=dict(stdout='stdout.bytes@1', dirs=[dict(path='input', host=str(data), access='read')]), args=args)
        if omit == 'dir': policy['io']['dirs'] = []
        if omit == 'stdout': policy['io'].pop('stdout')
        grants.write_text(json.dumps(policy))
        for engine in ('h', 'llvm', 'cranelift'):
            command = ([hirsel, 'run', selection, '--grants', grants] if engine == 'h' else
                       [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, command)
            assert actual == expected, (label, engine, actual, expected, str(output))
            observations += 1
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}, 'input files changed'
    for p in data.iterdir(): p.unlink()
    data.rmdir()
    grants.unlink()
    assert not data.exists() and not grants.exists(), 'fixture cleanup'
    result = dict(cases=len(cases), observations=observations, vector_observations=vector_observations, engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    if sys.argv[1:2] == ['--cell']:
        argv, prefix = json.loads(sys.argv[2]), Path(sys.argv[3])
        with prefix.with_suffix('.stdout').open('wb') as stdout, prefix.with_suffix('.stderr').open('wb') as stderr:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, timeout=55)
        prefix.with_suffix('.status').write_text(json.dumps(done.returncode))
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--hirsel', required=True, type=Path)
        parser.add_argument('--root', required=True, type=Path)
        parser.add_argument('--output', required=True, type=Path)
        args = parser.parse_args()
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
