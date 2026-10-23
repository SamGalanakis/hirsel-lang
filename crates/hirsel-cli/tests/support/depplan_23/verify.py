#!/usr/bin/env python3
"""Named graph oracle and actual CLI/interpreter/LLVM/Cranelift workflows.

Vectors adapt CPython Lib/test/test_graphlib.py:40-57 at
8b7d6c53eee2226b08bed761d5a1742b5adb8507; Copyright (c) 2001 Python
Software Foundation; All Rights Reserved. PSF-2.0: third-party/NOTICES.md.
Changes: explicit ordered string declarations, strict unknown/duplicate names,
raw-reference caps, full blocked sets. Algorithms remain in shipped std.
"""
import argparse
import graphlib
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def invalid(error, node='', reference='', blocked=()):
    return canonical(dict(error=error, node=node, reference=reference, blocked=list(blocked)))


def blocked_names(nodes):
    # Independent transitive closure: a vertex is blocked iff a cycle reaches it.
    names = [n['name'] for n in nodes]
    reach = {(p, n['name']) for n in nodes for p in n['prerequisites']}
    for k in names:
        reach |= {(a, b) for a in names for b in names if (a, k) in reach and (k, b) in reach}
    cycles = [n for n in names if (n, n) in reach]
    return [n for n in names if any((c, n) in reach for c in cycles)]


def verify_precedence(nodes, ordered):
    names = [n['name'] for n in nodes]
    assert len(ordered) == len(names) and set(ordered) == set(names)
    positions = {n: i for i, n in enumerate(ordered)}
    assert all(positions[p] < positions[n['name']] for n in nodes for p in n['prerequisites'])


def oracle(nodes):
    # The host's CPython, never Hirsel, computes expected success order.
    graph = {n['name']: list(dict.fromkeys(n['prerequisites'])) for n in nodes}
    try:
        sorter = graphlib.TopologicalSorter()
        for name in graph:
            sorter.add(name)
        for name, prerequisites in graph.items():
            sorter.add(name, *prerequisites)
        ordered = list(sorter.static_order())
    except graphlib.CycleError:
        blocked = blocked_names(nodes)
        assert blocked
        return 1, b'', invalid('blocked', blocked=blocked)
    verify_precedence(nodes, ordered)
    return 0, canonical(dict(order=ordered)), b''


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

    def graph(label, nodes, expected_order=None):
        expected = oracle(nodes)
        if expected_order is not None:
            assert expected == (0, canonical(dict(order=expected_order)), b'')
        add(label, canonical(dict(nodes=nodes)), expected, nodes)

    for vector in json.loads((root / 'fixtures/vectors.json').read_text()):
        graph(vector['label'], vector['nodes'], vector.get('order'))
    for label in ('release', 'invalid'):
        raw = (root / ('fixtures/' + label + '.json')).read_bytes()
        nodes = json.loads(raw)['nodes']
        if label == 'release':
            add(label, raw, oracle(nodes), nodes)
        else:
            add(label, raw, (2, b'', invalid('unknown_name', 'deploy', 'migrate')))
    graph('empty', [])
    graph('node-exact', [dict(name=str(i), prerequisites=[]) for i in range(64)])
    edge_nodes = [dict(name=str(i), prerequisites=[str(j) for j in range(i)]) for i in range(17)]
    edge_nodes[-1]['prerequisites'] = edge_nodes[-1]['prerequisites'][:8]
    assert sum(len(n['prerequisites']) for n in edge_nodes) == 128
    graph('edge-exact', edge_nodes)
    edge_nodes[-1]['prerequisites'].append('8')
    add('edge-over', canonical(dict(nodes=edge_nodes)), (2, b'', invalid('edge_limit')))
    add('node-over', canonical(dict(nodes=[dict(name=str(i), prerequisites=[]) for i in range(65)])),
        (2, b'', invalid('node_limit')))
    graph('references-exact', [dict(name='base', prerequisites=[]), dict(name='next', prerequisites=['base'] * 64)])
    add('references-over', canonical(dict(nodes=[dict(name='a', prerequisites=['a'] * 65)])),
        (2, b'', invalid('prerequisite_limit')))
    graph('name-exact', [dict(name='x' * 128, prerequisites=[])])
    for label, nodes, error in [
        ('name-over', [dict(name='x' * 129, prerequisites=[])], 'name'),
        ('name-empty', [dict(name='', prerequisites=[])], 'name'),
        ('duplicate', [dict(name='a', prerequisites=[])] * 2, 'duplicate_node'),
    ]:
        add(label, canonical(dict(nodes=nodes)), (2, b'', invalid(error, 'a' if label == 'duplicate' else '')))
    for label, raw, error in [
        ('malformed', b'{"nodes":[', 'json'),
        ('duplicate-key', b'{"nodes":[],"nodes":[]}', 'json'),
        ('invalid-utf8', b'\xff', 'json'),
        ('shape', b'{"nodes":[{"name":"a","prerequisites":false}]}', 'shape'),
        ('missing-name', b'{"nodes":[{"prerequisites":[]}]}', 'shape'),
        ('missing-nodes', b'{}', 'shape'),
        ('wrong-root', b'[]', 'shape'),
        ('trailing', b'{"nodes":[]} false', 'json'),
        ('oversized', b' ' * 65537, 'input_bytes_limit'),
    ]:
        add(label, raw, (2, b'', invalid(error)))
    exact = b'{"nodes":[]}'
    add('bytes-exact', exact + b' ' * (65536 - len(exact)), oracle([]), [])
    # Grants apply to host fixtures only. Missing grant is an application report.
    cases.append(('missing-directory', ['release.json'], (2, b'', invalid('directory_grant')),
                  None, dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1', dirs=[]), args=[])))
    cases.append(('missing-file', ['absent.json'], (2, b'', invalid('file_open')), None, None))
    cases.append(('escape', ['../sentinel'], (2, b'', invalid('file_open')), None, None))
    cases.append(('usage', [], (2, b'', invalid('usage')), None, None))
    cases.append(('missing-stdout', ['release.json'], (2, b'', b''), None,
                  dict(io=dict(stderr='stderr.bytes@1', dirs=[]), args=[])))
    cases.append(('missing-stderr', ['invalid.json'], (2, b'', b''), None,
                  dict(io=dict(stdout='stdout.bytes@1', dirs=policy['io']['dirs']), args=[])))
    help_bytes = (b'depplan FILE\nDry-run dependency planner; never executes nodes.\n'
                  b'Reads FILE beneath the read-only input directory grant.\n'
                  b'Limits: 65536 bytes, 64 nodes, 128 raw prerequisites, 64 per node, 128 bytes per name.\n'
                  b'Exit: 0 ordered/help, 1 blocked (includes downstream), 2 invalid input or I/O.\n')
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
                verify_precedence(nodes, json.loads(actual[1])['order'])
            assert snapshot() == before, (engine, label, 'fixture mutation')
            assert (output / 'sentinel').read_bytes() == b'outside grant\n'
            executed += 1
        # Repeat one realistic input to check deterministic bytes in each engine.
        grants.write_text(json.dumps(policy))
        command = ([hirsel, 'run', selection, '--grants', grants, '--', 'release.json']
                   if engine == 'interpreter' else
                   [binaries[engine], '--hirsel-grants', grants, 'release.json'])
        assert run(engine + '-repeat', command, runtime=True) == oracle(json.loads((data / 'release.json').read_text())['nodes'])
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
