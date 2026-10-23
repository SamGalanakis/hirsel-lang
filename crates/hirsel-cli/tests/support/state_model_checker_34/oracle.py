"""Independent Boolean closure and exhaustive bounded edge-walk oracles.
No SCC or parent-search implementation is shared with the application.
NetworkX BSD-3-Clause vectors; see examples/statecheck/REFERENCES.md.
"""
import copy
import json
from pathlib import Path


def graph(n, edges=(), terminal=(), forbidden=(), initial=0, name='g'):
    return dict(id=name, states=[dict(id=f's{i}') for i in range(n)],
                edges=[dict(source=a, target=b, label=f'e{i}') for i, (a,b) in enumerate(edges)],
                initial=initial, terminal=list(terminal), forbidden=list(forbidden))


def closure(g, absorbing):
    n=len(g['states'])
    a=[[False]*n for _ in range(n)]
    for e in g['edges']:
        if not absorbing or e['source'] not in g['terminal']:
            a[e['source']][e['target']]=True
    for k in range(n):
        for i in range(n):
            for j in range(n):
                a[i][j] |= a[i][k] and a[k][j]
    return a


def walks(g, start, goal, absorbing=False, members=None):
    # Enumerate edge walks by length; distinct visited paths stay distinct.
    # A shortest path is simple, so at most n-1 edges suffice.
    layer=[(start, [])]
    for _ in range(len(g['states'])):
        for node, path in layer:
            if goal(node): return path
        following=[]
        for node, path in layer:
            if absorbing and node in g['terminal']: continue
            for i,e in enumerate(g['edges']):
                if e['source']==node and (members is None or e['target'] in members):
                    vertices=[start]+[g['edges'][k]['target'] for k in path]
                    if e['target'] not in vertices:
                        following.append((e['target'], path+[i]))
        layer=following
        if not layer: break
    return None


def order(g, absorbing=False):
    # Ordering from all simple walk prefixes, independent of BFS parent storage.
    start=g['initial']; paths=[(start, [])]; seen=[]
    for _ in range(len(g['states'])):
        next_paths=[]
        for node,path in paths:
            if node not in seen: seen.append(node)
            if absorbing and node in g['terminal']: continue
            for i,e in enumerate(g['edges']):
                if e['source']==node and e['target'] not in seen:
                    next_paths.append((e['target'],path+[i]))
        paths=next_paths
        if not paths: break
    return seen


def expected(g):
    # Large cap graphs use closed forms, avoiding exponential walk enumeration.
    if len(g['states'])>8:
        return large_expected(g)
    reach=order(g); a=closure(g,True); root=g['initial']
    active=[i for i in range(len(a)) if i not in g['terminal'] and (i==root or a[root][i])]
    comp=[min([j for j in active if j==i or a[i][j] and a[j][i]]) if i in active else 64 for i in range(len(a))]
    safety=walks(g,root,lambda i:i in g['forbidden'])
    dead=walks(g,root,lambda i:i not in g['terminal'] and not any(e['source']==i for e in g['edges']))
    out=f"model {g['id']}\nreachable"+''.join(f' {i}' for i in reach)+'\ncomponents'+''.join(f' {i}' for i in comp)+'\n'
    for prop,good,bad,path in [('safety','safe','unsafe',safety),('deadlock','clear','stuck',dead)]:
        out+=f'{prop} {good}\n' if path is None else f'{prop} {bad} {root}'+''.join(f' {i}' for i in path)+'\n'
    cyclic=next((i for i in order(g,True) if i in active and a[i][i]),None)
    if cyclic is None: return out+'termination terminates\nend\n'
    members={j for j in active if comp[j]==comp[cyclic]}
    edge=next(i for i,e in enumerate(g['edges']) if e['source']==cyclic and e['target'] in members)
    prefix=walks(g,root,lambda i:i==cyclic,True)
    cycle=[edge]+walks(g,g['edges'][edge]['target'],lambda i:i==cyclic,True,members)
    return out+f'termination loop {root}'+''.join(f' {i}' for i in prefix)+' |'+''.join(f' {i}' for i in cycle)+'\nend\n'


def large_expected(g):
    # Frozen large vectors have all states reachable in declaration order,
    # initial terminal (isolated absorption), or a chain ending in a terminal.
    n=len(g['states']); root=g['initial']
    reach=order(g)
    a=closure(g,True)
    active=[i for i in range(n) if i not in g['terminal'] and (i==root or a[root][i])]
    comp=[min(j for j in active if i==j or a[i][j] and a[j][i]) if i in active else 64 for i in range(n)]
    out=f"model {g['id']}\nreachable"+''.join(f' {i}' for i in reach)+'\ncomponents'+''.join(f' {i}' for i in comp)+'\nsafety safe\ndeadlock clear\n'
    assert not any(a[i][i] for i in active)
    return out+'termination terminates\nend\n'


def vectors():
    nx=[(8,[(0,1),(1,2),(1,7),(2,3),(2,6),(3,4),(4,2),(4,5),(6,3),(6,5),(7,0),(7,6)]),
        (4,[(0,1),(0,2),(0,3),(3,1),(2,3),(1,2)]),
        (3,[(0,1),(1,2),(2,1),(1,0)]),
        (7,[(0,1),(1,2),(1,3),(2,4),(2,5),(3,4),(3,5),(4,6)]),
        (5,[(0,1),(1,2),(1,3),(1,4),(2,0),(2,3),(3,4),(4,3)])]
    rows=[]
    def add(name, models=None, error=None, **kw):
        row=dict(id=name,input=json.dumps(dict(version=1,models=models or [graph(1,terminal=[0])]),separators=(',',':')),jobs=2,exit=2 if error else 0,stdout='' if error else ''.join(expected(g) for g in models or [graph(1,terminal=[0])]),stderr=f'statecheck: {error}\n' if error else '')
        row.update(kw); rows.append(row)
    for i,(n,e) in enumerate(nx): add(f'networkx-scc-{i}',[graph(n,e)])
    bfs=[(0,1),(1,2),(1,3),(2,4),(3,4)]
    add('networkx-bfs',[graph(5,bfs+[ (b,a) for a,b in bfs],forbidden=[4])])
    add('networkx-bfs-reverse',[graph(5,[(b,a) for a,b in bfs],initial=4,forbidden=[0])])
    add('isolated',[graph(1)])
    add('absorbing',[graph(3,[(0,1),(1,2),(2,2)],terminal=[1],forbidden=[2])])
    add('initial-terminal',[graph(2,[(0,1),(1,1)],terminal=[0],forbidden=[1])])
    add('self-loop',[graph(1,[(0,0)])])
    add('unreachable',[graph(3,[(1,2),(2,1)],terminal=[0],forbidden=[1])])
    add('ties',[graph(5,[(0,2),(0,1),(2,4),(1,3)],terminal=[3,4],forbidden=[3,4])])
    add('prefix-cycle',[graph(5,[(0,1),(1,2),(2,3),(3,2),(1,4)],terminal=[4])])
    add('terminal-forbidden',[graph(1,terminal=[0],forbidden=[0])])
    add('four-models',[graph(1,[(0,0)],name=f'g{i}') for i in range(4)])
    add('cap-states',[graph(64,[(i,i+1) for i in range(63)],terminal=[63])])
    add('cap-edges',[graph(17,[(0,i) for i in range(1,17)]+[(i,j) for i in range(1,8) for j in range(1,17)],terminal=list(range(17)))])
    add('over-states',[graph(65,terminal=[0])],'states')
    over=graph(17,[(0,i) for i in range(1,17)]+[(i,j) for i in range(1,8) for j in range(1,17)]+[(8,1)],terminal=list(range(17)))
    add('over-edges',[over],'edges')
    add('over-models',[graph(1,terminal=[0],name=f'g{i}') for i in range(5)],'models')
    add('empty-models',error='models',input='{"version":1,"models":[]}')
    for field,value,error in [('states',[],'states'),('initial',1,'initial'),('terminal',[1],'terminal'),('terminal',[0,0],'terminal'),('forbidden',[1],'forbidden'),('forbidden',[0,0],'forbidden'),('id','?','identifier')]:
        g=graph(1);g[field]=value;add(f'invalid-{field}-{len(rows)}',[g],error)
    g=graph(2);g['states'][1]['id']='s0';add('duplicate-state',[g],'duplicate_state')
    g=graph(1,[(0,0),(0,0)]);add('duplicate-edge',[g],'duplicate_edge')
    g=graph(1,[(0,1)]);add('bad-edge',[g],'edge')
    add('duplicate-model',[graph(1),graph(1)],'duplicate_model')
    add('schema-extra',error='schema',input='{"version":1,"models":[],"extra":0}')
    add('schema-missing',error='schema',input='{"version":1}')
    add('schema-type',error='schema',input='{"version":1,"models":true}')
    add('version',error='version',input='{"version":2,"models":[]}')
    add('json',error='json',input='{')
    add('cap-input',pad=32768)
    add('over-input',error='input_bytes',pad=32769)
    add('cap-jobs',jobs=4)
    add('zero-jobs',error='jobs',jobs=0)
    add('over-jobs',error='jobs',jobs=5)
    add('denial',error='input_grant',deny=True)
    add('output-denial',error='output_grant',output_deny=True)
    add('pending-cancel',error='canceled',pending=True)
    return rows


def validate(g, text):
    lines=text.splitlines(); root=g['initial']; a=closure(g,True)
    n=len(g['states']); active={i for i in range(n) if i not in g['terminal'] and (root==i or a[root][i])}
    comp=list(map(int,lines[2].split()[1:]));assert len(comp)==n
    for i in range(n):
        assert (comp[i]!=64)==(i in active)
        for j in active:
            if i in active: assert (comp[i]==comp[j])==(i==j or a[i][j] and a[j][i])
    def trace(start,ids,absorb=False):
        pos=start
        for i in ids:
            e=g['edges'][i];assert e['source']==pos
            if absorb: assert pos not in g['terminal']
            pos=e['target']
        return pos
    for line,field in [(lines[3],'forbidden'),(lines[4],'deadlock')]:
        parts=line.split()
        if parts[1] in ('unsafe','stuck'):
            ids=list(map(int,parts[3:]));assert int(parts[2])==root
            end=trace(root,ids)
            if field=='forbidden': goal=lambda i:i in g['forbidden']
            else: goal=lambda i:i not in g['terminal'] and not any(e['source']==i for e in g['edges'])
            assert goal(end)
            if n<=8: assert len(ids)==len(walks(g,root,goal))
    if ' loop ' in lines[5]:
        p,c=lines[5].split('|'); parts=p.split();assert int(parts[2])==root
        prefix=list(map(int,parts[3:])); cycle=list(map(int,c.split()))
        assert cycle and len(prefix)+len(cycle)<=128
        entry=trace(root,prefix,True); assert trace(entry,cycle,True)==entry


if __name__=='__main__':
    import sys
    path=Path(__file__).with_name('cases.json')
    rows=vectors()
    if '--freeze' in sys.argv: path.write_text(json.dumps(rows,indent=2)+'\n')
    else: assert json.loads(path.read_text())==rows
    for row in rows:
        if row['exit']==0:
            models=json.loads(row['input'])['models']
            reports=row['stdout'].split('end\n')[:-1]
            for g,report in zip(models,reports,strict=True): validate(g,report+'end\n')
    print(f'{len(rows)} frozen cases checked; closure, walk lengths, every witness edge and cycle')
