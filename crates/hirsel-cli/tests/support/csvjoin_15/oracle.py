#!/usr/bin/env python3
"""Independent stdlib csv row oracle; all subprocess evidence is retained.

Cookbook population vectors: rust-csv 05612e87 tests/tests.rs:1-65,
examples/data/smallpop.csv (MIT, Copyright (c) 2015 Andrew Gallant).
Full notice: examples/csvjoin/README.md. Adaptation: select region as join key
and use three population rows as left queries, preserving duplicate cities.
Other vectors are independently authored for the join contract.
"""
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys

CLI, ROOT = sys.argv[1], Path(sys.argv[2])
MANIFEST = ROOT / 'hirsel.json'
DATA = ROOT / 'data'
DATA.mkdir()
GRANTS = ROOT / 'grants.json'
GRANTS.write_text(json.dumps({'io': {'stdout': 'stdout.bytes@1',
    'stderr': 'stderr.bytes@1', 'dirs': [
        {'path': 'data', 'host': str(DATA), 'access': 'read'}]}}))
BASE = ['left.csv', 'right.csv', '--left-key', 'id', '--right-key', 'key']
sequence = 0
PHASE = os.environ.get("CSVJOIN_15_PHASE", "all")
assert PHASE in ["all", "tail-replay"]


def execute(command, label):
    global sequence
    sequence += 1
    stem = ROOT / f'{sequence:04}-{label}'
    stem.with_suffix('.command.json').write_text(json.dumps(list(map(str, command))))
    inputs = [MANIFEST, ROOT / 'main.hir', ROOT / 'oracle.py', GRANTS,
              DATA / 'left.csv', DATA / 'right.csv']
    def hashes():
        return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in inputs if path.exists()}
    before = hashes()
    stem.with_suffix('.inputs-before.json').write_text(json.dumps(before, indent=2))
    try:
        result = subprocess.run(command, capture_output=True, timeout=300)
    except subprocess.TimeoutExpired as error:
        stem.with_suffix('.stdout').write_bytes(error.stdout or b'')
        stem.with_suffix('.stderr').write_bytes(error.stderr or b'')
        stem.with_suffix('.exit').write_text('timeout')
        stem.with_suffix('.inputs-after.json').write_text(json.dumps(hashes(), indent=2))
        raise
    after = hashes()
    stem.with_suffix('.inputs-after.json').write_text(json.dumps(after, indent=2))
    stem.with_suffix('.stdout').write_bytes(result.stdout)
    stem.with_suffix('.stderr').write_bytes(result.stderr)
    stem.with_suffix('.exit').write_text(str(result.returncode))
    assert before == after, (label, 'consumed inputs changed', before, after)
    return result


engines = {'H': None}
for backend in ['llvm', 'cranelift']:
    executable = ROOT / backend
    built = execute([CLI, 'build', '--backend', backend, MANIFEST, '-o', executable],
                    f'build-{backend}')
    assert built.returncode == 0, (backend, built.stderr.decode())
    engines[backend] = executable


def launch(engine, args, *, record=None, replay=None):
    if engine == 'H':
        command = [CLI, 'run', MANIFEST, '--max-steps', 'unbounded']
        if replay:
            command += ['--replay', replay]
        else:
            command += ['--grants', GRANTS]
            if record:
                command += ['--record', record]
    else:
        command = [engines[engine]]
        if replay:
            command += ['--hirsel-replay', replay]
        else:
            if record:
                command += ['--hirsel-record', record]
            command += ['--hirsel-grants', GRANTS]
    # Replay restores recorded args; supplying a different argv would be rejected.
    if not replay:
        command += (['--'] if engine == 'H' else []) + args
    return command


def encoded(rows):
    text = io.StringIO(newline='')
    csv.writer(text, lineterminator='\n').writerows(rows)
    return text.getvalue().encode()


def read(data):
    # std CSV follows Go: ignore blank lines and normalize CRLF in quoted fields.
    return [row for row in csv.reader(io.StringIO(
        data.decode().replace('\r\n', '\n'), newline=''), strict=True) if row]


def joined(left, right, mode, left_key='id', right_key='key'):
    """Deliberately nested-loop oracle, with no shared join/index implementation."""
    lh, *lr = read(left)
    rh, *rr = read(right)
    li, ri = lh.index(left_key), rh.index(right_key)
    headers = list(lh)
    for j, name in enumerate(rh):
        if j != ri:
            while name in headers:
                name = 'right_' + name
            headers.append(name)
    result = [headers]
    for lrow in lr:
        found = False
        for rrow in rr:
            if lrow[li] == rrow[ri]:
                found = True
                result.append(lrow + [v for j, v in enumerate(rrow) if j != ri])
        if not found and mode == 'left':
            result.append(lrow + [''] * (len(rh) - 1))
    return result


# Ported cookbook values, read via csv rather than embedded prejoined output.
population = b'''city,region,country,population
Southborough,MA,United States,9686
Northbridge,MA,United States,14061
Westborough,MA,United States,29313
Marlborough,MA,United States,38334
Springfield,MA,United States,152227
Springfield,MO,United States,150443
Springfield,NJ,United States,14976
Springfield,OH,United States,64325
Springfield,OR,United States,56032
Concord,NH,United States,42605
'''
valid = [
    ('headers', b'id,left\n', b'key,right\n', 'id', 'key'),
    ('empty-left', b'id,left\n', b'key,right\nx,R\n', 'id', 'key'),
    ('empty-right', b'id,left\nx,L\n', b'key,right\n', 'id', 'key'),
    ('duplicates', b'id,left\nx,L1\ny,L2\nx,L3\n',
     b'key,right\nx,R1\nx,R2\ny,R3\n', 'id', 'key'),
    ('unmatched', b'id,left\nmissing,L0\nx,L1\nother,L2\n',
     b'key,right\nx,R\n', 'id', 'key'),
    ('empty-keys', b'id,left\n,L0\nx,L1\n,L2\n',
     b'key,right\n,R0\n,R1\nx,R2\n', 'id', 'key'),
    ('exact-keys', b'id,left\n01,L0\n1,L1\n 1,L2\n',
     b'key,right\n1,R0\n01,R1\n', 'id', 'key'),
    ('unicode', encoded([['id', '名前'], ['羊', 'café'], ['🌍', '東京']]),
     encoded([['key', '名前'], ['🌍', 'naïve'], ['羊', 'λ']]), 'id', 'key'),
    ('quoted', b'id,note\n"x,y","a ""quote""\nand a line"\n',
     b'key,note\n"x,y","comma, quote "" and\nnewline"\n', 'id', 'key'),
    ('crlf', b'id,left\r\n\r\nx,"L\r\nline"\r\n',
     b'key,right\r\nx,"R\r\nline"\r\n', 'id', 'key'),
    ('unterminated-final', b'id,left\nx,last', b'key,right\nx,final', 'id', 'key'),
    ('collision', b'id,name,right_name\nx,L,N\n',
     b'key,name,right_name\nx,R,S\n', 'id', 'key'),
    ('empty-key-only', b'id\n""\nx\n', b'key\n""\n""\n', 'id', 'key'),
    ('key-only', b'id\nx\ny\n', b'key\nx\nx\n', 'id', 'key'),
    ('empty-header-name', b',left\nx,L\n', b',right\nx,R\n', '', ''),
    ('cookbook', b'query,region\na,MA\nb,OR\nc,XX\n',
     population, 'region', 'region'),
]
cells = 0
streaming = []
left, right = b'id,left\nx,L\n', b'key,right\nx,R\n'
header = b'id,left,right\n'
if PHASE == 'all':
    for name, left, right, lk, rk in valid:
        (DATA / 'left.csv').write_bytes(left)
        (DATA / 'right.csv').write_bytes(right)
        for mode in ['inner', 'left']:
            expected = joined(left, right, mode, lk, rk)
            (ROOT / f'expected-{name}-{mode}.json').write_text(
                json.dumps(expected, ensure_ascii=False, indent=2) + '\n')
            args = ['left.csv', 'right.csv', '--left-key', lk, '--right-key', rk,
                    '--mode', mode]
            for engine in engines:
                result = execute(launch(engine, args), f'{engine}-{name}-{mode}')
                assert result.returncode == 0 and not result.stderr, (
                    name, mode, engine, result.returncode, result.stderr)
                assert read(result.stdout) == expected, (name, mode, engine,
                                                          read(result.stdout), expected)
                assert result.stdout.endswith(b'\n') and b'\r\n' not in result.stdout
                cells += 1

    left, right = b'id,left\nx,L\n', b'key,right\nx,R\n'
    # Label, left, right, additional flags, expected error fragment, exact stdout.
    # Failed right validation precedes output; late left failures retain the header.
    header = b'id,left,right\n'
    errors = [
        ('empty-file-left', b'', right, [], 'Header left row=1 byte=0', b''),
        ('empty-file-right', left, b'', [], 'Header right row=1 byte=0', b''),
        ('duplicate-left', b'id,id\nx,L\n', right, [], 'Header left row=1 byte=6', b''),
        ('duplicate-right', left, b'key,key\nx,R\n', [], 'Header right row=1 byte=8', b''),
        ('absent-left-key', b'other,left\nx,L\n', right, [], 'Header left row=1 byte=11', b''),
        ('absent-right-key', left, b'other,right\nx,R\n', [], 'Header right row=1 byte=12', b''),
        ('short-left', b'id,left\nx\n', right, [], 'Width left row=2 byte=10', header),
        ('wide-right', left, b'key,right\nx,R,extra\n', [], 'Width right row=2 byte=20', b''),
        ('bare-quote-left', b'id,left\nx,L"\n', right, [], 'Csv left row=2 byte=12 Syntax', header),
        ('unclosed-right', left, b'key,right\nx,"R', [], 'Csv right row=2 byte=14 Syntax', b''),
        ('invalid-utf8', left, b'key,right\nx,\xff\n', [], 'Csv right row=2 byte=14 InvalidUtf8', b''),
        ('right-bytes', left, right, ['--max-right-bytes', '13'], 'Limit right-bytes right row=2 byte=14', b''),
        ('right-rows', left, right, ['--max-right-rows', '0'], 'Limit right-rows right row=2 byte=14', b''),
        ('key-right', left, b'key,right\nxx,R\n', ['--max-key-bytes', '1'], 'Limit key-bytes right row=2 byte=15', b''),
        ('key-left', b'id,left\nxx,L\n', right, ['--max-key-bytes', '1'], 'Limit key-bytes left row=2 byte=13', header),
        ('row-right', left, right, ['--max-row-bytes', '9'], 'Limit row-bytes right row=1 byte=10', b''),
        ('row-left', b'id,left\nx,12345678\n', right, ['--max-row-bytes', '10'], 'Limit row-bytes left row=2 byte=19', header),
        ('output-rows', left, right, ['--max-output-rows', '0'], 'Limit output-rows left row=2 byte=12', header),
        ('duplicate-expansion', left, b'key,right\nx,R\nx,S\n', ['--max-output-rows', '1'], 'Limit output-rows left row=2 byte=12', header),
        ('output-header', left, right, ['--max-output-bytes', '13'], 'Limit output-bytes left row=1 byte=8', b''),
        ('output-data', left, right, ['--max-output-bytes', '19'], 'Limit output-bytes left row=2 byte=12', header),
        ('usage-mode', left, right, ['--mode', 'wrong'], 'Usage', b''),
        ('usage-number', left, right, ['--max-right-rows', '-1'], 'Usage', b''),
        ('usage-ceiling', left, right, ['--max-row-bytes', '1048577'], 'Usage', b''),
    ]
    for name, ldata, rdata, extra, message, expected in errors:
        (DATA / 'left.csv').write_bytes(ldata)
        (DATA / 'right.csv').write_bytes(rdata)
        for engine in engines:
            result = execute(launch(engine, BASE + ['--mode', 'left'] + extra),
                             f'{engine}-{name}')
            assert result.returncode == 1, (name, engine, result.returncode, result.stderr)
            assert message in result.stderr.decode(), (name, engine, message, result.stderr)
            assert result.stdout == expected, (name, engine, result.stdout, expected)
            cells += 1

    # Inclusive boundaries: exact raw byte / row / key / output ceilings succeed.
    (DATA / 'left.csv').write_bytes(left)
    (DATA / 'right.csv').write_bytes(right)
    for engine in engines:
        result = execute(launch(engine, BASE + ['--mode', 'inner',
            '--max-right-bytes', '14', '--max-right-rows', '1',
            '--max-key-bytes', '1', '--max-row-bytes', '10',
            '--max-output-rows', '1', '--max-output-bytes', '20']), f'{engine}-exact-caps')
        assert result.returncode == 0 and result.stdout == header + b'x,L,R\n', result
        cells += 1

    for engine in engines:
        for count in [32, 2048]:
            ldata = b'id,left\n' + b'x,L\n' * count
            (DATA / 'left.csv').write_bytes(ldata)
            result = execute(launch(engine, BASE + ['--mode', 'inner', '--stats']),
                             f'{engine}-stream-{count}')
            assert result.returncode == 0, result.stderr
            assert result.stdout == header + b'x,L,R\n' * count
            stats = dict(item.split('=') for item in result.stderr.decode().split())
            assert int(stats['left_rows']) == count
            assert int(stats['right_rows']) == 1
            assert int(stats['max_left_row_bytes']) == 8
            assert int(stats['file_buffer_bytes']) == 4096
            assert int(stats['output_rows']) == count
            assert int(stats['output_bytes']) == len(result.stdout)
            streaming.append({'engine': engine, 'input_bytes': len(ldata), **stats})

# A large unread tail must not be fetched when output expansion is refused.
# Recording separately snapshots files. The FileRead request events prove
# bounded reads by the join algorithm (not by the snapshot infrastructure), as well as
# the source-level one-row lifetime and the deterministic stats above.
(DATA / 'right.csv').write_bytes(right)
(DATA / 'left.csv').write_bytes(b'id,left\nx,L\n' + b'z,Z\n' * 1048576)
for engine in engines:
    tape = ROOT / f'{engine}-bounded.tape'
    result = execute(launch(engine, BASE + ['--mode', 'left', '--max-output-rows', '0'],
                            record=tape), f'{engine}-bounded-tail')
    assert result.returncode == 1 and result.stdout == header, result

# Record on all engines, remove both originals, replay every tape on every engine.
ldata = b'id,left\nx,"line\nquote """\n,empty\ny,unmatched\n'
rdata = b'key,right\nx,R1\nx,R2\n,empty\n'
expected = joined(ldata, rdata, 'left')
(DATA / 'left.csv').write_bytes(ldata)
(DATA / 'right.csv').write_bytes(rdata)
tapes = []
for engine in engines:
    tape = ROOT / f'{engine}.tape'
    result = execute(launch(engine, BASE + ['--mode', 'left'], record=tape),
                     f'{engine}-record')
    assert result.returncode == 0 and not result.stderr and read(result.stdout) == expected
    tapes.append((engine, tape, result.stdout))
DATA.rename(ROOT / 'originals-unavailable')
replays = 0
for origin, tape, output in tapes:
    for engine in engines:
        result = execute(launch(engine, [], replay=tape), f'{origin}-replay-{engine}')
        assert result.returncode == 0 and not result.stderr and result.stdout == output, (
            origin, engine, result.returncode, result.stderr)
        assert not DATA.exists()
        replays += 1
report = {'engines': len(engines), 'oracle_cells': cells, 'replay_cells': replays,
          'streaming_cells': len(streaming), 'streaming': streaming,
          'bounded_tail_cells': 3, 'subprocesses': sequence, 'phase': PHASE}
(ROOT / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))
