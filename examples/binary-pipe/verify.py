#!/usr/bin/env python3
"""Independent byte/count oracle; real OS pipes, grants and replay on H/LLVM/CLIF."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=True)
    manifest = root / 'hirsel.json'
    grants = json.loads((root / 'grants.json').read_text())
    records = []

    def run(label, command, data=b'', pipe=True, dev_full=False):
        # communicate's stdin/stdout are real OS pipes; input is deliberately bytes.
        stdout = open('/dev/full', 'wb') if dev_full else subprocess.PIPE
        try:
            result = subprocess.run(list(map(str, command)), input=data if pipe else None,
                                    stdin=None if pipe else subprocess.DEVNULL,
                                    stdout=stdout, stderr=subprocess.PIPE, timeout=180)
        finally:
            if dev_full:
                stdout.close()
        (output / (label + '.stdout')).write_bytes(result.stdout or b'')
        (output / (label + '.stderr')).write_bytes(result.stderr)
        records.append(dict(label=label, command=list(map(str, command)),
                            exit=result.returncode,
                            input_bytes=len(data),
                            stdout_sha256=hashlib.sha256(result.stdout or b'').hexdigest()))
        (output / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        return result

    engines = {'H': [hirsel, 'run', manifest]}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        result = run('build-' + backend,
                     [hirsel, 'build', manifest, '--backend', backend, '-o', exe])
        assert result.returncode == 0, result.stderr
        engines[backend] = [exe]

    # No text decoding: invalid UTF-8, every octet, CRLF, EOF following data,
    # and a span longer than the largest configured buffer without a newline.
    vectors = {
        'empty': b'',
        'binary': b'\x00\x80\xff\xc0\xaf\r\n\n\r\x00tail\xff',
        'all-octets': bytes(range(256)) * 2,
        'long': b'\xff' * 4103,
        'eof-data': b'last\x00\x80\xff',
    }
    host_runs = replays = denied = failures = 0
    for capacity in (1, 2, 7, 4096):
        grants['args'] = [str(capacity)]
        config = output / f'grants-{capacity}.json'
        config.write_text(json.dumps(grants) + '\n')
        for name, data in vectors.items():
            original = None
            tape = output / f'{capacity}-{name}.tape'
            for engine, command in engines.items():
                args = ['--grants', config] if engine == 'H' else ['--hirsel-grants', config, str(capacity)]
                if engine == 'H':
                    args += ['--record', tape]
                label = f'{capacity}-{name}-{engine}'
                result = run(label, [*command, *args], data)
                assert result.returncode == 0, (label, result.stderr)
                assert result.stdout == data, (label, result.stdout[:100])
                count = re.fullmatch(rb'bytes=(\d+) newlines=(\d+) max_buffered=(\d+)\n', result.stderr)
                assert count, (label, result.stderr)
                size, lines, peak = map(int, count.groups())
                assert (size, lines) == (len(data), data.count(b'\n')), label
                assert 0 <= peak <= capacity and (peak > 0) == bool(data), (label, capacity, peak)
                host_runs += 1
                if engine == 'H':
                    original = (result.stdout, result.stderr)
            for engine, command in engines.items():
                flag = '--replay' if engine == 'H' else '--hirsel-replay'
                result = run(f'{capacity}-{name}-replay-{engine}', [*command, flag, tape], pipe=False)
                assert result.returncode == 0, (engine, result.stderr)
                assert (result.stdout, result.stderr) == original, engine
                replays += 1

    # Acquiring each required projection must fail before any stdout bytes.
    for missing in ('stdin', 'stdout', 'stderr'):
        config = output / f'no-{missing}.json'
        denied_grants = json.loads(json.dumps(grants))
        denied_grants['io'].pop(missing)
        config.write_text(json.dumps(denied_grants) + '\n')
        for engine, command in engines.items():
            flag = '--grants' if engine == 'H' else '--hirsel-grants'
            result = run(f'no-{missing}-{engine}', [*command, flag, config], b'\x00\xff\n')
            assert result.returncode != 0 and result.stdout == b'' and result.stderr, (engine, result)
            denied += 1

    # A real host write failure; no successful count line can be reported.
    config = output / 'grants-7.json'
    for engine, command in engines.items():
        flag = '--grants' if engine == 'H' else '--hirsel-grants'
        args = [flag, config] if engine == 'H' else [flag, config, '7']
        result = run(f'output-error-{engine}', [*command, *args], b'\x00\xff\n', dev_full=True)
        assert result.returncode != 0 and result.stderr, (engine, result)
        assert b'max_buffered=' not in result.stderr, result.stderr
        failures += 1
    print(f'binary-pipe: {host_runs} pipe vectors, {replays} grant-free replays, '
          f'{denied} grant denials, {failures} host write errors passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', required=True)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        verify(args.hirsel, args.root.resolve(), args.output.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix='hirsel-binary-pipe-') as scratch:
            verify(args.hirsel, args.root.resolve(), Path(scratch))


if __name__ == '__main__':
    main()
