#!/usr/bin/env python3
"""Bounded metricfold acceptance through the interpreter, LLVM and Cranelift.

Histogram vector: OpenTelemetry Rust SDK histogram.rs:317-342, fb6b5356,
Apache-2.0 (example fixtures/LICENSE). Integer arithmetic and an ordered direct
accumulator replace SDK floating point and concurrency. No expected data comes
from Hirsel. Runtime processes run sequentially under the shipped tools/capped.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

HELP = (b'usage: metricfold FILE\n'
        b'Fold bounded job measurements into counters and duration_ms histograms.\n'
        b'Input: NDJSON instrument, unit, labels (queue/status text), value (integer).\n'
        b'Limits: 16384 input bytes, 1024 bytes/line, 64 records, 2 series/instrument.\n'
        b'Unseen series beyond the limit aggregate into overflow without labels.\n')


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def record(value=1, instrument='jobs', labels=None, unit=None):
    return dict(instrument=instrument, unit=unit or ('job' if instrument == 'jobs' else 'ms'),
                labels=labels if labels is not None else dict(queue='build', status='ok'), value=value)


def oracle(records):
    """Small direct integer accumulator; interval index counts bounds below n."""
    instruments = []
    for name, unit, bounds in [('jobs', 'job', []), ('duration_ms', 'ms', [1, 3, 6])]:
        def zero():
            return dict(count=0, sum=0, buckets=[0] * (len(bounds) + 1) if bounds else [])
        result = dict(name=name, unit=unit, bounds=bounds, series=[], overflow=zero())
        for row in records:
            if row['instrument'] != name:
                continue
            assert row['unit'] == unit and type(row['value']) is int and 0 <= row['value'] < 2**64
            key = dict(sorted(row['labels'].items()))
            series = next((s for s in result['series'] if s['labels'] == key), None)
            if series is None and len(result['series']) < 2:
                series = dict(labels=key, aggregate=zero())
                result['series'].append(series)
            aggregate = result['overflow'] if series is None else series['aggregate']
            aggregate['count'] += 1
            aggregate['sum'] += row['value']
            assert aggregate['sum'] < 2**64
            if bounds:
                aggregate['buckets'][sum(row['value'] > b for b in bounds)] += 1
        instruments.append(result)
    return dict(schema=1, records=len(records),
                overflow_policy='aggregate_unseen_without_labels', instruments=instruments)


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    selection = root / 'hirsel.json'
    commands = []
    runtime_count = 0
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')

    def run(label, argv, runtime=False):
        nonlocal runtime_count
        argv = list(map(str, argv))
        stdout = output / (label + '.stdout')
        stderr = output / (label + '.stderr')
        if runtime:
            # Redirection keeps cap accounting separate from the program streams.
            # Arguments are positional, with no shell interpolation of user text.
            command = [sys.executable, root / 'capped', '--timeout', '60',
                       'sh', '-c', 'exec "$@" >"$METRICFOLD_STDOUT" 2>"$METRICFOLD_STDERR"',
                       'metricfold', *argv]
            process = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True,
                                     timeout=180, env=env | dict(METRICFOLD_STDOUT=str(stdout),
                                                                METRICFOLD_STDERR=str(stderr)))
            (output / (label + '.cap')).write_bytes(process.stdout + process.stderr)
            runtime_count += 1
            result = (process.returncode, stdout.read_bytes(), stderr.read_bytes())
        else:
            process = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                     timeout=900, env=env)
            stdout.write_bytes(process.stdout)
            stderr.write_bytes(process.stderr)
            result = (process.returncode, process.stdout, process.stderr)
        commands.append(dict(label=label, argv=argv, exit=result[0]))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return result

    for command in ('fmt', 'check'):
        args = [hirsel, command, *(['--check'] if command == 'fmt' else ['--json']), selection]
        assert run(command, args) == (0, b'', b''), command
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        actual = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert actual == (0, b'', b''), (backend, actual)
        binaries[backend] = exe

    cases = []

    def good(label, rows, blob=None):
        contents = b''.join(canonical(r) for r in rows) if blob is None else blob
        name = label + '.ndjson'
        (data / name).write_bytes(contents)
        cases.append((label, [name], (0, canonical(oracle(rows)), b''), None))

    def bad(label, blob, line, kind):
        name = label + '.ndjson'
        (data / name).write_bytes(blob)
        cases.append((label, [name], (2, b'', canonical(dict(schema=1, error=dict(line=line, kind=kind)))), None))

    realistic = (root / 'fixtures/jobs.ndjson').read_bytes()
    good('realistic', [json.loads(s) for s in realistic.splitlines()], realistic)
    invalid = (root / 'fixtures/invalid.ndjson').read_bytes()
    bad('realistic-invalid', invalid, 2, 'invalid_value')
    vector = [record(n, 'duration_ms') for n in range(1, 11)]
    assert oracle(vector)['instruments'][1]['series'][0]['aggregate'] == dict(count=10, sum=55, buckets=[1, 2, 3, 4])
    good('upstream-inclusive', vector)
    good('empty', [])
    good('final-no-newline', [record(6, 'duration_ms')], canonical(record(6, 'duration_ms')).rstrip(b'\n'))
    good('crlf', [record()], canonical(record()).replace(b'\n', b'\r\n'))
    series = [record(n, name, dict(status='ok', queue=q)) for name in ('jobs', 'duration_ms')
              for n, q in [(1, 'a'), (2, 'b'), (3, 'c'), (4, 'c'), (5, 'a'), (6, 'd'), (7, 'b')]]
    good('bounded-series', series)
    good('empty-labels-zero', [record(0, 'duration_ms', {}), record(0, labels={})])
    good('maximum-counter', [record(2**64 - 1)])
    good('labels-exact', [record(labels=dict(queue='q' * 64, status='β\n'))])
    good('records-exact', [record()] * 64)
    line = canonical(record()).rstrip(b'\n')
    good('line-exact', [record()], line + b' ' * (1024 - len(line)))
    padded = line + b' ' * (1023 - len(line)) + b'\n'
    good('input-exact', [record()] * 16, padded * 16)
    bad('input-over', padded * 16 + b'\n', 17, 'input_bytes_limit')
    bad('line-over', line + b' ' * (1025 - len(line)), 1, 'line_bytes_limit')
    bad('records-over', canonical(record()) * 65, 65, 'record_count_limit')
    bad('descriptor-conflict', canonical(record()) + canonical(record(unit='count')), 2, 'descriptor_conflict')
    bad('negative-counter', canonical(record(-1)), 1, 'invalid_value')
    bad('negative-duration', canonical(record(-1, 'duration_ms')), 1, 'invalid_value')
    bad('integer-overflow', canonical(record(2**64 - 1)) + canonical(record()), 2, 'integer_overflow')
    bad('histogram-overflow', canonical(record(2**64 - 1, 'duration_ms')) + canonical(record(1, 'duration_ms')), 2, 'integer_overflow')
    bad('overflow-series-arithmetic', b''.join(canonical(record(n, labels=dict(queue=q)))
        for n, q in [(1, 'a'), (1, 'b'), (2**64 - 1, 'c'), (1, 'd')]), 4, 'integer_overflow')
    bad('malformed-json', canonical(record()) + b'{\n', 2, 'invalid_json')
    bad('first-invalid-before-long-line', b'{\n' + b'x' * 1025, 1, 'invalid_json')
    bad('utf8', canonical(record()) + b'\xff\n', 2, 'invalid_utf8')
    bad('duplicate-key', b'{"instrument":"jobs","instrument":"jobs"}\n', 1, 'invalid_json')
    bad('blank-line', b'\n', 1, 'invalid_json')
    bad('wrong-shape', canonical(dict(instrument='jobs')), 1, 'record_shape')
    bad('unknown-instrument', canonical(record(instrument='other')), 1, 'descriptor_conflict')
    bad('labels-over', canonical(record(labels=dict(queue='q' * 65))), 1, 'invalid_labels')
    bad('label-count-over', canonical(record(labels=dict(queue='a', status='ok', host='h'))), 1, 'invalid_labels')
    bad('unknown-label', canonical(record(labels=dict(request_id='secret'))), 1, 'invalid_labels')
    bad('nontext-label', canonical(record(labels=dict(queue=7))), 1, 'invalid_labels')
    for label, value in [('float', 1.5), ('too-large', 2**64), ('text-value', '1')]:
        bad(label, canonical(record(value)), 1, 'invalid_value')
    cases.extend([
        ('help', ['--help'], (0, HELP, b''), 'dir'),
        ('missing-grant', ['realistic.ndjson'], (2, b'', canonical(dict(schema=1, error=dict(line=0, kind='directory_grant')))), 'dir'),
        ('missing-file', ['absent.ndjson'], (2, b'', canonical(dict(schema=1, error=dict(line=0, kind='file_open')))), None),
        ('outside-grant', ['../outside.ndjson'], (2, b'', canonical(dict(schema=1, error=dict(line=0, kind='file_open')))), None),
        ('usage', [], (2, b'', canonical(dict(schema=1, error=dict(line=0, kind='usage')))), None),
        ('missing-stdout', ['realistic.ndjson'], (3, b'', b''), 'stdout'),
        ('missing-stderr', ['malformed-json.ndjson'], (3, b'', b''), 'stderr'),
        ('deterministic-repeat', ['bounded-series.ndjson'], (0, canonical(oracle(series)), b''), None),
    ])
    outside = output / 'outside.ndjson'
    outside.write_bytes(canonical(record()))
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    outside_hash = hashlib.sha256(outside.read_bytes()).hexdigest()
    grants = output / 'grants.json'
    for label, args, expected, missing in cases:
        io = dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
                  dirs=[dict(path='input', host=str(data), access='read')])
        if missing == 'dir':
            io['dirs'] = []
        elif missing:
            del io[missing]
        grants.write_text(json.dumps(dict(io=io, args=args)))
        selected_grants = root / 'grants.json' if label == 'realistic' else grants
        launch_args = ['jobs.ndjson'] if label == 'realistic' else args
        for engine in ('h', 'llvm', 'cranelift'):
            argv = ([hirsel, 'run', selection, '--grants', selected_grants] if engine == 'h' else
                    [binaries[engine], '--hirsel-grants', selected_grants, *launch_args])
            actual = run(label + '-' + engine, argv, runtime=True)
            assert actual == expected, (label, engine, actual, expected, str(output))
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}, 'fixture changed'
    assert outside_hash == hashlib.sha256(outside.read_bytes()).hexdigest(), 'outside grant changed'
    for p in data.iterdir():
        p.unlink()
    data.rmdir()
    grants.unlink()
    outside.unlink()
    assert not data.exists() and not grants.exists() and not outside.exists(), 'cleanup'
    assert len(cases) == 44 and runtime_count == 132
    result = dict(cases=len(cases), executions=runtime_count, engines=['h', 'llvm', 'cranelift'], cleanup=True)
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    opts = parser.parse_args()
    verify(opts.hirsel.resolve(), opts.root.resolve(), opts.output.resolve())
