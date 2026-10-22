#!/usr/bin/env python3
"""Independent Python tarfile/zipfile/hashlib oracle and offline replay checks.

Walk/truncation controls adapt Go archive/tar/reader{,_test}.go:44-130,793-855;
ZIP undersize control: archive/zip/reader_test.go:1656-1681 (ebe2feef, BSD-3-Clause,
Copyright The Go Authors; third-party/NOTICES.md). Harness structure adapts
examples/verify-dogfood6.py:18-90 at 7d84d688. No codec bytes come from Hirsel.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import warnings
import zipfile


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def entry(name, data=b'', kind='file'):
    return dict(path=name, type=kind, size=len(data), sha256=hashlib.sha256(data).hexdigest())


def report(entries, missing=(), extra=(), changed=()):
    entries = sorted(entries, key=lambda e: e['path'].encode())
    return dict(schema=1, entries=entries,
                summary=dict(**{'class': 'mismatch' if missing or extra or changed else 'ok'},
                             members=len(entries), bytes=sum(e['size'] for e in entries),
                             missing=list(missing), extra=list(extra), changed=list(changed)))


def archive(kind, items):
    out = io.BytesIO()
    if kind == 'tar':
        with tarfile.open(fileobj=out, mode='w', format=tarfile.PAX_FORMAT) as writer:
            for name, data, member_kind in items:
                member = tarfile.TarInfo(name)
                member.size = len(data)
                if member_kind == 'directory':
                    member.type = tarfile.DIRTYPE
                elif member_kind == 'symlink':
                    member.type, member.linkname = tarfile.SYMTYPE, 'target'
                elif member_kind == 'hardlink':
                    member.type, member.linkname = tarfile.LNKTYPE, 'target'
                elif member_kind == 'fifo':
                    member.type = tarfile.FIFOTYPE
                writer.addfile(member, io.BytesIO(data))
    else:
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', UserWarning)
            with zipfile.ZipFile(out, 'w', compression=zipfile.ZIP_DEFLATED) as writer:
                for name, data, member_kind in items:
                    member = zipfile.ZipInfo(name, (2020, 1, 1, 0, 0, 0))
                    member.create_system = 3
                    mode = {'file': 0o100644, 'directory': 0o40755,
                            'symlink': 0o120777, 'fifo': 0o10644}[member_kind]
                    member.external_attr = mode << 16
                    if member_kind == 'directory':
                        member.external_attr |= 0x10
                    member.compress_type = zipfile.ZIP_DEFLATED
                    writer.writestr(member, data)
    return out.getvalue()


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    selection = root / 'hirsel.json'
    commands = []

    def run(label, argv):
        argv = list(map(str, argv))
        done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=240,
                              env=os.environ | {'HIRSEL_DAEMON': 'off'})
        (output / (label + '.stdout')).write_bytes(done.stdout)
        (output / (label + '.stderr')).write_bytes(done.stderr)
        commands.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return done.returncode, done.stdout, done.stderr

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    assert run('check', [hirsel, 'check', selection]) == (0, b'', b'')
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        result = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert result == (0, b'', b''), result
        binaries[backend] = exe

    policy = dict(io=dict(stdout='stdout.bytes@1', dirs=[
        dict(path='input', host=str(data), access='read')]), args=[])
    grants = output / 'grants.json'
    cases = []

    def case(label, args, expected=None, error=None, record=False, grant=None):
        if error:
            expected = dict(schema=1, summary={'class': 'input_error', 'error': error})
        code = int(error is not None or expected['summary']['class'] == 'mismatch')
        stderr = (('error: InvalidInput(' + json.dumps(error) + ')\n').encode() if error else
                  b'error: Mismatch\n' if code else b'')
        cases.append((label, args, (code, canonical(expected), stderr), record, grant))

    mixed = [('z.bin', b'\0\xff\r\n', 'file'), ('docs/', b'', 'directory'),
             ('docs/日本.txt', '羊\n'.encode(), 'file'), ('a "quote".txt', b'abc', 'file')]
    expected = report([entry(n, b, k) for n, b, k in mixed])
    manifest = canonical(expected)
    (data / 'expected.json').write_bytes(manifest)
    for kind in ('tar', 'zip'):
        good = archive(kind, mixed)
        (data / ('good.' + kind)).write_bytes(good)
        # Independent decode confirms fixture contents, including binary and UTF-8.
        if kind == 'tar':
            with tarfile.open(fileobj=io.BytesIO(good)) as reader:
                assert [m.name for m in reader] == [n.rstrip('/') if k == 'directory' else n for n, _, k in mixed]
                assert reader.extractfile('z.bin').read() == mixed[0][1]
        else:
            with zipfile.ZipFile(io.BytesIO(good)) as reader:
                assert reader.read('z.bin') == mixed[0][1]
        case(kind + '-manifest', ['manifest', 'good.' + kind], expected, record=True)
        case(kind + '-verify', ['verify', 'good.' + kind, 'expected.json'], expected, record=True)
        (data / ('empty.' + kind)).write_bytes(archive(kind, []))
        case(kind + '-empty', ['manifest', 'empty.' + kind], report([]))
        for label, items, error in [
            ('duplicate', [('a', b'', 'file')] * 2, 'duplicate_path'),
            ('alias', [('a', b'', 'file'), ('a/', b'', 'directory')], 'duplicate_path'),
            ('link', [('a', b'', 'symlink')], 'unsupported_member_kind'),
            ('fifo', [('a', b'', 'fifo')], 'unsupported_member_kind'),
            ('member-cap', [(str(n), b'', 'file') for n in range(129)], 'member_count_limit'),
            ('entry-cap', [('a', b'x' * 65537, 'file')], 'entry_bytes_limit'),
            ('total-cap', [(str(n), b'x' * 65536, 'file') for n in range(5)], 'expanded_bytes_limit'),
        ]:
            name = kind + '-' + label + '.' + kind
            (data / name).write_bytes(archive(kind, items))
            case(kind + '-' + label, ['manifest', name], error=error,
                 record=label in ('entry-cap', 'total-cap'))
        for i, name in enumerate(('/abs', '../escape', 'a/../b', './a', 'a//b', 'a\\b', 'C:drive')):
            file = f'{kind}-path-{i}.{kind}'
            (data / file).write_bytes(archive(kind, [(name, b'', 'file')]))
            case(f'{kind}-path-{i}', ['manifest', file], error='invalid_path')
        # Simultaneously missing, extra and changed, with sorted report arrays.
        altered = archive(kind, [('z.bin', b'changed', 'file'), ('new', b'', 'file')])
        file = 'changed.' + kind
        (data / file).write_bytes(altered)
        actual = report([entry('z.bin', b'changed'), entry('new')],
                        missing=['a "quote".txt', 'docs/', 'docs/日本.txt'], extra=['new'], changed=['z.bin'])
        case(kind + '-mismatch', ['verify', file, 'expected.json'], actual, record=True)

    # Both sides of each numeric boundary, and both ZIP methods.
    for kind in ('tar', 'zip'):
        for label, items in [
                ('members-exact', [(str(n), b'', 'file') for n in range(128)]),
                ('entry-exact', [('a', b'x' * 65536, 'file')]),
                ('total-exact', [(str(n), b'x' * 65536, 'file') for n in range(4)]),
                ('path-exact', [('a' * 512, b'', 'file')])]:
            file = kind + '-' + label + '.' + kind
            (data / file).write_bytes(archive(kind, items))
            case(kind + '-' + label, ['manifest', file], report([entry(n, b, k) for n, b, k in items]))
        file = 'path-over.' + kind
        (data / file).write_bytes(archive(kind, [('a' * 513, b'', 'file')]))
        case(kind + '-path-over', ['manifest', file], error='path_bytes_limit')
    (data / 'archive-exact.tar').write_bytes(archive('tar', []) + b'\0' * (1048576 - 10240))
    case('archive-exact', ['manifest', 'archive-exact.tar'], report([]))
    (data / 'manifest-exact.json').write_bytes(manifest + b' ' * (131072 - len(manifest)))
    case('manifest-exact', ['verify', 'good.zip', 'manifest-exact.json'], expected)
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, 'w', compression=zipfile.ZIP_STORED) as writer:
        writer.writestr('a', b'abc')
    (data / 'stored.zip').write_bytes(buf.getvalue())
    case('zip-stored', ['manifest', 'stored.zip'], report([entry('a', b'abc')]))
    (data / 'type.zip').write_bytes(archive('zip', [('a', b'', 'file')]))
    (data / 'type.json').write_bytes(canonical(report([entry('a', kind='directory')])))
    case('type-only', ['verify', 'type.zip', 'type.json'], report([entry('a')], changed=['a']))

    # Go's exact-end-data / incomplete-padding behavior is intentionally retained.
    short_tar = archive('tar', [('a', b'abc', 'file')])
    for cut, error in [(0, None), (511, 'tar_malformed'), (512, 'tar_malformed'),
                       (514, 'tar_malformed'), (515, None), (600, None), (1025, 'tar_malformed')]:
        file = f'cut-{cut}.tar'
        blob = short_tar[:cut] if cut != 1025 else short_tar[:1024] + b'x'
        (data / file).write_bytes(blob)
        case(f'tar-cut-{cut}', ['manifest', file], report([] if cut == 0 else [entry('a', b'abc')]), error)
    (data / 'hardlink.tar').write_bytes(archive('tar', [('a', b'', 'hardlink')]))
    case('hardlink', ['manifest', 'hardlink.tar'], error='unsupported_member_kind')
    (data / 'nul.tar').write_bytes(archive('tar', [('a\0b', b'', 'file')]))
    # Force PAX rather than a conventional NUL-terminated header name.
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode='w', format=tarfile.PAX_FORMAT) as writer:
        member = tarfile.TarInfo('a'); member.pax_headers = {'path': 'a\0b'}
        writer.addfile(member)
    (data / 'nul.tar').write_bytes(buf.getvalue())
    case('tar-nul', ['manifest', 'nul.tar'], error='tar_malformed')
    # ZIP names are length-delimited; replace equal-length bytes in both headers.
    blob = archive('zip', [('axb', b'', 'file')]).replace(b'axb', b'a\0b')
    (data / 'nul.zip').write_bytes(blob)
    case('zip-nul', ['manifest', 'nul.zip'], error='invalid_path')
    for file, blob, error in [('bad.zip', b'PK', 'zip_malformed'), ('bad.tar', b'x' * 512, 'tar_malformed'),
                              ('truncated.zip', archive('zip', mixed)[:-1], 'zip_malformed'),
                              ('archive-cap.tar', b'\0' * 1048577, 'input_bytes_limit')]:
        (data / file).write_bytes(blob)
        case(file, ['manifest', file], error=error, record=file == 'bad.zip')
    blob = bytearray(archive('zip', [('a', b'abc', 'file')]))
    central = blob.index(b'PK\x01\x02')
    struct.pack_into('<I', blob, central + 24, 1)  # Go TestUnderSize
    (data / 'undersize.zip').write_bytes(blob)
    case('zip-undersize', ['manifest', 'undersize.zip'], error='zip_member_malformed')
    blob = bytearray(archive('zip', [('a', b'abc', 'file')]))
    central = blob.index(b'PK\x01\x02'); blob[central + 16] ^= 1
    (data / 'crc.zip').write_bytes(blob)
    case('zip-crc', ['manifest', 'crc.zip'], error='zip_member_malformed', record=True)

    blob = bytearray(archive('zip', [('a', b'abc', 'file')]))
    central = blob.index(b'PK\x01\x02')
    struct.pack_into('<I', blob, central + 16, 0)
    (data / 'zero-crc.zip').write_bytes(blob)
    case('zip-zero-crc', ['manifest', 'zero-crc.zip'], error='zip_member_checksum')
    blob = bytearray(archive('zip', [('a', b'', 'symlink')]))
    central = blob.index(b'PK\x01\x02'); blob[central + 5] = 19
    (data / 'darwin-link.zip').write_bytes(blob)
    case('darwin-link', ['manifest', 'darwin-link.zip'], error='unsupported_member_kind')
    blob = bytearray(archive('zip', [('a', b'', 'file')]))
    central = blob.index(b'PK\x01\x02'); blob[central + 8] |= 1
    (data / 'encrypted.zip').write_bytes(blob)
    case('encrypted', ['manifest', 'encrypted.zip'], error='unsupported_encryption')
    for label, changes in [('size-only', {'size': 5}),
                            ('digest-only', {'sha256': hashlib.sha256(b'xxxx').hexdigest()})]:
        value = json.loads(manifest)
        next(e for e in value['entries'] if e['path'] == 'z.bin').update(changes)
        file = label + '.json'; (data / file).write_bytes(canonical(value))
        case(label, ['verify', 'good.zip', file], report(expected['entries'], changed=['z.bin']))
    value = json.loads(manifest); value['entries'].reverse()
    (data / 'unsorted.json').write_bytes(canonical(value))
    case('unsorted-manifest', ['verify', 'good.tar', 'unsorted.json'], expected)

    blob = bytearray(archive('zip', [('d/', b'', 'directory')]))
    blob[0] = 0
    (data / 'directory-local.zip').write_bytes(blob)
    case('directory-local', ['manifest', 'directory-local.zip'], error='zip_member_malformed')

    invalid = [('version', canonical(dict(expected, schema=2)), 'manifest_schema'),
               ('duplicate-entry', canonical(report([entry('a'), entry('a')])), 'duplicate_path'),
               ('duplicate-key', b'{"schema":1,"schema":1,"entries":[],"summary":{}}', 'manifest_json'),
               ('json', b'{', 'manifest_json'), ('utf8', b'\xff', 'manifest_utf8'),
               ('manifest-cap', b' ' * 131073, 'input_bytes_limit')]
    for label, blob, error in invalid:
        file = label + '.json'; (data / file).write_bytes(blob)
        case(label, ['verify', 'good.tar', file], error=error, record=label == 'version')
    for label, change, error in [('digest', {'sha256': 'Z' * 64}, 'manifest_digest'),
                                  ('path', {'path': '../bad'}, 'invalid_path'),
                                  ('size', {'size': -1}, 'manifest_size'),
                                  ('kind', {'type': 'symlink'}, 'unsupported_member_kind')]:
        value = report([entry('a')]); value['entries'][0].update(change)
        file = label + '.json'; (data / file).write_bytes(canonical(value))
        case('manifest-' + label, ['verify', 'good.tar', file], error=error)
    case('usage', [], error='usage')
    case('missing-file', ['manifest', 'missing.tar'], error='file_open', record=True)
    case('directory-grant', ['manifest', 'good.tar'], error='directory_grant', grant='dir')
    case('extension', ['manifest', 'expected.json'], error='archive_extension')

    tapes = []
    observations = 0
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    for label, args, expected, record, grant in cases:
        current = json.loads(json.dumps(policy))
        if grant == 'dir': current['io']['dirs'] = []
        current['args'] = args
        grants.write_text(json.dumps(current))
        for engine in ('h', 'llvm', 'cranelift'):
            tape = output / f'{label}-{engine}.tape' if record else None
            command = ([hirsel, 'run', selection, '--max-steps', 'unbounded', '--grants', grants,
                        *(['--record', tape] if tape else [])] if engine == 'h' else
                       [binaries[engine], *(['--hirsel-record', tape] if tape else []),
                        '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, command)
            assert actual == expected, (label, engine, actual, expected, str(output))
            observations += 1
            if tape:
                assert tape.is_file()
                tapes.append((label, engine, tape, expected))
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}, 'host writes'
    # Remove every source and launch template. Replays have no grants and no host
    # directory to read; native tapes are also replayed by the other native engine.
    for p in data.iterdir(): p.unlink()
    data.rmdir(); grants.unlink()
    for label, recorded, tape, expected in tapes:
        for engine in ('h', 'llvm', 'cranelift'):
            argv = ([hirsel, 'run', selection, '--max-steps', 'unbounded', '--replay', tape] if engine == 'h'
                    else [binaries[engine], '--hirsel-replay', tape])
            actual = run(f'{label}-{recorded}-replay-{engine}', argv)
            assert actual == expected, (label, recorded, engine, actual, expected, str(output))
            observations += 1
    result = dict(cases=len(cases), observations=observations, recordings=len(tapes),
                  replays=len(tapes) * 3, engines=['h', 'llvm', 'cranelift'])
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve())
