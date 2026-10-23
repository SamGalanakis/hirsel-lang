#!/usr/bin/env python3
"""Independent undirected reachability oracle and bounded three-engine workflows.

Vectors adapt rustc union_find/tests.rs:1-32 at
15c10447a46529b68e38c7f138aeb95240e4d7d6. MIT, Copyright The Rust
Project Contributors; license in third-party/NOTICES.md. Dense string names
replace typed IDs; public labels are first declarations, never forest roots.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def invalid(error, name=''):
    return canonical(dict(error=error, name=name))


def oracle(inventory):
    # Adjacency traversal, independent of disjoint-set representatives and sorting.
    names = [record['name'] for record in inventory['records']]
    adjacent = {name: set() for name in names}
    for edge in inventory['aliases']:
        adjacent[edge['left']].add(edge['right'])
        adjacent[edge['right']].add(edge['left'])
    seen = set()
    components = []
    for first in names:
        if first in seen:
            continue
        reached, pending = {first}, [first]
        while pending:
            for neighbor in adjacent[pending.pop()]:
                if neighbor not in reached:
                    reached.add(neighbor)
                    pending.append(neighbor)
        seen |= reached
        components.append(dict(label=first, members=[name for name in names if name in reached]))
    return 0, canonical(dict(components=components)), b''


def inventory(names, edges=()):
    return dict(records=[dict(name=name) for name in names],
                aliases=[dict(left=a, right=b) for a, b in edges])


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'input'
    data.mkdir()
    grants = output / 'grants.json'
    selection = root / 'hirsel.json'
    commands = []
    policy = json.loads((root / 'grants.json').read_text())
    assert set(policy) == {'io', 'args'} and policy['args'] == []
    assert set(policy['io']) == {'stdout', 'stderr', 'dirs'}
    assert policy['io']['stdout'] == 'stdout.bytes@1'
    assert policy['io']['stderr'] == 'stderr.bytes@1'
    assert policy['io']['dirs'] == [dict(path='input', host='fixtures', access='read')]
    policy['io']['dirs'][0]['host'] = str(data.resolve())

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout = output / (label + '.stdout')
        stderr = output / (label + '.stderr')
        env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                                HIRSEL_MEMORY_LIMIT='2G', HIRSEL_JOBS='1')
        if runtime:
            # Separate the tool's stderr from capped's resource report. One scope
            # and one CLI/native fixture at a time, with a 60-second runtime cap.
            wrapper = ('import subprocess,sys; '
                       'o=open(sys.argv[1],"wb"); e=open(sys.argv[2],"wb"); '
                       'p=subprocess.run(sys.argv[3:],stdin=subprocess.DEVNULL,stdout=o,stderr=e); '
                       'sys.exit(p.returncode)')
            done = subprocess.run([sys.executable, str(root / 'capped'), '--timeout', '60',
                                   sys.executable, '-c', wrapper, str(stdout), str(stderr), *argv],
                                  capture_output=True, timeout=90, env=env)
            (output / (label + '.scope')).write_bytes(done.stdout + done.stderr)
            actual = done.returncode, stdout.read_bytes(), stderr.read_bytes()
        else:
            done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                  timeout=300, env=env)
            stdout.write_bytes(done.stdout)
            stderr.write_bytes(done.stderr)
            actual = done.returncode, done.stdout, done.stderr
        commands.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return actual

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = exe

    cases = []

    def add(label, raw, expected, nodes=None, grant=None):
        (data / (label + '.json')).write_bytes(raw)
        cases.append((label, [label + '.json'], expected, nodes, grant))

    def graph(label, value, expected_components=None):
        expected = oracle(value)
        if expected_components is not None:
            assert json.loads(expected[1])['components'] == expected_components
        add(label, canonical(value), expected, value)

    for vector in json.loads((root / 'fixtures/vectors.json').read_text()):
        value = vector['inventory']
        graph(vector['label'], value, vector['components'])
        reversed_value = value | dict(aliases=[dict(left=e['right'], right=e['left'])
                                               for e in reversed(value['aliases'])])
        assert oracle(value) == oracle(reversed_value)
        graph(vector['label'] + '-reversed', reversed_value, vector['components'])
    for label in ('inventory', 'invalid'):
        raw = (root / ('fixtures/' + label + '.json')).read_bytes()
        value = json.loads(raw)
        add(label, raw, oracle(value) if label == 'inventory' else
            (2, b'', invalid('unknown_name', 'vendor:hex-8')), value if label == 'inventory' else None)
    graph('empty', inventory([]))
    graph('record-exact', inventory([str(i) for i in range(64)]))
    graph('aliases-exact', inventory(['a', 'b'], [('a', 'b')] * 64))
    graph('name-exact', inventory(['x' * 128, 'é' * 64]))
    graph('name-escaping', inventory(['quote"', 'line\n', 'slash\\', '雪'], [('雪', 'quote"')]))
    graph('independent', inventory(['z', 'a', 'b', 'c', 'solo'], [('b', 'z'), ('c', 'a')]))
    for label, value, error, name in [
        ('record-over', inventory([str(i) for i in range(65)]), 'record_limit', ''),
        ('aliases-over', inventory(['a'], [('a', 'a')] * 65), 'alias_limit', ''),
        ('name-over', inventory(['x' * 129]), 'name', ''),
        ('name-utf8-over', inventory(['é' * 65]), 'name', ''),
        ('name-empty', inventory(['']), 'name', ''),
        ('reference-over', inventory(['a'], [('a', 'x' * 129)]), 'name', ''),
        ('duplicate', inventory(['a', 'a']), 'duplicate_name', 'a'),
        ('unknown-left', inventory(['a'], [('missing', 'a')]), 'unknown_name', 'missing'),
        ('unknown-right', inventory(['a'], [('a', 'missing')]), 'unknown_name', 'missing'),
    ]:
        add(label, canonical(value), (2, b'', invalid(error, name)))
    for label, raw, error in [
        ('malformed', b'{"records":[', 'json'),
        ('duplicate-key', b'{"records":[],"records":[],"aliases":[]}', 'json'),
        ('invalid-utf8', b'\xff', 'json'),
        ('shape', b'{"records":[],"aliases":false}', 'shape'),
        ('missing-name', b'{"records":[{}],"aliases":[]}', 'shape'),
        ('missing-records', b'{"aliases":[]}', 'shape'),
        ('missing-aliases', b'{"records":[]}', 'shape'),
        ('alias-shape', b'{"records":[],"aliases":[{"left":"a"}]}', 'shape'),
        ('wrong-root', b'[]', 'shape'),
        ('trailing', b'{"records":[],"aliases":[]} false', 'json'),
        ('oversized', b' ' * 65537, 'input_bytes_limit'),
    ]:
        add(label, raw, (2, b'', invalid(error)))
    exact = canonical(inventory([])).rstrip(b'\n')
    add('bytes-exact', exact + b' ' * (65536 - len(exact)), oracle(inventory([])))
    graph('unknown-fields', inventory(['a']) | dict(comment='ignored'))
    cases.append(('missing-directory', ['inventory.json'], (2, b'', invalid('directory_grant')),
                  None, dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[]), args=[])))
    cases.append(('missing-file', ['absent.json'], (2, b'', invalid('file_open')), None, None))
    cases.append(('escape', ['../sentinel'], (2, b'', invalid('file_open')), None, None))
    cases.append(('usage', [], (2, b'', invalid('usage')), None, None))
    cases.append(('extra-arg', ['inventory.json', 'extra'], (2, b'', invalid('usage')), None, None))
    cases.append(('missing-stdout', ['inventory.json'], (2, b'', b''), None,
                  dict(io=dict(stderr='stderr.bytes@1', dirs=[]), args=[])))
    cases.append(('missing-stderr', ['invalid.json'], (2, b'', b''), None,
                  dict(io=dict(stdout='stdout.bytes@1', dirs=policy['io']['dirs']), args=[])))
    help_bytes = (b'groupmap FILE\nGroup explicitly aliased inventory records; labels follow declaration order.\n'
                  b'Reads FILE beneath the read-only input directory grant.\n'
                  b'Limits: 65536 bytes, 64 records, 64 aliases, 128 bytes per name.\n'
                  b'Exit: 0 grouped/help, 2 invalid input or I/O.\n')
    cases.append(('help', ['--help'], (0, help_bytes, b''), None,
                  dict(io=dict(stdout='stdout.bytes@1', dirs=[]), args=[])))
    (output / 'sentinel').write_bytes(b'outside grant\n')

    def snapshot():
        return {str(p.relative_to(data)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in data.rglob('*') if p.is_file()}

    before = snapshot()
    executed = 0
    for engine in ('interpreter', 'llvm', 'cranelift'):
        for label, args, expected, nodes, grant in cases:
            grants.write_text(json.dumps(policy if grant is None else grant))
            command = ([hirsel, 'run', selection, '--grants', grants, '--', *args]
                       if engine == 'interpreter' else
                       [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(engine + '-' + label, command, runtime=True)
            assert actual == expected, (engine, label, actual, expected)
            if nodes is not None and actual[0] == 0:
                assert actual == oracle(nodes)
            assert snapshot() == before, (engine, label, 'fixture mutation')
            assert (output / 'sentinel').read_bytes() == b'outside grant\n'
            executed += 1
        # Repeat one realistic input to check deterministic bytes in each engine.
        grants.write_text(json.dumps(policy))
        command = ([hirsel, 'run', selection, '--grants', grants, '--', 'inventory.json']
                   if engine == 'interpreter' else
                   [binaries[engine], '--hirsel-grants', grants, 'inventory.json'])
        assert run(engine + '-repeat', command, runtime=True) == oracle(json.loads((data / 'inventory.json').read_text()))
        executed += 1
    for file in data.iterdir():
        file.unlink()
    data.rmdir()
    grants.unlink()
    (output / 'sentinel').unlink()
    assert not data.exists() and not grants.exists() and not (output / 'sentinel').exists()
    print(json.dumps(dict(cases=len(cases), engines=3, executed=executed, cleanup=True)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
