#!/usr/bin/env python3
"""Verify archived sample counts, hashes, counter runs and printed ratios."""
import gzip
import hashlib
import json
import math
from pathlib import Path
from statistics import median

HERE = Path(__file__).resolve().parent
index = json.loads((HERE / 'index.json').read_text())
data = {}
for name, expected in index['files'].items():
    compressed = (HERE / name).read_bytes()
    assert hashlib.sha256(compressed).hexdigest() == expected['compressed_sha256'], name
    raw = gzip.decompress(compressed)
    assert hashlib.sha256(raw).hexdigest() == expected['raw_sha256'], name
    assert len(raw) == expected['raw_bytes'], name
    data[name.removesuffix('.gz')] = json.loads(raw)

service = data['service.json']
compiler = data['compiler.json']
report = (HERE.parent / 'RESULTS-app-perf-refresh-2.md').read_text()
assert len(service['samples']) == 237 and len(service['preflight']) == 24
assert len(service['errors']) == 1
assert service['errors'][0]['cell'] == '7-hirsel-recorded-list-16'
assert len(compiler['cells']) == 32
assert sum(len(cell['runs']) for cell in compiler['cells']) == 320
assert compiler['wall_ratio_valid'] is False
assert compiler['toolchain']['hirsel_sha256'] == data['identity.json']['binaries'][
    compiler['toolchain']['hirsel']]

for row in service['samples'] + service['preflight']:
    assert row['server_exit'] == 0 and row.get('errors', 0) == 0
    assert all(value == 100 for value in row['running_percent'].values())

def cost(rows, event):
    return median(row['perf'][event] / (
        1 if row['operation'] in ('cold', 'restart', 'sse') else row['completed'])
        for row in rows)

for op, rate in [('cold', 0), ('restart', 0), ('create', 16), ('list', 16),
                 ('search', 16), ('update', 16), ('sse', 1), ('sse', 100)]:
    groups = [[row for row in service['samples']
               if (row['operation'], row['rate'], row['kind']) == (op, rate, kind)]
              for kind in ('hirsel', 'rust-one-worker', 'hirsel-recorded')]
    h, rust, rec = groups
    assert {row['round'] for row in h} == set(range(10))
    assert {row['round'] for row in rust} == set(range(10))
    ratios = [cost(h, e) / cost(rust, e) for e in ('instructions', 'instructions:u')]
    tail = ('INCOMPLETE (7/10) | INCOMPLETE (7/10)' if op == 'list' else
            ' | '.join(f'{cost(rec, e) / cost(h, e):.6f}'
                       for e in ('instructions', 'instructions:u')))
    assert len(rec) == (7 if op == 'list' else 10)
    expected = f'| {op}/{rate} | {ratios[0]:.6f} | {ratios[1]:.6f} | {tail} |'
    assert expected in report, expected

by_key = {(cell['program'], cell['language'], cell['mode'], cell['temperature']): cell
          for cell in compiler['cells']}
for cell in compiler['cells']:
    assert cell['status'] == 'ok'
    assert {row['round'] for row in cell['runs']} == set(range(10))
    for row in cell['runs']:
        assert row['status'] == 'ok' and row['exit'] == 0
        assert all(value == 100 for value in row['running_percent'].values())
for pair in compiler['pairs']:
    h = by_key[pair['program'], 'hirsel', pair['mode'], pair['temperature']]
    rust = by_key[pair['program'], 'rust', pair['rust_mode'], pair['temperature']]
    values = [median(r['perf'][event] for r in h['runs']) /
              median(r['perf'][event] for r in rust['runs'])
              for event in ('instructions', 'instructions:u')]
    assert math.isclose(values[1], pair['ratio'], rel_tol=1e-12)
    assert f'| {values[0]:.6f} / {values[1]:.6f} |' in report

body = data.get('body.json')
if body:
    assert len(body['runs']) == 80 and len(body['errors']) == 1
    assert 'No such file or directory' in body['errors'][0]['error']
    for name in ('hello', 'csvjson', 'tasks', 'syn10k'):
        groups = [[row for row in body['runs']
                   if (row['program'], row['language']) == (name, language)]
                  for language in ('hirsel', 'rust')]
        for rows in groups:
            assert {row['round'] for row in rows} == set(range(10))
            for row in rows:
                assert row['exit'] == 0 and row['status'] == 'ok'
                assert all(v == 100 for v in row['running_percent'].values())
        if name != 'tasks':
            ratios = [median(r['perf'][e] for r in groups[0]) /
                      median(r['perf'][e] for r in groups[1])
                      for e in ('instructions', 'instructions:u')]
            assert f'| {ratios[0]:.6f} / {ratios[1]:.6f} |' in report

for program in ('tasks', 'syn10k'):
    resident = data.get(f'resident-{program}.json')
    if resident:
        assert len(resident['runs']) == 60
        assert resident['shutdown'].startswith('natural')
        for mode in ('resident', 'disk', 'cold'):
            for edit in ('body', 'unchanged'):
                rows = [r for r in resident['runs'] if (r['mode'], r['edit']) == (mode, edit)]
                assert {r['round'] for r in rows} == set(range(10))
                for row in rows:
                    assert row['exit'] == 0 and row['workload_status'] == 'accepted'
                    assert row['instructions'] == row['client'].get('instructions', 0) + row['server'].get('instructions', 0)
                    assert row['client'].get('instructions:u', 0) > 0

print('PASS: archive hashes; 237 service + 24 preflight; 320 compiler; '
      '80 body-edit; 120 resident diagnostic requests; published instruction ratios')
