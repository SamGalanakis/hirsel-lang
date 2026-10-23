#!/usr/bin/env python3
"""Independent lifecycle oracle; no Hirsel implementation is imported.

CPython unittest runner.py:242-284 (PSF license) supplies run/finalize/report
ordering, test_runner.py:1264-1282 supplies failfast and empty-suite controls.
This model adds explicit bounded attempts and never clears a failed attempt.
"""
import copy
import json
from pathlib import Path


def case(identity='a', codes=(0,), **changes):
    result = dict(id=identity, argv=['/fixture/child', identity], tags=['fast'],
                  expect=0, retries=len(codes)-1, timeout_ms=100)
    result.update(changes)
    return result


def manifest(cases=(), **changes):
    result = dict(jobs=1, timeout_ms=1000, failure_limit=0,
                  suites=[dict(id='unit', cases=list(cases))])
    result.update(changes)
    return result


def shard(identity, count):
    value = 0
    for byte in identity.encode():
        value = (value * 31 + byte) % count
    return value


def expected(source, scripts, tag='-', shard_index=0, shards=1):
    rows = []
    failures = 0
    stopped = False
    for suite in source['suites']:
        for item in suite['cases']:
            why = ('tag' if tag != '-' and tag not in item['tags'] else
                   'shard' if shard(item['id'], shards) != shard_index else 'selected')
            attempts = []
            status = 'skipped' if why != 'selected' else 'not_run'
            if why == 'selected' and not stopped:
                for number, code in enumerate(scripts[item['id']][:item['retries']+1], 1):
                    value = ('passed' if code == item['expect'] else 'failed') if isinstance(code, int) else code
                    attempts.append(dict(number=number, status=value,
                                         exit=code if isinstance(code, int) else None, signal=None))
                    if value != 'passed':
                        failures += 1
                    stopped = bool(source['failure_limit'] and failures >= source['failure_limit'])
                    if value == 'passed' or stopped:
                        break
                status = ('passed' if all(a['status'] == 'passed' for a in attempts) else
                          'flaky' if attempts[-1]['status'] == 'passed' else 'failed')
            if status == 'not_run':
                why = 'failure_limit' if stopped else 'not_admitted'
            rows.append(dict(index=len(rows), id=item['id'], suite=suite['id'], status=status,
                             reason=why, attempts=attempts))
    return dict(version=1, cases=rows, selected=sum(r['status'] != 'skipped' for r in rows),
                passed=sum(r['status'] == 'passed' for r in rows),
                failed=sum(r['status'] in ['failed', 'flaky'] for r in rows),
                skipped=sum(r['status'] == 'skipped' for r in rows),
                not_run=sum(r['status'] == 'not_run' for r in rows), incomplete=0,
                attempts=sum(len(r['attempts']) for r in rows), failed_attempts=failures,
                expired=False, cutoff=stopped, output='discarded')


def vectors():
    rows = []
    def add(identity, source, scripts=None, args=None, **options):
        scripts = scripts or {c['id']: [0] for s in source['suites'] for c in s['cases']}
        args = args or ['run', 'manifest.json', 'report.json']
        tag = args[3] if len(args) > 3 else '-'
        si, sn = (int(args[4]), int(args[5])) if len(args) > 4 else (0, 1)
        report = expected(source, scripts, tag, si, sn)
        rows.append(dict(id=identity, manifest=source, scripts=scripts, args=args, report=report,
                         exit=0 if not (report['failed'] or report['not_run']) else 1, **options))
    add('empty', manifest())
    add('empty-suites', manifest(suites=[]))
    add('pass', manifest([case()]))
    add('fail', manifest([case()]), {'a': [7]})
    add('expected-nonzero', manifest([case(expect=7)]), {'a': [7]})
    add('retry-pass', manifest([case(codes=(7, 0))]), {'a': [7, 0]})
    add('retry-exhaust', manifest([case(codes=(7, 7, 7))]), {'a': [7, 7, 7]})
    add('spawn-denial', manifest([case()]), {'a': ['process_not_granted']}, refused=True)
    add('spawn-missing', manifest([case()]), {'a': ['spawn_not_found']}, absent=True)
    add('tag', manifest([case(), case('b', tags=['slow'])]), args=['run', 'manifest.json', 'report.json', 'fast'])
    add('shard', manifest([case(), case('b')]), args=['run', 'manifest.json', 'report.json', '-', '0', '2'])
    add('cutoff', manifest([case(), case('b')], failure_limit=1), {'a': [7], 'b': [0]})
    add('cutoff-retry', manifest([case(codes=(7, 0)), case('b')], failure_limit=1), {'a': [7, 0], 'b': [0]})
    add('jobs-four', manifest([case(chr(97+i)) for i in range(4)], jobs=4))
    add('multi-suite', manifest([case()], suites=[dict(id='unit', cases=[case()]), dict(id='other', cases=[case('b')])]))
    add('literal-argv', manifest([case(argv=['/fixture/child', '$HOME', '*.hir', 'a b', '; exit 7'])]))
    for mode in ['list','plan']:
        add(mode, manifest([case(),case('b',tags=['slow'])]), args=[mode,'manifest.json']+(['fast'] if mode=='plan' else []))
        row=rows[-1]; row['plan']=[dict(index=i,id=c['id'],suite='unit',selected=mode=='list' or i==0,reason='tag' if mode=='plan' and i==1 else 'selected') for i,c in enumerate(row['manifest']['suites'][0]['cases'])];row['exit']=0
    add('report-write-failure', manifest([case()]), write_failure=True);rows[-1]['exit']=1
    add('output-write-failure', manifest([case()]), output_failure=True);rows[-1]['exit']=1
    # Scripted provider cannot express time passage during wait. Expect with
    # empty stdin produces a provider error, so deadline tests use host fixtures.
    for identity, code, verdict in [('compat-pass',0,'PASS'),('compat-fail',7,'FAIL: exit 7')]:
        add(identity,manifest([case(argv=['/fixture/child'])]),{'a':[code]},args=['/fixture/child','100'])
        rows[-1].update(text='| OOO\n'+verdict+'\n',exit=0)
    return rows


def caps():
    rows=[]
    def valid(identity, source, **opts):
        scripts={c['id']:[0] for s in source['suites'] for c in s['cases']}
        rows.append(dict(id=identity,manifest=source,scripts=scripts,args=['run','manifest.json','report.json'],report=expected(source,scripts),exit=0,**opts))
    def invalid(identity, source, kind, **opts):
        rows.append(dict(id=identity,manifest=source,scripts={},args=['run','manifest.json','report.json'],exit=2,error=kind,**opts))
    for field,cap,over in [('jobs',4,5),('timeout_ms',30000,30001),('failure_limit',96,97)]:
        valid(field+'-cap',manifest(**{field:cap}))
        invalid(field+'-over',manifest(**{field:over}),field)
    valid('cases-32',manifest([case('c'+str(i)) for i in range(32)]))
    invalid('cases-33',manifest([case('c'+str(i)) for i in range(33)]),'cases')
    valid('suites-4',manifest(suites=[dict(id='s'+str(i),cases=[]) for i in range(4)]))
    invalid('suites-5',manifest(suites=[dict(id='s'+str(i),cases=[]) for i in range(5)]),'suites')
    valid('retries-2',manifest([case(retries=2)]))
    invalid('retries-3',manifest([case(retries=3)]),'retries')
    valid('attempt-ms-5000',manifest([case(timeout_ms=5000)]))
    invalid('attempt-ms-5001',manifest([case(timeout_ms=5001)]),'timeout_ms')
    for bound in [4096,4097]:
        item=case(argv=['/fixture/child','x'*(bound-len('/fixture/child')-2)])
        (valid('argv-4096',manifest([item])) if bound==4096 else invalid('argv-4097',manifest([item]),'argv_bytes'))
    valid('input-262144',manifest(),raw_bytes=262144)
    invalid('input-262145',manifest(),'input_bytes_limit',raw_bytes=262145)
    valid('argv-count-64',manifest([case(argv=['/fixture/child']+['']*63)]))
    invalid('argv-count-65',manifest([case(argv=['/fixture/child']+['']*64)]),'argv')
    valid('id-64',manifest([case('x'*64)]))
    invalid('id-65',manifest([case('x'*65)]),'id')
    valid('tags-8',manifest([case(tags=['t'+str(i) for i in range(8)])]))
    invalid('tags-9',manifest([case(tags=['t'+str(i) for i in range(9)])]),'tags')
    invalid('duplicate-case',manifest([case(),case()]),'duplicate_case')
    invalid('duplicate-suite',manifest(suites=[dict(id='s',cases=[]),dict(id='s',cases=[])]),'duplicate_suite')
    invalid('nul',manifest([case(argv=['/fixture/child','a\0b'])]),'argv_bytes')
    invalid('relative',manifest([case(argv=['child'])]),'program')
    return rows


if __name__ == '__main__':
    root=Path(__file__).parent
    (root/'cases.json').write_text(json.dumps(vectors(),indent=2)+'\n')
    (root/'caps.json').write_text(json.dumps(caps(),indent=2)+'\n')
