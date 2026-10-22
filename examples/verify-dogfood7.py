#!/usr/bin/env python3
"""Independent hashlib oracle for assetcheck; retain exact bytes and commands.

No network. All fixtures, grants and tapes live in a new owned output directory.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time

ENGINES = ('h', 'llvm', 'cranelift')


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    manifests = output / 'manifests'
    assets = output / 'assets'
    manifests.mkdir()
    assets.mkdir()
    selection = root / 'hirsel.json'
    records = []
    fixture_bytes = {
        'empty': b'', 'short': b'a', 'binary.bin': b'\x00\xff\r\n',
        'boundary.bin': bytes(range(256)) * 16 + b'\xff',
        '日本.bin': 'snow: 雪\n'.encode(),
    }
    # Independent Go golden vectors; do not obtain expected hashes from Hirsel.
    assert hashlib.sha256(b'').hexdigest() == 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
    assert hashlib.sha256(b'a').hexdigest() == 'ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb'
    for name, data in fixture_bytes.items():
        (assets / name).write_bytes(data)
    (assets / 'over-cap').write_bytes(b'x' * 65537)
    (assets / 'at-cap').write_bytes(b'x' * 65536)
    sentinel = output / 'denied.bin'
    sentinel.write_bytes(b'denied sentinel\x00\xff')
    (assets / 'symlink').symlink_to(sentinel)
    original_tree = {p.name: p.read_bytes() for p in assets.iterdir() if not p.is_symlink()}

    def entry(name, data=None):
        data = fixture_bytes[name] if data is None else data
        return dict(path=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest())

    def finding(item, status='ok', data=None):
        if data is None and status == 'ok':
            data = fixture_bytes[item['path']]
        return dict(path=item['path'], status=status,
                    bytes=len(data) if data is not None else 0,
                    sha256=hashlib.sha256(data).hexdigest() if data is not None else '')

    def report(findings):
        return compact(dict(ok=all(f['status'] == 'ok' for f in findings), findings=findings))

    def run(label, args, executable=None):
        command = [str(executable or hirsel), *map(str, args)]
        started = time.monotonic()
        result = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True)
        (output / (label + '.stdout')).write_bytes(result.stdout)
        (output / (label + '.stderr')).write_bytes(result.stderr)
        records.append(dict(label=label, command=command, exit=result.returncode,
                            seconds=time.monotonic() - started,
                            stdout_sha256=hashlib.sha256(result.stdout).hexdigest(),
                            stderr_sha256=hashlib.sha256(result.stderr).hexdigest()))
        (output / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        return result.returncode, result.stdout, result.stderr

    def native(engine):
        return [] if engine == 'h' else ['--native', '--backend', engine]

    def expect(label, actual, expected):
        assert actual == expected, (label, actual, expected)

    def grant(name, denied=False):
        value = json.loads((root / 'grants.json').read_text())
        for mount in value['io']['dirs']:
            mount['host'] = str(output / mount['path'])
            assert mount['access'] == 'read'
        if denied:
            value['io']['dirs'] = [m for m in value['io']['dirs'] if m['path'] != 'assets']
        value['args'] = ['verify', name + '.json']
        path = output / (name + '-grants.json')
        path.write_bytes(compact(value))
        return path

    # Run the checked guide's package commands on each engine with exact counts.
    expect('fmt', run('fmt', ['fmt', '--check', root / 'main.hir']), (0, b'', b''))
    for engine in ENGINES:
        actual = run('package-' + engine, ['test', selection, '--json', *native(engine)])
        assert actual[0] == 0, actual
        result = json.loads(actual[1])
        assert result['summary']['total'] == 4, result
        assert result['summary']['failed'] == 0, result

    binaries = {}
    for engine in ENGINES[1:]:
        binary = output / ('assetcheck-' + engine)
        expect('build-' + engine,
               run('build-' + engine, ['build', selection, '--debug', '--backend', engine, '-o', binary]),
               (0, b'', b''))
        binaries[engine] = binary

    def launch(label, engine, grants=None, tape=None, record=False):
        if engine == 'h':
            args = ['run', selection]
            if grants is not None:
                args += ['--grants', grants]
            if tape is not None:
                args += ['--record' if record else '--replay', tape]
            return run(label, args)
        args = []
        if tape is not None:
            args += ['--hirsel-record' if record else '--hirsel-replay', tape]
        if grants is not None:
            args += ['--hirsel-grants', grants, *json.loads(grants.read_text())['args']]
        return run(label, args, executable=binaries[engine])

    good = [entry(name) for name in fixture_bytes]
    wrong_length = dict(good[1], bytes=2)
    wrong_digest = dict(good[2], sha256='0' * 64)
    both = dict(good[3], bytes=0, sha256='0' * 64)
    missing = entry('missing', b'')
    bad = [wrong_length, wrong_digest, missing, both, good[0]]
    success = (0, report([finding(e) for e in good]), b'')
    failure = (1, report([
        finding(wrong_length, 'length', fixture_bytes['short']),
        finding(wrong_digest, 'digest', fixture_bytes['binary.bin']),
        finding(missing, 'missing'),
        finding(both, 'length_digest', fixture_bytes['boundary.bin']),
        finding(good[0]),
    ]), b'error: Mismatch\n')
    cases = [('success', dict(assets=good), success), ('failure', dict(assets=bad), failure)]
    # Every engine records its own success and meaningful failure, then every
    # engine consumes each tape. Fresh native runs also use original grants.
    for name, manifest, expected in cases:
        (manifests / (name + '.json')).write_bytes(compact(manifest))
        grants = grant(name)
        for recorder in ENGINES:
            tape = output / (name + '-' + recorder + '.tape')
            actual = launch(name + '-record-' + recorder, recorder, grants, tape, record=True)
            expect(name + '-record-' + recorder, actual, expected)
            assert tape.is_file()
            # Deny the recorded host root by moving it away. Keep a different
            # sentinel at the old root: any fresh host read would disagree.
            hidden = output / 'assets-hidden'
            assets.rename(hidden)
            assets.mkdir()
            (assets / 'empty').write_bytes(b'replay sentinel')
            hidden_manifests = output / 'manifests-hidden'
            manifests.rename(hidden_manifests)
            manifests.mkdir()
            (manifests / (name + '.json')).write_bytes(b'manifest replay sentinel')
            try:
                for engine in ENGINES:
                    actual = launch(name + '-replay-' + recorder + '-' + engine, engine, tape=tape)
                    expect(name + '-replay-' + recorder + '-' + engine, actual, expected)
                    assert sorted(p.name for p in assets.iterdir()) == ['empty']
                    assert (assets / 'empty').read_bytes() == b'replay sentinel'
                    assert sentinel.read_bytes() == b'denied sentinel\x00\xff'
                    assert sorted(p.name for p in manifests.iterdir()) == [name + '.json']
                    assert (manifests / (name + '.json')).read_bytes() == b'manifest replay sentinel'
            finally:
                (assets / 'empty').unlink()
                assets.rmdir()
                hidden.rename(assets)
                (manifests / (name + '.json')).unlink()
                manifests.rmdir()
                hidden_manifests.rename(manifests)
        for engine in ENGINES[1:]:
            expect(name + '-fresh-' + engine,
                   launch(name + '-fresh-' + engine, engine, grant(name)), expected)

    # Each invalid input is independently asserted, including full prevalidation
    # of a later entry after an otherwise valid first entry.
    invalid = [
        ('duplicate', dict(assets=[good[0], good[0]]), 'DuplicatePath'),
        ('escape', dict(assets=[good[0], dict(good[1], path='../denied.bin')]), 'InvalidPath'),
        ('absolute', dict(assets=[dict(good[0], path=str(sentinel))]), 'InvalidPath'),
        ('alias', dict(assets=[dict(good[0], path='a/../empty')]), 'InvalidPath'),
        ('nul', dict(assets=[dict(good[0], path='a\0b')]), 'InvalidPath'),
        ('hex', dict(assets=[dict(good[0], sha256='z' * 64)]), 'InvalidHex'),
        ('short-hex', dict(assets=[dict(good[0], sha256='aa')]), 'InvalidHex'),
        ('negative-length', dict(assets=[dict(good[0], bytes=-1)]), 'InvalidJson'),
        ('missing-field', dict(assets=[dict(path='empty', bytes=0)]), 'InvalidJson'),
        ('expected-cap', dict(assets=[dict(good[0], bytes=65537)]), 'ExpectedSizeLimit'),
        ('entries-cap', dict(assets=[dict(good[0], path='x' + str(i)) for i in range(65)]), 'EntryLimit'),
        ('malformed-json', b'{', 'InvalidJson'),
        ('duplicate-key', b'{"assets":[],"assets":[]}', 'InvalidJson'),
        ('manifest-cap', b' ' * 16385, 'ManifestLimit'),
    ]
    for name, value, error in invalid:
        (manifests / (name + '.json')).write_bytes(value if isinstance(value, bytes) else compact(value))
        expect(name, run(name, ['run', selection, '--grants', grant(name)]),
               (1, b'', ('error: ' + error + '\n').encode()))

    # The file-byte cap is enforced while reading, independently of its claimed
    # size. Equality at both caps is allowed and requires a final EOF probe.
    cap = entry('over-cap', b'')
    exact = entry('at-cap', b'x' * 65536)
    upper = dict(good[1], sha256=good[1]['sha256'].upper())
    more = [
        ('actual-cap', [cap], (1, report([finding(cap, 'limit')]), b'error: Mismatch\n')),
        ('at-cap', [exact], (0, report([finding(exact, data=b'x' * 65536)]), b'')),
        ('uppercase', [upper], (0, report([finding(upper)]), b'')),
        ('symlink', [entry('symlink', b'')],
         (1, report([finding(entry('symlink', b''), 'read')]), b'error: Mismatch\n')),
        ('empty-manifest', [], (0, report([]), b'')),
    ]
    for name, items, expected in more:
        (manifests / (name + '.json')).write_bytes(compact(dict(assets=items)))
        for engine in ENGINES:
            expect(name + '-' + engine,
                   launch(name + '-' + engine, engine, grant(name)),
                   expected)
    (manifests / 'exact-manifest-cap.json').write_bytes(compact(dict(assets=[])).rstrip() + b' ' * (16384 - len(compact(dict(assets=[])).rstrip())))
    expect('exact-manifest-cap', run('exact-manifest-cap', ['run', selection, '--grants', grant('exact-manifest-cap')]),
           (0, report([]), b''))
    expect('denied-root', run('denied-root', ['run', selection, '--grants', grant('success', denied=True)]),
           (1, b'', b'error: File(NotGranted)\n'))
    expect('usage', run('usage', ['run', selection, '--grants', grant('success'), '--', '--unknown']),
           (1, b'', b'error: Usage\n'))
    expect('manifest-escape', run('manifest-escape', ['run', selection, '--grants', grant('success'), '--', 'verify', '../denied.bin']),
           (1, b'', b'error: InvalidPath\n'))
    expect('manifest-missing', run('manifest-missing', ['run', selection, '--grants', grant('no-manifest')]),
           (1, b'', b'error: ManifestRead\n'))
    assert {p.name: p.read_bytes() for p in assets.iterdir() if not p.is_symlink()} == original_tree
    assert sentinel.read_bytes() == b'denied sentinel\x00\xff'
    print(f'assetcheck: 4 package tests x 3 engines; {len(records)} exact command observations passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent / 'assetcheck')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix='hirsel-dogfood7-') as scratch:
            verify(args.hirsel.resolve(), args.root.resolve(), Path(scratch) / 'evidence')


if __name__ == '__main__':
    main()
