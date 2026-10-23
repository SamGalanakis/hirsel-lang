#!/usr/bin/env python3
"""Python Decimal oracle; structure follows archiveaudit_15/verify.py.

HalfEven goldens: rust_decimal tests/decimal_tests/rounding.rs:73-97 at
b23fb077 (MIT, Copyright (c) 2016 Paul Mason; third-party/NOTICES.md).
Shipped std implements arithmetic; this harness only quantizes independent data.
"""
import argparse
import csv
from decimal import Decimal, ROUND_HALF_EVEN, localcontext
import hashlib
import io
import json
import os
from pathlib import Path
import resource
import subprocess
import sys

HELP = (b'Usage: invoicecheck FILE.csv\nRead FILE.csv from the fixtures directory grant. '
        b'HalfEven to 2 places per line.\nExit: 0 reconciled/help; 1 mismatches; '
        b'2 invalid input; 1 launcher/output error.\n')
HEADER = 'invoice_id,quantity,unit_price,expected_total\n'


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def oracle(text):
    groups, mismatches = {}, []
    with localcontext() as ctx:
        ctx.prec = 160
        rows = list(csv.DictReader(io.StringIO(text)))
        for record, row in enumerate(rows, 1):
            total = (Decimal(row['unit_price']) * int(row['quantity'])).quantize(
                Decimal('.01'), rounding=ROUND_HALF_EVEN)
            expected = Decimal(row['expected_total'])
            if total != expected:
                mismatches.append(record)
            group = groups.setdefault(row['invoice_id'], [0, Decimal('0.00'), Decimal(0)])
            group[0] += 1
            group[1] += total
            group[2] += expected
        invoices = [dict(invoice_id=id, lines=n, total=format(total, '.2f'),
                         expected=format(expected.normalize(), 'f'), matches=total == expected)
                    for id, (n, total, expected) in groups.items()]
    return (int(bool(mismatches)), canonical(dict(records=len(rows), invoices=invoices,
                                                 mismatches=mismatches)), b'')


def verify(hirsel, root, output, selected=None):
    output.mkdir(parents=True, exist_ok=False)
    data = root / 'fixtures'
    selection = root / 'hirsel.json'
    commands = []
    environment = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                                    HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        prefix = output / label
        if runtime:
            # Keep capped's resource report separate from the application's streams.
            command = [sys.executable, str(root / 'capped'), '--timeout', '60',
                       sys.executable, __file__, '--one', str(prefix), *argv]
        else:
            command = argv
        done = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=600, env=environment)
        if runtime:
            prefix.with_suffix('.cap').write_bytes(done.stderr)
            assert done.returncode == 0, (label, done, str(output))
            result = (int(prefix.with_suffix('.status').read_text()),
                      prefix.with_suffix('.stdout').read_bytes(),
                      prefix.with_suffix('.stderr').read_bytes())
        else:
            result = done.returncode, done.stdout, done.stderr
            prefix.with_suffix('.stdout').write_bytes(done.stdout)
            prefix.with_suffix('.stderr').write_bytes(done.stderr)
        commands.append(dict(label=label, argv=argv, exit=result[0]))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return result

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    checked = run('check', [hirsel, 'check', '--json', selection])
    assert checked[0] == 0 and checked[2] == b'', checked
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output / 'app.tir']) == (0, b'', b'')
    assert (output / 'app.tir').stat().st_size > 0
    # Native test commands include compilation as setup under ordinary build limits.
    for engine in (() if selected else ('h', 'llvm', 'cranelift')):
        command = [hirsel, 'test', '--json', selection]
        if engine != 'h':
            command += ['--native', '--backend', engine]
        tested = run('model-tests-' + engine, command)
        assert tested[0] == 0 and tested[2] == b'', tested
        summary = json.loads(tested[1])['summary']
        assert summary['total'] == 2 and summary['failed'] == 0, summary
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        binary = output / backend
        result = run('build-' + backend, [hirsel, 'build', selection, '--backend', backend, '-o', binary])
        assert result == (0, b'', b''), result
        binaries[backend] = binary

    cases = []

    def valid(name, text):
        (data / (name + '.csv')).write_text(text)
        cases.append((name, [name + '.csv'], oracle(text), 'normal'))

    def invalid(name, text, record, reason):
        (data / (name + '.csv')).write_bytes(text.encode() if isinstance(text, str) else text)
        cases.append((name, [name + '.csv'],
                      (2, canonical(dict(error=dict(record=record, reason=reason))), b''), 'normal'))

    for name in ('success',):
        cases.append((name, [name + '.csv'], oracle((data / (name + '.csv')).read_text()), 'normal'))
    cases.append(('invalid', ['invalid.csv'],
                  (2, canonical(dict(error=dict(record=2, reason='decimal_syntax'))), b''), 'normal'))
    valid('exact', HEADER + 'sum,1,0.1,0.10\nsum,1,0.2,0.20\nscale,1,1.0,1.00\n')
    valid('rounding', HEADER + 'ties,1,1.2250,1.22\nties,1,1.2350,1.24\ncredit,3,-1.225,-3.68\nzero,0,-123.456,0\n')
    valid('mismatch', HEADER + 'expense,3,19.995,59.99\nexpense,1,-5,-5\n')
    # Opposing line mismatches must remain visible even when invoice totals agree.
    valid('cancel', HEADER + 'invoice,1,1,2\ninvoice,1,2,1\n')
    valid('duplicate', HEADER + 'same,2,12.50,25.00\n' * 2)
    valid('quoted', HEADER + '"INV,\"\"A\"\"",1,1.000000,1\n')
    valid('empty', HEADER)
    valid('record-exact', HEADER + ''.join(f'i{n},1,1,1\n' for n in range(64)))
    valid('quantity-exact', HEADER + 'q,1000000,0.000001,1.00\n')
    valid('amount-exact', HEADER + 'wide,1,' + '1' * 57 + '.123456,' + '1' * 57 + '.12\n')
    valid('id-exact', HEADER + 'i' * 64 + ',1,1,1\n')
    valid('bytes-exact', HEADER + 'a,1,1,1\n' + '\n' * (65536 - len(HEADER) - 8))
    invalid('scale', HEADER + 'a,1,1.0000000,1\n', 1, 'decimal_scale')
    invalid('decimal', HEADER + 'a,1,1e2,100\n', 1, 'decimal_syntax')
    invalid('expected-scale', HEADER + 'a,1,1,1.0000000\n', 1, 'decimal_scale')
    invalid('expected-decimal', HEADER + 'a,1,1,abc\n', 1, 'decimal_syntax')
    invalid('length', HEADER + 'a,1,' + '1' * 65 + ',1\n', 1, 'decimal_length')
    for name, q in [('negative-quantity', '-1'), ('quantity-over', '1000001'),
                    ('quantity-syntax', '1.5'), ('quantity-empty', ''), ('quantity-long', '00000001')]:
        invalid(name, HEADER + f'a,{q},1,1\n', 1, 'quantity')
    invalid('id-empty', HEADER + ',1,1,1\n', 1, 'invoice_id')
    invalid('id-long', HEADER + 'a' * 65 + ',1,1,1\n', 1, 'invoice_id')
    invalid('id-control', HEADER + 'a\t,1,1,1\n', 1, 'invoice_id')
    invalid('header', 'id,quantity,unit_price,expected_total\n', 0, 'header')
    invalid('no-header', '', 0, 'header')
    invalid('width', HEADER + 'a,1,1\n', 1, 'csv')
    if selected:
        invalid('width-row2', HEADER + 'a,1,1,1\n' + 'b,1,1\n', 2, 'csv')
    invalid('malformed', HEADER + 'a,1,"1,1\n', 1, 'csv')
    invalid('record-bytes', HEADER + 'a' * 513 + ',1,1,1\n', 1, 'record_bytes_limit')
    invalid('record-over', HEADER + 'a,1,1,1\n' * 65, 65, 'record_limit')
    invalid('bytes-over', b'\n' * 65537, 0, 'input_bytes_limit')
    invalid('utf8', b'\xff', 0, 'utf8')
    cases.extend([
        ('help', ['--help'], (0, HELP, b''), 'no-dir'),
        ('usage', [], (2, canonical(dict(error=dict(record=0, reason='usage'))), b''), 'normal'),
        ('missing-grant', ['success.csv'], (2, canonical(dict(error=dict(record=0, reason='directory_grant'))), b''), 'no-dir'),
        ('missing-file', ['absent.csv'], (2, canonical(dict(error=dict(record=0, reason='file_open'))), b''), 'normal'),
        ('path', ['../success.csv'], (2, canonical(dict(error=dict(record=0, reason='filename'))), b''), 'normal'),
    ])
    if selected:
        cases = [case for case in cases if case[0] in selected]
        assert {case[0] for case in cases} == selected, selected
    grants = output / 'grants.json'
    frozen = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}
    observations = 0
    for label, args, expected, grant in cases:
        policy = dict(io=dict(stdout='stdout.bytes@1', dirs=[] if grant == 'no-dir' else [
            dict(path='fixtures', host=str(data), access='read')]), args=args)
        grants.write_text(json.dumps(policy))
        for engine in ('h', 'llvm', 'cranelift'):
            command = ([hirsel, 'run', selection, '--max-steps', 'unbounded', '--grants', grants]
                       if engine == 'h' else [binaries[engine], '--hirsel-grants', grants, *args])
            actual = run(label + '-' + engine, command, runtime=True)
            assert actual == expected, (label, engine, actual, expected, str(output))
            observations += 1
    # Repeat a successful run byte-for-byte to control output determinism.
    grants.write_text(json.dumps(dict(io=dict(stdout='stdout.bytes@1', dirs=[
        dict(path='fixtures', host=str(data), access='read')]), args=['success.csv'])))
    for engine in (() if selected else ('h', 'llvm', 'cranelift')):
        command = ([hirsel, 'run', selection, '--grants', grants] if engine == 'h' else
                   [binaries[engine], '--hirsel-grants', grants, 'success.csv'])
        assert run('repeat-' + engine, command, runtime=True) == oracle((data / 'success.csv').read_text())
        observations += 1
    assert frozen == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in data.iterdir()}, 'host writes'
    for path in data.iterdir():
        path.unlink()
    data.rmdir()
    grants.unlink()
    assert not data.exists() and not grants.exists(), 'cleanup'
    result = dict(cases=len(cases), observations=observations, engines=['h', 'llvm', 'cranelift'])
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--one':
        prefix = Path(sys.argv[2])
        resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
        with prefix.with_suffix('.stdout').open('wb') as out, prefix.with_suffix('.stderr').open('wb') as err:
            done = subprocess.run(sys.argv[3:], stdin=subprocess.DEVNULL, stdout=out, stderr=err)
        prefix.with_suffix('.status').write_text(str(done.returncode))
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--hirsel', required=True, type=Path)
        parser.add_argument('--root', required=True, type=Path)
        parser.add_argument('--output', required=True, type=Path)
        parser.add_argument('--cases', help='Comma-separated affected cells; omit for the full original matrix')
        args = parser.parse_args()
        verify(args.hirsel.resolve(), args.root.resolve(), args.output.resolve(), set(args.cases.split(',')) if args.cases else None)
