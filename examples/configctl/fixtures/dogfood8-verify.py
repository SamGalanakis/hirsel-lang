#!/usr/bin/env python3
"""Independent fixture oracles and build-once cross-engine recording checks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(hirsel, root, output, group):
    output.mkdir(parents=True, exist_ok=False)
    inputs = output / 'configs'
    shutil.copytree(root / 'fixtures', inputs)
    selection = root / 'hirsel.json'
    records = []
    frozen_paths = [*root.rglob('*'), hirsel]
    if os.environ.get('HIRSEL_IMAGE'):
        frozen_paths.append(Path(os.environ['HIRSEL_IMAGE']))
    manifest = json.loads(selection.read_text())
    std = (root / manifest['dependencies'][0]['manifest']).resolve().parent
    frozen_paths.extend(std.rglob('*'))
    for name in ('HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX'):
        if os.environ.get(name):
            frozen_paths.extend(Path(os.environ[name]).rglob('*'))
    frozen = {str(p): digest(p) for p in frozen_paths if p.is_file()}
    (output / 'frozen.json').write_text(json.dumps(frozen, indent=2) + '\n')
    original = {str(p.relative_to(inputs)): digest(p) for p in inputs.rglob('*') if p.is_file()}
    commands_path = output / 'commands.json'
    binaries = {}
    builds = {'llvm': 0, 'cranelift': 0}
    result = {'status': 'running', 'group': group, 'observations': 0, 'builds': builds,
              'package_executions': 0, 'recordings': 0, 'replays': 0}

    def run(label, argv):
        started = time.monotonic()
        command = list(map(str, argv))
        done = subprocess.run(command, capture_output=True, stdin=subprocess.DEVNULL)
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        records.append(dict(label=label, argv=command, cwd=str(Path.cwd()),
                            exit=done.returncode, seconds=time.monotonic() - started,
                            stdout_sha256=hashlib.sha256(done.stdout).hexdigest(),
                            stderr_sha256=hashlib.sha256(done.stderr).hexdigest()))
        commands_path.write_text(json.dumps(records, indent=2) + '\n')
        return done.returncode, done.stdout, done.stderr

    def expect(label, argv, triple):
        actual = run(label, argv)
        assert actual == triple, (label, actual, triple)
        result['observations'] += 1

    def command(engine, args, grant, record=None):
        if engine == 'h':
            return [hirsel, 'run', selection, '--grants', grant,
                    *(['--record', record] if record else []), '--', *args]
        return [binaries[engine], *(['--hirsel-record', record] if record else []),
                '--hirsel-grants', grant, *args]

    try:
        # Package tests are separate from observations; confirm actual execution.
        if group in ('all', 'replay'):
            for engine in ('h', 'llvm', 'cranelift'):
                actual = run('package-' + engine, [hirsel, 'test', selection,
                             *([] if engine == 'h' else ['--native', '--backend', engine])])
                assert actual[0] == 0 and b'3 passed' in actual[1] and b'0 failed' in actual[1], actual
                result['package_executions'] += 3
        for engine in builds:
            binary = output / ('configctl-' + engine)
            actual = run('build-' + engine, [hirsel, 'build', selection, '--backend', engine, '-o', binary])
            assert actual[0] == 0, actual
            builds[engine] += 1
            binaries[engine] = binary
        binary_hashes = {str(p): digest(p) for p in binaries.values()}
        (output / 'binaries.json').write_text(json.dumps(binary_hashes, indent=2) + '\n')
        grants = output / 'grants.json'
        grants.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1', 'dirs': [
            {'path': 'configs', 'host': str(inputs), 'access': 'read'}]}, 'args': []}) + '\n')
        denied = output / 'denied.json'
        denied.write_text('{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}\n')
        live_hashes = {str(p): digest(p) for p in (grants, denied)}
        (output / 'grants-hashes.json').write_text(json.dumps(live_hashes, indent=2) + '\n')
        casefile = lambda name: 'dogfood8-' + name + '.json'
        expected = lambda name: (root / 'fixtures' / ('dogfood8-' + name + '.out')).read_bytes()
        zero, ten, v2 = map(casefile, ('v1-zero', 'v1-ten', 'v2'))
        cases = [
            ('root-help', ['--help'], expected('help')),
            ('root-short-help', ['-h'], expected('help')),
            *[(c + '-help', [c, '--help'], expected(c + '-help')) for c in ('validate', 'migrate', 'diff')],
            ('validate-zero', ['validate', zero], b''),
            ('validate-ten', ['validate', ten], b''),
            ('validate-v2', ['validate', v2], b''),
            ('dash-file', ['validate', '--', '-dogfood8-name.json'], b''),
            ('migrate-zero', ['migrate', zero, '--to', '2'], expected('migrated-zero')),
            ('migrate-ten', ['migrate', '--to', '2', ten], expected('migrated-ten')),
            ('migrate-v2', ['migrate', v2, '--to=2'], expected('migrated-ten')),
            ('migrate-dash', ['migrate', '--to', '2', '--', '-dogfood8-name.json'], expected('migrated-zero')),
            ('diff-change', ['diff', zero, v2], expected('diff')),
            ('diff-equal', ['diff', zero, casefile('v2-equal')], expected('equal')),
        ]
        usage = [[], ['unknown'], ['validate'], ['migrate'], ['diff'], ['diff', zero],
                 ['validate', zero, ten], ['diff', zero, ten, v2],
                 ['migrate', zero], ['migrate', zero, '--to'],
                 ['migrate', zero, '--to', '1'], ['migrate', zero, '--to', '3'],
                 ['migrate', zero, '--to', '2', '--to', '2'],
                 ['validate', '--wat', zero], ['validate', '-dogfood8-name.json'],
                 ['--help', '--help'], ['validate', '--help', '--help'],
                 ['migrate', '--to', '2', '--', zero, '--help']]
        invalid = [('invalid11', 'RetryRange'), ('empty', 'EmptyName'),
                   ('duplicate', 'InvalidJson'), ('unknown', 'InvalidShape'),
                   ('version', 'UnknownVersion'), ('missing', 'InvalidShape'),
                   ('type', 'InvalidShape'), ('malformed', 'InvalidJson')]
        # Independent exact-byte cap controls exercise the filesystem EOF probe.
        (inputs / 'at-cap.json').write_bytes((inputs / zero).read_bytes().rstrip() +
                                             b' ' * (65536 - len((inputs / zero).read_bytes().rstrip())))
        (inputs / 'over-cap.json').write_bytes((inputs / 'at-cap.json').read_bytes() + b' ')
        original.update({name: digest(inputs / name) for name in ('at-cap.json', 'over-cap.json')})
        (output / 'input-hashes.json').write_text(json.dumps(original, indent=2) + '\n')
        for engine in ('h', 'llvm', 'cranelift'):
            for label, args, stdout in cases:
                is_help = label.endswith('help')
                if group not in ('all', 'cli' if is_help else 'data'):
                    continue
                expect(engine + '-' + label, command(engine, args, grants), (0, stdout, b''))
            for n, args in enumerate(usage if group in ('all', 'cli') else []):
                expect(engine + '-usage-' + str(n), command(engine, args, grants), (1, b'', b'error: Usage\n'))
            for name, error in (invalid if group in ('all', 'data') else []):
                expect(engine + '-invalid-' + name, command(engine, ['validate', casefile(name)], grants),
                       (1, b'', ('error: Config(' + error + ')\n').encode()))
            if group in ('all', 'data'):
                expect(engine + '-denied', command(engine, ['validate', zero], denied),
                       (1, b'', b'error: File(NotGranted)\n'))
                expect(engine + '-missing-file', command(engine, ['validate', 'absent.json'], grants),
                       (1, b'', b'error: Read\n'))
                expect(engine + '-exact-cap', command(engine, ['validate', 'at-cap.json'], grants), (0, b'', b''))
                expect(engine + '-over-cap', command(engine, ['validate', 'over-cap.json'], grants),
                       (1, b'', b'error: Limit\n'))
        tapes = []
        for engine in (('h', 'llvm', 'cranelift') if group in ('all', 'replay') else []):
            for name, args, stdout in [('migration', ['migrate', zero, '--to', '2'], expected('migrated-zero')),
                                       ('diff', ['diff', zero, v2], expected('diff'))]:
                tape = output / (engine + '-' + name + '.tape')
                expect('record-' + engine + '-' + name, command(engine, args, grants, tape), (0, stdout, b''))
                tapes.append((tape, stdout))
                result['recordings'] += 1
        # Prove immutability before any intentional host renames.
        assert original == {str(p.relative_to(inputs)): digest(p) for p in inputs.rglob('*') if p.is_file()}
        assert live_hashes == {str(p): digest(Path(p)) for p in live_hashes}
        inputs.rename(output / 'retired-configs')
        inputs.mkdir()
        for name in original:
            (inputs / name).write_bytes(b'REPLAY MUST NOT READ THIS HOST SENTINEL\n')
        sentinels = {str(p): digest(p) for p in inputs.iterdir()}
        grants.rename(output / 'retired-grants.json')
        denied.rename(output / 'retired-denied.json')
        tape_hashes = {str(p): digest(p) for p, _ in tapes}
        (output / 'tapes.json').write_text(json.dumps(tape_hashes, indent=2) + '\n')
        for tape, stdout in tapes:
            expect('replay-h-' + tape.stem, [hirsel, 'run', selection, '--replay', tape], (0, stdout, b''))
            result['replays'] += 1
            for engine in binaries:
                expect('replay-' + engine + '-' + tape.stem,
                       [binaries[engine], '--hirsel-replay', tape], (0, stdout, b''))
                result['replays'] += 1
        assert sentinels == {str(p): digest(Path(p)) for p in sentinels}
        assert tape_hashes == {str(p): digest(Path(p)) for p in tape_hashes}
        assert binary_hashes == {str(p): digest(Path(p)) for p in binary_hashes}
        expected_counts = {'all': 159, 'cli': 69, 'data': 66, 'replay': 24}
        assert result['observations'] == expected_counts[group], result
        assert builds == {'llvm': 1, 'cranelift': 1}, result
        result['status'] = 'passed'
        print(str(result['package_executions']) + ' package test executions; ' +
              str(result['observations']) + ' exact observations; ' +
              str(result['recordings']) + ' records; ' + str(result['replays']) +
              ' replays; builds=' + str(builds))
    finally:
        if result['status'] == 'running':
            result['status'] = 'failed'
        unchanged = all(Path(p).is_file() and digest(Path(p)) == h for p, h in frozen.items())
        result['frozen_inputs_unchanged'] = unchanged
        (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        assert unchanged, 'consumed source/toolchain inputs changed'


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--group', choices=('all', 'cli', 'data', 'replay'), default='all')
    options = parser.parse_args()
    os.environ['HIRSEL_JOBS'] = '1'
    os.environ['HIRSEL_DAEMON'] = 'off'
    verify(options.hirsel.resolve(), options.root.resolve(), options.output.resolve(), options.group)
