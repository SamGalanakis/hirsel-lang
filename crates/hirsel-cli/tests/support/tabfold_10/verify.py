#!/usr/bin/env python3
"""Private correctness verifier; literal byte goldens, bounded inputs, offline replay.

Indexed success/zero/single/error controls adapt Go x/sync 1f920df1
errgroup/errgroup_test.go:65–136 (BSD-3-Clause, Go Authors). Hirsel's
completion error is observed rather than assuming an operand-order race winner.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(hirsel, root, output, prebuilt=None):
    output.mkdir(parents=True, exist_ok=False)
    fixtures, selection = root / 'fixtures', root / 'hirsel.json'
    data = output / 'data'
    data.mkdir()
    for path in fixtures.glob('*.tsv'):
        (data / path.name).write_bytes(path.read_bytes())
    # Boundary vectors constructed independently of tabfold's parser/reducer.
    vectors = {'records-cap': b'a\t1\n' * 1024, 'records-over': b'a\t1\n' * 1025,
               'bytes-cap': b'a' * 128 + b'\t' + b'0' * (65536 - 130) + b'\n',
               'bytes-over': b'a' * 128 + b'\t' + b'0' * (65537 - 130) + b'\n',
               'key-cap': ('é' * 64 + '\t-0\n').encode(),
               'key-over': ('é' * 64 + 'a\t0\n').encode(),
               'blank': b'\n', 'empty-key': b'\t1\n', 'empty-int': b'a\t\n',
               'extra-tab': b'a\t1\t2\n', 'bare-cr': b'a\t1\r',
               'space': b'a\t 1\n', 'fraction': b'a\t1.0\n', 'sign': b'a\t+\n',
               'hi': b'a\t1000000001\n', 'lo': b'a\t-1000000001\n',
               'overflow-int': b'a\t99999999999999999999999\n',
               'number-utf8': b'a\t\xff\n', '-name': b'a\t+0001\n'}
    for name, value in vectors.items():
        (data / name).write_bytes(value)
    original = {p.name: digest(p) for p in data.iterdir()}
    grants = output / 'grants.json'
    policy = {'io': {'stdout': 'stdout.bytes@1', 'tasks': 'tasks.cooperative@1',
                     'dirs': [{'path': 'data', 'host': str(data), 'access': 'read'}]}, 'args': []}
    grants.write_text(json.dumps(policy))
    policies = [grants]
    for name, field in [('no-tasks', 'tasks'), ('no-stdout', 'stdout'), ('no-dir', 'dirs')]:
        p = output / (name + '.json')
        copy = json.loads(json.dumps(policy))
        if field == 'dirs':
            copy['io']['dirs'] = []
        else:
            del copy['io'][field]
        p.write_text(json.dumps(copy))
        policies.append(p)
    frozen_paths = [p for p in root.rglob('*') if p.is_file()] + [hirsel, Path(__file__)]
    std = (root / json.loads(selection.read_text())['dependencies'][0]['manifest']).resolve().parent
    frozen_paths += [p for p in std.rglob('*') if p.is_file()]
    for key in ('HIRSEL_PRODUCTS', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX'):
        if os.environ.get(key):
            p = Path(os.environ[key])
            frozen_paths += [q for q in p.rglob('*') if q.is_file()] if p.is_dir() else [p]
    frozen = {str(p): digest(p) for p in frozen_paths}
    (output / 'frozen.json').write_text(json.dumps(frozen, indent=2))
    commands, binaries, tapes = [], {}, []
    result = {'status': 'running', 'observations': 0, 'recordings': 0, 'replays': 0,
              'builds': {'llvm': 0, 'cranelift': 0}, 'provider': 'tasks.cooperative@1',
              'worker_permits': 'ordinary shipping defaults; no mc opt-in'}

    def run(label, argv):
        argv = list(map(str, argv))
        done = subprocess.run(argv, capture_output=True, stdin=subprocess.DEVNULL, timeout=240)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        commands.append({'label': label, 'argv': argv, 'cwd': str(Path.cwd()), 'exit': done.returncode})
        (output / 'commands.json').write_text(json.dumps(commands, indent=2))
        return done.returncode, done.stdout, done.stderr

    def expect(label, argv, expected):
        actual = run(label, argv)
        assert actual == expected, (label, actual, expected, str(output))
        result['observations'] += 1
        return actual

    def command(engine, args, policy=grants, tape=None):
        if engine == 'h':
            return [hirsel, 'run', selection, '--grants', policy,
                    *(['--record', tape] if tape else []), '--', *args]
        return [binaries[engine], *(['--hirsel-record', tape] if tape else []),
                '--hirsel-grants', policy, '--', *args]

    def fail(error):
        return 1, b'', f'error: {error}\n'.encode()

    def bad(reason, ordinal=1, line=1):
        return fail(f'Input({{ ordinal: {ordinal}, line: {line}, reason: {reason} }})')

    def golden(name):
        return 0, (fixtures / (name + '.csv')).read_bytes(), b''

    try:
        for backend in result['builds']:
            binary = (prebuilt or output) / ('tabfold-' + backend)
            if prebuilt:
                assert binary.is_file(), binary
                binaries[backend] = binary
                continue
            actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', binary])
            assert actual[0] == 0, ('build', backend, actual)
            binaries[backend] = binary
            result['builds'][backend] += 1
        identities = {str(p): digest(p) for p in [*binaries.values(), *policies]}
        cases = [('empty', ['empty.tsv'], golden('empty')),
                 ('single', ['first.tsv'], golden('first')),
                 ('combined', ['first.tsv', 'second.tsv'], golden('combined')),
                 ('reverse', ['second.tsv', 'first.tsv'], golden('combined')),
                 ('permuted', ['permuted.tsv', 'second.tsv'], golden('combined')),
                 ('batches', ['empty.tsv', 'first.tsv', 'empty.tsv', 'second.tsv', 'empty.tsv'], golden('combined')),
                 ('32', ['empty.tsv'] * 32, golden('empty')),
                 ('records-cap', ['records-cap'], (0, b'key,count,sum,min,max\na,1024,1024,1,1\n', b'')),
                 ('bytes-cap', ['bytes-cap'], (0, b'key,count,sum,min,max\n' + b'a' * 128 + b',1,0,0,0\n', b'')),
                 ('key-cap', ['key-cap'], (0, ('key,count,sum,min,max\n' + 'é' * 64 + ',1,0,0,0\n').encode(), b'')),
                 ('records-over', ['records-over'], bad('RecordLimit', line=1025)),
                 ('bytes-over', ['bytes-over'], fail('Limit(1)')),
                 ('key-over', ['key-over'], bad('Key')),
                 ('bad-after-good', ['first.tsv', 'bad.tsv'], bad('Tab', ordinal=2, line=2)),
                 ('late-batch-failure', ['first.tsv', 'empty.tsv', 'empty.tsv', 'empty.tsv', 'bad.tsv'], bad('Tab', ordinal=5, line=2)),
                 ('utf8', ['utf8.tsv'], bad('Utf8', line=2)),
                 ('number-utf8', ['number-utf8'], bad('Utf8')),
                 ('missing', ['missing.tsv'], fail('File(1, NotFound)')),
                 ('denied-path', ['../escape'], fail('File(1, FileFault(1))'))]
        cases += [(name, [name], bad(reason)) for name, reason in [
            ('blank', 'Empty'), ('empty-key', 'Key'), ('empty-int', 'Integer'),
            ('extra-tab', 'Tab'), ('bare-cr', 'Tab'), ('space', 'Integer'),
            ('fraction', 'Integer'), ('sign', 'Integer'), ('hi', 'Integer'),
            ('lo', 'Integer'), ('overflow-int', 'Integer')]]
        for engine in ('h', 'llvm', 'cranelift'):
            for jobs in (1, 2, 4):
                for label, args, expected in cases:
                    expect(f'{engine}-j{jobs}-{label}', command(engine, ['--jobs', str(jobs), *args]), expected)
            expect(engine + '-default', command(engine, ['first.tsv', 'second.tsv']), golden('combined'))
            expect(engine + '-interspersed', command(engine, ['first.tsv', '--jobs=4', 'second.tsv']), golden('combined'))
            expect(engine + '-dash', command(engine, ['--', '-name']), (0, b'key,count,sum,min,max\na,1,1,1,1\n', b''))
            for n, args in enumerate([[], ['empty.tsv'] * 33, ['--jobs', '0', 'empty.tsv'],
                    ['--jobs', '5', 'empty.tsv'], ['--jobs', 'x', 'empty.tsv'],
                    ['--jobs', '1', '--jobs', '2', 'empty.tsv']]):
                expect(engine + '-usage-' + str(n), command(engine, args), fail('Usage'))
            expect(engine + '-flag-index', command(engine, ['empty.tsv', '--wat']), fail('Flags(UnknownFlag(1))'))
            expect(engine + '-missing-value', command(engine, ['empty.tsv', '--jobs']), fail('Flags(MissingValue(1))'))
            for name in ('no-tasks', 'no-stdout', 'no-dir'):
                expect(engine + '-' + name, command(engine, ['first.tsv'], output / (name + '.json')), fail('Directory(NotGranted)' if name == 'no-dir' else 'Grant(NotGranted)'))
            for name, args, expected in [('success', ['--jobs', '4', 'first.tsv', 'second.tsv'], golden('combined')),
                    ('failure', ['--jobs', '2', 'first.tsv', 'bad.tsv'], bad('Tab', ordinal=2, line=2))]:
                tape = output / (engine + '-' + name + '.tape')
                observed = expect('record-' + tape.stem, command(engine, args, tape=tape), expected)
                tapes.append((tape, observed))
                result['recordings'] += 1
            # Competing errors: assert either real observation, never invent a winner.
            actual = run(engine + '-competing', command(engine, ['--jobs', '2', 'bad.tsv', 'utf8.tsv']))
            assert actual in (bad('Tab', line=2), bad('Utf8', ordinal=2, line=2)), actual
            result['observations'] += 1
        assert {p.name: digest(p) for p in data.iterdir()} == original
        assert all(digest(Path(p)) == h for p, h in identities.items())
        data.rename(output / 'retired-data')
        for p in policies:
            p.rename(p.with_suffix('.retired.json'))
        tape_hashes = {str(p): digest(p) for p, _ in tapes}
        for tape, observed in tapes:
            for engine in ('h', 'llvm', 'cranelift'):
                argv = [hirsel, 'run', selection, '--replay', tape] if engine == 'h' else [binaries[engine], '--hirsel-replay', tape]
                expect('replay-' + engine + '-' + tape.stem, argv, observed)
                result['replays'] += 1
        assert {p.name: digest(p) for p in (output / 'retired-data').iterdir()} == original
        assert all(digest(Path(p)) == h for p, h in tape_hashes.items())
        assert all(digest(p) == identities[str(p)] for p in binaries.values())
        assert result['recordings'] == 6 and result['replays'] == 18
        assert result['observations'] == 339, result
        result['status'] = 'passed'
        print(f"{result['observations']} exact observations; 6 records; 18 replays; builds={result['builds']}")
    finally:
        unchanged = all(Path(p).is_file() and digest(Path(p)) == h for p, h in frozen.items())
        result['frozen_inputs_unchanged'] = unchanged
        result['input_hashes'] = original
        if result['status'] == 'running':
            result['status'] = 'failed'
        (output / 'result.json').write_text(json.dumps(result, indent=2))
        assert unchanged, 'source/toolchain changed'


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--prebuilt', type=Path, help='Reuse unchanged app executables after a checker-only failure')
    args = parser.parse_args()
    os.environ.update(HIRSEL_JOBS='1', HIRSEL_DAEMON='off')
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(),
           args.prebuilt.resolve() if args.prebuilt else None)
