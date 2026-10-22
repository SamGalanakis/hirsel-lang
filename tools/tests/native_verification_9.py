#!/usr/bin/env python3
"""One checked recipe, two builds, unchanged observation assertions on three engines."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BACKENDS = ('llvm', 'cranelift')


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')


class Recipe:
    def __init__(self, compiler):
        self.compiler = compiler.absolute()
        self.scratch = Path(tempfile.mkdtemp(prefix='native-verification-9-')).resolve()
        print(f'evidence: {self.scratch}', flush=True)
        self.env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_JOBS': '1',
                                 'HIRSEL_RESULTS': 'none@1',
                                 'HIRSEL_NATIVE_CACHE': str(self.scratch / 'native-cache')}
        self.builds = dict.fromkeys(BACKENDS, 0)
        self.binaries = {}
        self.observations = 0
        self.commands = 0
        self.phase = 'setup'
        self.frozen = {}
        self.launch_inputs = {}

    def command(self, name, argv):
        argv = list(map(str, argv))
        if argv[0] == str(self.compiler):
            assert '--native' not in argv, 'observations must never compile'
            if argv[1] == 'build' and '--help' not in argv:
                assert self.phase == 'build', 'build after observations began'
                backend = argv[argv.index('--backend') + 1]
                self.builds[backend] += 1
                assert self.builds[backend] == 1, f'repeated {backend} build'
        self.commands += 1
        stem = self.scratch / f'{self.commands:02d}-{name}'
        save(stem.with_suffix('.command.json'), {'argv': argv, 'cwd': str(self.scratch),
                                                'phase': self.phase})
        print(f'{name}: {shlex.join(argv)}', flush=True)
        with stem.with_suffix('.stdout').open('wb') as stdout, stem.with_suffix('.stderr').open('wb') as stderr:
            try:
                result = subprocess.run(argv, cwd=self.scratch, env=self.env,
                                        stdout=stdout, stderr=stderr, timeout=180)
            except subprocess.TimeoutExpired:
                stem.with_suffix('.exit').write_text('TIMEOUT\n')
                raise
        stem.with_suffix('.exit').write_text(f'{result.returncode}\n')
        return (result.returncode, stem.with_suffix('.stdout').read_bytes(),
                stem.with_suffix('.stderr').read_bytes())

    def freeze(self, paths):
        self.frozen = {str(p): digest(p) for p in sorted(set(paths))}
        save(self.scratch / 'inputs.sha256.json', self.frozen)

    def check_frozen(self, full=False):
        for path, expected in self.frozen.items():
            if full or path.startswith(str(self.scratch / 'examples')) or path.startswith(str(self.scratch / 'stdlib')):
                assert digest(Path(path)) == expected, f'input changed: {path}'
        for path, expected in self.launch_inputs.items():
            assert digest(path) == expected, f'launch input changed: {path}'
        for backend, (path, expected, mtime) in self.binaries.items():
            assert digest(path) == expected, f'{backend} executable changed'
            assert path.stat().st_mtime_ns == mtime, f'{backend} executable rewritten'

    def remember(self, paths):
        self.launch_inputs.update({p: digest(p) for p in paths})
        save(self.scratch / 'launch-inputs.sha256.json',
             {str(p): h for p, h in self.launch_inputs.items()})

    def retire(self, source, destination):
        self.check_frozen()
        source.rename(destination)
        self.launch_inputs = {
            destination / p.relative_to(source) if source in p.parents else
            destination if p == source else p: h for p, h in self.launch_inputs.items()
        }
        self.remember([])

    def observe(self, name, engine, flags, args, expected, tape=None, replay=False, record=None):
        self.check_frozen()
        if engine == 'interpreted':
            argv = [self.compiler, 'run', self.manifest]
            if replay:
                argv += ['--replay', tape]
            else:
                if record:
                    argv += ['--record', record]
                argv += flags
                # Bound interpreted records own their args; grants allow an override.
                if flags[0] == '--grants':
                    argv += ['--', *args]
        else:
            argv = [self.binaries[engine][0]]
            if replay:
                argv += ['--hirsel-replay', tape]
            else:
                if record:
                    argv += ['--hirsel-record', record]
                argv += ['--hirsel-' + flags[0][2:], flags[1], *args]
        actual = self.command(f'{name}-{engine}', argv)
        self.check_frozen()
        assert actual == expected, f'{name}/{engine}: expected {expected!r}, got {actual!r}'
        self.observations += 1

    def run(self):
        self.compiler = self.compiler.resolve(strict=True)
        fixture = ROOT / 'examples/native-verification-9'
        copied = self.scratch / 'examples/native-verification-9'
        copied.parent.mkdir()
        shutil.copytree(fixture, copied)
        shutil.copytree(ROOT / 'stdlib', self.scratch / 'stdlib')
        self.manifest = copied / 'hirsel.json'
        inputs = [p for base in (fixture, ROOT / 'stdlib', copied, self.scratch / 'stdlib')
                  for p in base.rglob('*') if p.is_file()]
        inputs += [Path(__file__).resolve(), ROOT / 'docs/native-verification.md', self.compiler]
        # Freeze the actual link inputs as well as the program and its dependencies.
        for key in ('HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX', 'HIRSEL_IMAGE'):
            if self.env.get(key):
                path = Path(self.env[key]).resolve(strict=True)
                inputs += [path] if path.is_file() else [p for p in path.rglob('*') if p.is_file()]
        self.freeze(inputs)
        save(self.scratch / 'environment.json', {k: v for k, v in self.env.items()
                                               if k.startswith(('HIRSEL_', 'LLVM_SYS_'))})
        code, stdout, stderr = self.command('help', [self.compiler, '--help'])
        assert code == 0, (stdout, stderr)
        for option in ('build', 'run', 'export', '--backend', '-o', '--grants', '--record', '--replay', '--run'):
            assert option.encode() in stdout + stderr, f'missing CLI option {option}'

        self.phase = 'build'
        for backend in BACKENDS:
            binary = self.scratch / f'recipe-{backend}'
            result = self.command(f'build-{backend}', [self.compiler, 'build', self.manifest,
                                                      '--backend', backend, '-o', binary])
            assert result[0] == 0, result
            self.binaries[backend] = (binary, digest(binary), binary.stat().st_mtime_ns)
        self.phase = 'observations'
        self.check_frozen(full=True)
        save(self.scratch / 'binaries.json', {b: {'path': str(p), 'sha256': h, 'mtime_ns': m}
                                             for b, (p, h, m) in self.binaries.items()})

        for name, text in (('data', 'hello\n'), ('other-data', 'other\n')):
            directory = self.scratch / name
            directory.mkdir()
            (directory / 'input.txt').write_text(text)
        grants = self.scratch / 'grants.json'
        other = self.scratch / 'other.json'
        denied = self.scratch / 'denied.json'
        def policy(host):
            return {'io': {'stdout': 'stdout.bytes@1', 'dirs': [
                {'path': 'data', 'host': host, 'access': 'read'}]}, 'args': []}
        save(grants, policy('data'))  # Relative to grants.json, not the launch cwd.
        save(other, policy('other-data'))
        save(denied, {'io': {'stdout': 'stdout.bytes@1', 'dirs': []}, 'args': []})
        # An existing host file is not authority. Denied launches must never print it.
        sentinel = self.scratch / 'input.txt'
        sentinel.write_text('HOST-DENIAL-SENTINEL\n')
        sentinel_hash = digest(sentinel)
        self.remember([grants, other, denied, sentinel, self.scratch / 'data/input.txt',
                       self.scratch / 'other-data/input.txt'])
        alpha = (0, b'alpha:hello\n', b'')
        engines = ('interpreted', *BACKENDS)
        cases = (
            ('success', grants, ['alpha'], alpha),
            ('different-args', grants, ['two words'], (0, b'two words:hello\n', b'')),
            ('different-grants', other, ['beta'], (0, b'beta:other\n', b'')),
            ('denied', denied, ['alpha'], (1, b'', b'error: Files(NotGranted)\n')),
            ('usage', grants, [], (1, b'', b'error: Usage\n')),
        )
        for name, grant, args, expected in cases:
            for engine in engines:
                self.observe(name, engine, ['--grants', grant], args, expected)

        # Keep the exact semantic program identity and canonical absolute run path visible.
        run = self.scratch / 'run.json'
        result = self.command('export-run', [self.compiler, 'export', '--run', self.manifest, '-o', run])
        assert result[0] == 0, result
        bound = json.loads(run.read_bytes())
        bound.update(policy(str(self.scratch / 'data')))
        bound['args'] = ['alpha']
        save(run, bound)
        self.remember([run])
        print(f'bound run: {run}\nprogram: {bound["program"]}', flush=True)
        for engine in engines:
            self.observe('bound-run', engine, ['--run', run], ['alpha'], alpha)

        tapes = []
        for engine in engines:
            tape = self.scratch / f'{engine}.tape'
            self.observe('record', engine, ['--grants', grants], ['alpha'], alpha, record=tape)
            assert tape.is_file() and tape.stat().st_size > 0, 'record did not create a tape'
            self.remember([tape])
            tapes.append((tape, digest(tape)))

        # Remove host access even as root: no chmod-based test. Preserve bytes as evidence
        # under an ungranted name, then test denial before removing the grant files too.
        self.retire(self.scratch / 'data', self.scratch / 'retired-data')
        self.retire(self.scratch / 'other-data', self.scratch / 'retired-other-data')
        for engine in engines:
            self.observe('host-denial-sentinel', engine, ['--grants', denied], ['alpha'],
                         (1, b'', b'error: Files(NotGranted)\n'))
        for path in (grants, other, denied, run):
            self.retire(path, path.with_suffix('.retired.json'))
        assert not (self.scratch / 'data').exists() and not grants.exists() and not run.exists()
        for tape, tape_hash in tapes:
            for engine in engines:
                self.observe(f'replay-{tape.stem}', engine, [], [], alpha, tape=tape, replay=True)
            assert digest(tape) == tape_hash, 'replay rewrote tape'
        assert digest(sentinel) == sentinel_hash, 'denied sentinel changed'
        assert self.builds == {'llvm': 1, 'cranelift': 1}, self.builds
        assert self.observations == 33, f'zero or incomplete selection: {self.observations}'
        self.check_frozen(full=True)
        self.phase = 'passed'

    def report(self, error=None):
        identity_error = None
        try:
            self.check_frozen(full=True)
        except (AssertionError, OSError) as failure:
            identity_error = str(failure)
        save(self.scratch / 'result.json', {'phase': self.phase, 'error': error,
                                          'identity_error': identity_error,
                                          'builds': self.builds, 'observations': self.observations,
                                          'commands': self.commands})
        assert identity_error is None, identity_error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path, help='matching compiler with native toolchain configured')
    args = parser.parse_args()
    if args.hirsel is None:
        print('SETUP ONLY: 0 recipe tests, 0 observations. Run root//:test-native-verification-9 '
              'or pass --hirsel /absolute/path/to/hirsel; this is not a recipe pass.')
        return 0
    recipe = Recipe(args.hirsel)
    try:
        recipe.run()
    except BaseException as error:
        recipe.report(f'{type(error).__name__}: {error}')
        print(f'FAIL ({recipe.phase}): retained {recipe.scratch}', file=sys.stderr)
        raise
    recipe.report()
    print(f'PASS: 1 recipe test, {recipe.observations} observations; builds={recipe.builds}; '
          f'evidence retained: {recipe.scratch}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
