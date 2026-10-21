#!/usr/bin/env python3
"""Verify one dogfood-6 example against host inputs and grant-free replay.

Uses only Python's standard library as an independent ZIP/JSON fixture oracle.
Run one tool per invocation; --output retains exact commands, bytes and statuses.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import time
import zipfile


def verify(hirsel, tool, root, output):
    output.mkdir(parents=True, exist_ok=True)
    data = output / 'data'
    data.mkdir()
    # Tapes are siblings of data, outside the directory authority.
    grants = json.loads((root / 'grants.json').read_text())
    for mount in grants['io']['dirs']:
        mount['host'] = str(data)
    selection = root / ('run.json' if tool == 'snapshift' else 'hirsel.json')
    if tool == 'zipinspect':
        selection = root / 'main.hir'  # deliberately has no sidecar
    records = []

    def run(label, args, stdin=b''):
        command = [hirsel, *map(str, args)]
        started = time.monotonic()
        result = subprocess.run(command, input=stdin, capture_output=True)
        (output / (label + '.stdout')).write_bytes(result.stdout)
        (output / (label + '.stderr')).write_bytes(result.stderr)
        records.append(dict(label=label, command=command, exit=result.returncode,
                            seconds=time.monotonic() - started,
                            stdout_sha256=hashlib.sha256(result.stdout).hexdigest(),
                            stderr_sha256=hashlib.sha256(result.stderr).hexdigest()))
        (output / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        return result.returncode, result.stdout, result.stderr

    # Verify formatting and checking of the actual runnable selection.
    for source in sorted(root.glob('*.hir')):
        result = run('fmt-' + source.stem, ['fmt', '--check', source])
        assert result[0] == 0, result
    result = run('check', ['check', selection])
    assert result[0] == 0, result

    binary = b'\0\xff\r\n'
    if tool == 'zipinspect':
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w') as writer:
            member = zipfile.ZipInfo('raw.bin', (2020, 1, 1, 0, 0, 0))
            writer.writestr(member, binary)
        stdin = archive.getvalue()
        # Independently read the generated member before handing it to Hirsel.
        with zipfile.ZipFile(io.BytesIO(stdin)) as reader:
            assert reader.read('raw.bin') == binary
        grants['args'] = ['raw.bin']
        success = binary
        bad_stdin = b'not a zip\0\xff'
    elif tool == 'snapshift':
        stdin = b'{"captured":"2024-02-29","items":[{"name":"wool","active":true},{"name":"old","active":false}]}'
        expected = json.loads(stdin)
        expected['items'] = [item for item in expected['items'] if item['active']]
        success = json.dumps(expected, separators=(',', ':')).encode() + b'\n'
        bad_stdin = b'{"captured":"2023-02-29","items":[]}'
    elif tool == 'httpmirror':
        stdin = bad_stdin = b''
        success = b'mirrored 4 bytes\n'
    else:
        stdin = bad_stdin = b''
        (data / 'app.log').write_bytes(b'ERROR broken\nWARN slow\nINFO ok')
        success = b'lines=3 errors=1 warnings=1\nstopped\n'

    for case in ('success', 'failure'):
        config = json.loads(json.dumps(grants))
        input_bytes = stdin if case == 'success' else bad_stdin
        if case == 'failure' and tool == 'httpmirror':
            config['args'][1] = '../outside.bin'
        if case == 'failure' and tool == 'logdigest':
            config['args'][0] = 'missing.log'
        config_path = output / (case + '-grants.json')
        config_path.write_text(json.dumps(config, indent=2) + '\n')
        (output / (case + '.input')).write_bytes(input_bytes)
        tape = output / (case + '.tape')
        original = run(case + '-record', ['run', selection, '--grants', config_path,
                                        '--record', tape], input_bytes)
        if case == 'success':
            assert original == (0, success, b''), original
            if tool == 'httpmirror':
                assert (data / 'raw.bin').read_bytes() == binary
            if tool == 'snapshift':
                assert json.loads(original[1]) == expected
        else:
            assert original[0] == 1, original
            assert original[1] == b'' and original[2], original
        assert tape.is_file(), 'the terminal outcome must be recorded'
        if tool == 'httpmirror':
            # A replay must confirm the write without applying it to the host.
            (data / 'raw.bin').write_bytes(b'replay sentinel')
        for engine in ('interpreted', 'llvm', 'cranelift'):
            native = [] if engine == 'interpreted' else ['--native', '--backend', engine]
            replayed = run(case + '-replay-' + engine,
                           ['run', selection, '--replay', tape, *native])
            assert replayed == original, (case, engine, original, replayed)
            if tool == 'httpmirror':
                assert (data / 'raw.bin').read_bytes() == b'replay sentinel'
        # Fresh native host runs exercise the launch grants as well as the tape path.
        for backend in ('llvm', 'cranelift'):
            actual = run(case + '-host-' + backend,
                         ['run', selection, '--grants', config_path,
                          '--native', '--backend', backend], input_bytes)
            assert actual == original, (case, backend, original, actual)
        assert not (output / 'outside.bin').exists(), 'directory confinement escaped'
    print(f'{tool}: fmt/check; 2 records, 6 replays, 4 fresh native runs passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True)
    parser.add_argument('--tool', required=True,
                        choices=['zipinspect', 'snapshift', 'httpmirror', 'logdigest'])
    parser.add_argument('--root', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    root = (args.root or Path(__file__).resolve().parent / args.tool).resolve()
    if args.output:
        verify(args.hirsel, args.tool, root, args.output.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix='hirsel-dogfood6-') as scratch:
            verify(args.hirsel, args.tool, root, Path(scratch))


if __name__ == '__main__':
    main()
