#!/usr/bin/env python3
"""Independent hashlib + transitive-closure oracle; never calls Hirsel's graph code.
CPython graphlib vectors: test_graphlib.py:40-79, PSF2 (app REFERENCES.md).
"""
import copy
import hashlib
import json
from pathlib import Path


def framed(parts):
    h = hashlib.sha256()
    for p in parts:
        if isinstance(p, str):
            p = p.encode()
        h.update(str(len(p)).encode() + b':' + p)
    return h.hexdigest()


def task(name, deps=(), inputs=(), outputs=(), mode='ok'):
    return dict(name=name, argv=['/fixture/child', mode, name],
                dependencies=list(deps), inputs=list(inputs), outputs=list(outputs))


def closure(tasks):
    names = [t['name'] for t in tasks]
    reach = {(names.index(d), i) for i,t in enumerate(tasks) for d in t['dependencies']}
    # Floyd-Warshall, independently from Kahn/readiness implementation.
    for k in range(len(tasks)):
        for i in range(len(tasks)):
            for j in range(len(tasks)):
                if (i,k) in reach and (k,j) in reach:
                    reach.add((i,j))
    return reach


def record(tasks, files):
    pending = set(range(len(tasks)))
    values, rows = {}, {}
    while pending:
        ready = [i for i in sorted(pending) if all(d in values for d in tasks[i]['dependencies'])]
        assert ready, 'cyclic oracle input'
        for i in ready:
            t=tasks[i]
            parts=['dagbuild-1',t['name'],'argv',str(len(t['argv'])),*t['argv'],'inputs',str(len(t['inputs']))]
            for p in t['inputs']: parts.extend([p,files[p]])
            parts.extend(['predecessors',str(len(t['dependencies']))])
            for d in t['dependencies']: parts.extend([str(next(j for j,x in enumerate(tasks) if x['name']==d)),values[d]])
            parts.extend(['outputs',str(len(t['outputs'])),*t['outputs']])
            key=framed(parts)
            outputs=[dict(path=p,digest=hashlib.sha256(files[p].encode()).hexdigest()) for p in t['outputs']]
            result=framed(['dagbuild-result-1',key,*[s for a in outputs for s in (a['path'],a['digest'])]])
            rows[t['name']]=dict(name=t['name'],record=dict(fingerprint=key,digest_result=result,outputs=outputs))
            values[t['name']]=result
            pending.remove(i)
    return dict(version=1,tasks=[rows[t['name']] for t in tasks])


def cases():
    out=[]
    def add(identity,tasks,files=None,prior=None,mode='build',error=None,exit=0,denied=None,fast=False):
        files=files or {}
        manifest=dict(tasks=tasks,jobs=2,fail_fast=fast)
        row=dict(id=identity,manifest=manifest,files=files,prior=prior,args=[mode,'manifest.json'],exit=exit,denied=denied)
        if error:
            row.update(rows=[],stderr=json.dumps(dict(kind=error),separators=(',',':'))+'\n',spawned=0,state=None)
        else:
            reach=closure(tasks)
            failed={i for i,t in enumerate(tasks) if t['argv'][1]=='fail' or denied=='spawn'}
            blocked={j for i in failed for j in range(len(tasks)) if (i,j) in reach}
            expected=record(tasks,files)
            records={r['name']:r['record'] for r in expected['tasks']}
            old={r['name']:r['record'] for r in (prior or {}).get('tasks',[])} if isinstance(prior,dict) else {}
            rows=[]; kept=[]; spawned=0
            dirty=set()
            # Evaluate dirty closure before report declaration order.
            for i,t in enumerate(tasks):
                oldrow=old.get(t['name'])
                if oldrow!=records[t['name']]: dirty.add(i)
                elif any(p not in files or hashlib.sha256(files[p].encode()).hexdigest()!=a['digest'] for p,a in zip(t['outputs'],oldrow['outputs'])): dirty.add(i)
            dirty|={j for i in list(dirty) for j in range(len(tasks)) if (i,j) in reach}
            for i,t in enumerate(tasks):
                key=records[t['name']]; oldrow=old.get(t['name'])
                reason='no_record' if oldrow is None else 'fingerprint_changed' if oldrow['fingerprint']!=key['fingerprint'] else 'unchanged'
                if oldrow and oldrow['fingerprint']==key['fingerprint']:
                    for p,a in zip(t['outputs'],oldrow['outputs']):
                        if p not in files: reason='output_missing';break
                        if hashlib.sha256(files[p].encode()).hexdigest()!=a['digest']:reason='output_changed';break
                status='clean' if i not in dirty else 'built'
                result=key['digest_result']
                if mode=='plan' and i in dirty:
                    status='dirty';result=''
                    if any((p,i) in reach for p in dirty if p!=i):reason='dependency_dirty'
                elif i in blocked: status='blocked';reason='dependency_failed';result=''
                elif i in failed:status='failed';reason='spawn_denied:-1' if denied=='spawn' else 'exit:7';result='';spawned+=1
                elif status=='built':spawned+=1
                if status in ['built','clean']:kept.append(dict(name=t['name'],record=key))
                rows.append(dict(name=t['name'],status=status,reason=reason,digest_result=result))
            rows.append(dict(tasks=len(tasks),expired=False,failed=bool(failed) and mode!='plan'))
            row.update(rows=rows,stderr='',spawned=spawned if mode=='build' else 0,state=dict(version=1,tasks=kept) if mode=='build' else prior)
        out.append(row)
    add('empty',[])
    chain=[task('a',inputs=['in']),task('b',['a']),task('c',['b'])]
    files={'in':'alpha'}
    add('chain-first',chain,files)
    diamond=[task('a'),task('b',['a']),task('c',['a']),task('d',['b','c']),task('e')]
    add('diamond-first',diamond)
    add('disconnected-first',[task('a'),task('b',['a']),task('x'),task('y',['x'])])
    add('reverse-chain',[task('c',['b']),task('b',['a']),task('a')])
    old=record(chain,files)
    add('unchanged',chain,files,old)
    add('unchanged-plan',chain,files,old,mode='plan')
    add('input-change',chain,{'in':'beta'},old)
    add('input-change-plan',chain,{'in':'beta'},old,mode='plan')
    changed=copy.deepcopy(chain);changed[0]['argv'][1]='changed'
    add('command-change',changed,files,old)
    add('failure-descendants',[task('a',mode='fail'),task('b',['a']),task('c',['b']),task('x'),task('y',['x'])],exit=1)
    add('denied-spawn',[task('a'),task('b',['a'])],exit=1,denied='spawn')
    artifacts=[task('a',outputs=['out'])]
    add('outputs-first',artifacts,{'out':'old'})
    old_art=record(artifacts,{'out':'old'})
    add('outputs-clean',artifacts,{'out':'old'},old_art)
    add('changed-output',artifacts,{'out':'new'},old_art)
    # Missing output in plan does not need a command side effect to restore it.
    missing=copy.deepcopy(out[-1]);missing.update(id='missing-output',files={},prior=old_art,args=['plan','manifest.json'],exit=0,rows=[dict(name='a',status='dirty',reason='output_missing',digest_result=''),dict(tasks=1,expired=False,failed=False)],spawned=0,state=old_art);out.append(missing)
    add('plan-first',diamond,mode='plan')
    add('one-input-first',[task('a',inputs=['in'])],{'in':'alpha'})
    single=[task('a',inputs=['in'])]; single_old=record(single,{'in':'alpha'})
    add('one-input-change',single,{'in':'beta'},single_old)
    add('one-command-change',[task('a',inputs=['in'],mode='changed')],{'in':'alpha'},single_old)
    add('two-chain',[task('a'),task('b',['a'])])
    add('small-failure',[task('a',mode='fail'),task('b',['a'])],exit=1)
    tie=[task('a',['b']),task('b'),task('c')]
    old_tie=record(tie,{})
    old_tie['tasks']=[r for r in old_tie['tasks'] if r['name']=='b']
    add('clean-ready-tie',tie,prior=old_tie)
    add('cycle',[task('a',['b']),task('b',['a'])],error='cycle',exit=2)
    add('unknown',[task('a',['missing'])],error='unknown_dependency',exit=2)
    add('duplicate-output',[task('a',outputs=['out']),task('b',outputs=['out'])],error='duplicate_output',exit=2)
    add('output-ancestor',[task('a',outputs=['out']),task('b',outputs=['out/file'])],error='duplicate_output',exit=2)
    add('state-corrupt',[],prior='broken',error='state_corrupt',exit=2)
    add('denied-read',[],error='manifest_read',exit=3,denied='missing')
    add('denied-read-grant',[],error='project_grant',exit=3,denied='read')
    add('denied-write',[],error='state_write',exit=3,denied='write')
    add('duplicate-task',[task('a'),task('a')],error='duplicate_task',exit=2)
    # Independent graphlib upstream shape, all vertices explicitly declared.
    add('cpython-disconnected',[task(str(i),[str(d) for d in {0:[1,2],1:[],2:[3],3:[],4:[5],5:[6],6:[]}[i]]) for i in range(7)])
    fast=[task('a',mode='fail'),task('x')]
    add('fail-fast',fast,exit=1,fast=True)
    out[-1]['rows'][1]=dict(name='x',status='canceled',reason='canceled',digest_result='')
    out[-1]['state']=dict(version=1,tasks=[])
    return out

if __name__=='__main__':
    Path(__file__).with_name('cases.json').write_text(json.dumps(cases(),indent=2)+'\n')
