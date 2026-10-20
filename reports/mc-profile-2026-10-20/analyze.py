"""Summarize one collect.py session; each prediction uses its own trace's N=1."""
import gzip
import hashlib
import json
from pathlib import Path
import statistics
import sys

source = Path(sys.argv[1]).resolve()
output = Path(__file__).resolve().parent
session = json.loads((source / 'samples.json').read_text())
samples = session['samples']
programs = {}
for name in sorted({s['program'] for s in samples}):
    on = [s for s in samples if s['program'] == name and s['enabled']]
    off = {s['round']: s for s in samples if s['program'] == name and not s['enabled']}
    assert len(on) == len(off) == 10
    med = lambda f: statistics.median(f(s) for s in on)
    models = []
    for index in range(5):
        metrics = on[0]['model']['results'][index]
        models.append({key: med(lambda s: s['model']['results'][index][key]) for key in metrics})
    instruction_ratios = [s['counters']['instructions'] / off[s['round']]['counters']['instructions'] for s in on]
    programs[name] = dict(
        rounds=len(on), tasks=med(lambda s: s['model']['tasks']),
        segments=med(lambda s: s['model']['segments']), results=models,
        bounds={key: med(lambda s: s['model']['bounds'][key]) for key in on[0]['model']['bounds']},
        n4_speedup_range=[min(s['model']['results'][2]['speedup'] for s in on), max(s['model']['results'][2]['speedup'] for s in on)],
        instruction_ratio_on_off=statistics.median(instruction_ratios),
        segment_ns_median=med(lambda s: s['segment_ns_median']),
        sibling_busy_median=statistics.median(s['sibling_busy'] for s in on if s['sibling_busy'] is not None),
        sensitivity={cost: med(lambda s: s['sensitivity'][cost]) for cost in on[0]['sensitivity']},
    )
    # Retain every timed segment, not just a representative prediction.
    logs = []
    for sample in on:
        path = source / 'samples' / str(sample['round']) / f'{name}-True' / 'measurement'
        assert hashlib.sha256(path.read_bytes()).hexdigest() == sample['trace_sha256']
        logs.append(dict(round=sample['round'], log=path.read_text()))
    (output / f'{name}.json.gz').write_bytes(gzip.compress(json.dumps(logs).encode(), mtime=0))
report = dict(schema=1, seed=session['seed'], commit_ns=200, programs=programs, samples=samples)
(output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
print('| Program | N=1 | N=2 | N=4 | N=8 | N=16 |')
print('|---|---:|---:|---:|---:|---:|')
for name, program in programs.items():
    values = [f'{r["speedup"]:.2f} / {100*r["stall_share"]:.1f} / {100*r["commit_busy_share"]:.1f}' for r in program['results']]
    print('| ' + name + ' | ' + ' | '.join(values) + ' |')
print('\nN=4 detail:')
for name, p in programs.items():
    r=p['results'][2]
    print(name, 'speedup-range',p['n4_speedup_range'], 'ready-HOL',r['hol_ready_share'],
          'work/span',p['bounds']['work_span'],'segment median ns',p['segment_ns_median'],
          'profiler instructions on/off',p['instruction_ratio_on_off'])
