#!/usr/bin/env python3
"""Independent v1 interpreter and pre-implementation fixture freezer.

Casbin c35c93d29063b8bc3bc2ef30b09943d344a289f4 effect/RBAC semantics;
Apache-2.0. Graph fixed point is independent of Hirsel's depth-first validator.
Run --check to verify frozen outputs; no native/implementation code is called.
"""
import copy
import json
from pathlib import Path
import re
import sys


def compact(x):
    return json.dumps(x, ensure_ascii=False, separators=(',', ':'))


class Invalid(Exception):
    pass


def fail(code):
    raise Invalid(code)


def obj(v, keys):
    if not isinstance(v, dict) or set(v) != set(keys.split()):
        fail('shape')


def arr(v, maximum, code):
    if not isinstance(v, list):
        fail('shape')
    if len(v) > maximum:
        fail(code)
    return v


def ident(v):
    if not isinstance(v, str) or not 1 <= len(v.encode()) <= 32:
        fail('id')
    return v


def path(v, pattern=False):
    if not isinstance(v, str) or not 1 <= len(v.encode()) <= 256:
        fail('path')
    if pattern and v.endswith('/*'):
        v = v[:-2] or '/'
    if v == '/':
        return
    if not v.startswith('/') or any(not re.fullmatch(r'[A-Za-z0-9_.-]+', s) or s in ('.', '..') for s in v[1:].split('/')):
        fail('path')


def distinct(items, code):
    if len(set(items)) != len(items):
        fail(code)


def policy(v):
    obj(v, 'roles edges principals rules')
    roles = [ident(r) for r in arr(v['roles'], 64, 'roles_limit')]
    es = arr(v['edges'], 128, 'edges_limit')
    ps = arr(v['principals'], 64, 'principals_limit')
    rs = arr(v['rules'], 128, 'rules_limit')
    for e in es:
        obj(e, 'role parent')
        ident(e['role']); ident(e['parent'])
    for p in ps:
        obj(p, 'id roles'); ident(p['id'])
        for r in arr(p['roles'], 64, 'grants_limit'):
            ident(r)
    for r in rs:
        obj(r, 'id effect role action path owner')
        for key in ('id', 'role', 'action', 'owner'):
            ident(r[key])
        path(r['path'], True)
        if r['effect'] not in ('allow', 'deny'):
            fail('effect')
    distinct(roles, 'duplicate_role')
    distinct([p['id'] for p in ps], 'duplicate_principal')
    distinct([r['id'] for r in rs], 'duplicate_rule')
    distinct([(e['role'], e['parent']) for e in es], 'duplicate_edge')
    for p in ps:
        distinct(p['roles'], 'duplicate_grant')
    refs = [x for e in es for x in (e['role'], e['parent'])] + [x for p in ps for x in p['roles']] + [r['role'] for r in rs]
    if any(x not in roles for x in refs):
        fail('unknown_role')
    # Propagate longest reachable distances. Positive diagonal means a cycle.
    distance = {(e['role'], e['parent']): 1 for e in es}
    for mid in roles:
        for a in roles:
            for b in roles:
                if (a, mid) in distance and (mid, b) in distance:
                    distance[a, b] = max(distance.get((a, b), 0), distance[a, mid] + distance[mid, b])
    if any((r, r) in distance for r in roles):
        fail('cycle')
    if max(distance.values(), default=0) > 16:
        fail('role_depth')
    return copy.deepcopy(v)


def queries(v):
    arr(v, 32, 'queries_limit')
    for q in v:
        obj(q, 'id principal action resource')
        for key in ('id', 'principal', 'action'):
            ident(q[key])
        obj(q['resource'], 'path owner')
        path(q['resource']['path']); ident(q['resource']['owner'])
    distinct([q['id'] for q in v], 'duplicate_query')
    return v


def decision(p, q):
    if p is None:
        hits = []
    else:
        held = set(next((x['roles'] for x in p['principals'] if x['id'] == q['principal']), []))
        while True:
            expanded = held | {e['parent'] for e in p['edges'] if e['role'] in held}
            if expanded == held:
                break
            held = expanded
        hits = []
        for r in p['rules']:
            resource = q['resource']
            path_ok = resource['path'] == r['path'] or (r['path'].endswith('/*') and resource['path'].startswith(r['path'][:-1]))
            if r['role'] in held and r['action'] in ('*', q['action']) and r['owner'] in ('*', resource['owner']) and path_ok:
                hits.append(r)
    return dict(id=q['id'], allow=any(r['effect'] == 'allow' for r in hits) and all(r['effect'] != 'deny' for r in hits), rules=[r['id'] for r in hits])


def interpret(raw):
    if len(raw.encode()) > 131072:
        return '', '{"error":"input_bytes_limit"}\n', 2
    try:
        try:
            v = json.loads(raw)
        except (ValueError, RecursionError):
            fail('json')
        obj(v, 'version operations')
        if type(v['version']) is not int or v['version'] != 1:
            fail('version')
        arr(v['operations'], 32, 'operations_limit')
        installed = None
        revision = 0
        results = []
        for op in v['operations']:
            if not isinstance(op, dict) or len(op) != 1:
                fail('shape')
            if 'reload' in op:
                try:
                    replacement = policy(op['reload'])
                except Invalid as e:
                    results.append(dict(reload=False, error=str(e)))
                else:
                    installed = replacement
                    revision += 1
                    results.append(dict(reload=True, revision=revision))
            elif 'batch' in op:
                qs = queries(op['batch'])
                results.append(dict(revision=revision, decisions=[decision(installed, q) for q in qs]))
            else:
                fail('shape')
        out = compact(dict(results=results)) + '\n'
        if len(out.encode()) > 65536:
            fail('output_bytes_limit')
        return out, '', 0
    except Invalid as e:
        return '', compact(dict(error=str(e))) + '\n', 2


def rule(id='allow', role='reader', effect='allow', action='read', path='/data/*', owner='*'):
    return dict(id=id, effect=effect, role=role, action=action, path=path, owner=owner)


def query(id='q', principal='alice', action='read', path='/data/a', owner='alice'):
    return dict(id=id, principal=principal, action=action, resource=dict(path=path, owner=owner))


def base():
    return dict(roles=['member', 'reader', 'blocked'], edges=[dict(role='member', parent='reader')], principals=[dict(id='alice', roles=['member']), dict(id='bob', roles=['blocked'])], rules=[rule(), rule('deny', 'blocked', 'deny')])


def workflow(*ops):
    return dict(version=1, operations=list(ops))


def vectors():
    rows = []
    def add(id, value, **extra):
        raw = value if isinstance(value, str) else compact(value)
        out, err, status = interpret(raw)
        rows.append(dict(id=id, input=raw, stdout=out, stderr=err, exit=status, **extra))
    p = base()
    add('no-policy', workflow(dict(batch=[query()])))
    add('inherited-allow', workflow(dict(reload=p), dict(batch=[query(), query('deny', 'bob')])))
    p['edges'].append(dict(role='reader', parent='blocked'))
    add('inherited-deny', workflow(dict(reload=p), dict(batch=[query()])))
    p = base(); p['rules'] += [rule('second'), rule('conflict', effect='deny')]
    add('conflict-order', workflow(dict(reload=p), dict(batch=[query()])))
    p['rules'].reverse()
    add('reordered-explanations', workflow(dict(reload=p), dict(batch=[query()])))
    p = base()
    add('empty-match-boundary', workflow(dict(reload=p), dict(batch=[query('empty', action='write'), query('boundary', path='/database/a'), query('parent', path='/data'), query('nested', path='/data/a/b'), query('unknown', principal='nobody')])))
    p['rules'] = [rule(owner='alice')]
    add('owner-equality', workflow(dict(reload=p), dict(batch=[query('own'), query('other', owner='bob')])))
    bad = base(); bad['edges'] += [dict(role='reader', parent='member')]
    add('atomic-reload', workflow(dict(reload=base()), dict(batch=[query('before')]), dict(reload=bad), dict(batch=[query('after')]), dict(reload=dict(roles=[], edges=[], principals=[], rules=[])), dict(batch=[query('empty')])))
    for name, mutate in [
        ('unknown-role', lambda p: p['rules'][0].update(role='missing')),
        ('duplicate-role', lambda p: p['roles'].append('reader')),
        ('duplicate-rule', lambda p: p['rules'].append(copy.deepcopy(p['rules'][0]))),
        ('duplicate-principal', lambda p: p['principals'].append(copy.deepcopy(p['principals'][0]))),
        ('duplicate-edge', lambda p: p['edges'].append(copy.deepcopy(p['edges'][0]))),
        ('duplicate-grant', lambda p: p['principals'][0]['roles'].append('member')),
        ('bad-path', lambda p: p['rules'][0].update(path='/data/*/x')),
        ('dot-path', lambda p: p['rules'][0].update(path='/data/../x')),
        ('bad-effect', lambda p: p['rules'][0].update(effect='maybe')),
        ('extra-field', lambda p: p.update(extra=1)),
    ]:
        p = base(); mutate(p)
        add(name, workflow(dict(reload=base()), dict(reload=p), dict(batch=[query()])))
    # Casbin model_test.go TestRBACModel: all eight decisions.
    p = dict(roles=['alice', 'bob', 'data2_admin'], edges=[dict(role='alice', parent='data2_admin')], principals=[dict(id=x, roles=[x]) for x in ['alice', 'bob']], rules=[rule('alice-data1', 'alice', path='/data1'), rule('bob-data2', 'bob', action='write', path='/data2'), rule('admin-data2', 'data2_admin', action='*', path='/data2')])
    add('casbin-rbac-eight', workflow(dict(reload=p), dict(batch=[query(f'{u}-{d}-{a}', u, a, f'/{d}') for u in ['alice', 'bob'] for d in ['data1', 'data2'] for a in ['read', 'write']])))
    # RoleManager TestRole: all twelve role reachability decisions.
    p = dict(roles=['u1', 'u2', 'u3', 'u4', 'g1', 'g2', 'g3'], edges=[dict(role=a, parent=b) for a,b in [('u1','g1'),('u2','g1'),('u3','g2'),('u4','g2'),('u4','g3'),('g1','g3')]], principals=[dict(id=u, roles=[u]) for u in ['u1','u2','u3','u4']], rules=[rule(g,g,path=f'/{g}') for g in ['g1','g2','g3']])
    add('casbin-role-twelve', workflow(dict(reload=p), dict(batch=[query(f'{u}-{g}',u,path=f'/{g}') for u in ['u1','u2','u3','u4'] for g in ['g1','g2','g3']])))
    for kind, maximum in [('roles',64),('principals',64),('edges',128),('rules',128)]:
        for n in [maximum,maximum+1]:
            p = base()
            if kind == 'roles': p['roles'] = p['roles'] + [f'r{i}' for i in range(n-3)]
            if kind == 'principals': p['principals'] = [dict(id=f'p{i}',roles=['reader']) for i in range(n)]
            if kind == 'rules': p['rules'] = [rule(f'r{i}') for i in range(n)]
            if kind == 'edges':
                p['roles'] = [f'r{i}' for i in range(24)]
                p['principals'] = []; p['rules'] = []
                p['edges'] = [dict(role=f'r{i}',parent=f'r{j}') for i in range(12) for j in range(12,24)][:n]
            add(f'cap-{kind}-{n}', workflow(dict(reload=p)))
    for n in [16,17]:
        p = base(); p['roles'] = [f'r{i}' for i in range(n+1)]
        p['principals'] = []; p['rules'] = []
        p['edges'] = [dict(role=f'r{i}',parent=f'r{i+1}') for i in range(n)]
        add(f'cap-depth-{n}', workflow(dict(reload=p)))
    for n in [32,33]:
        add(f'cap-queries-{n}', workflow(dict(reload=base()), dict(batch=[query(f'q{i}') for i in range(n)])))
        add(f'cap-operations-{n}', workflow(*[dict(batch=[]) for _ in range(n)]))
    for n in [64,65]:
        p = base(); p['roles'] = [f'r{i}' for i in range(64)]
        p['rules'] = []; p['edges'] = []; p['principals'] = [dict(id='alice',roles=p['roles'] + (['r0'] if n == 65 else []))]
        add(f'cap-grants-{n}', workflow(dict(reload=p)))
    for n in [32,33]:
        p = base(); p['rules'][0]['id'] = 'x'*n
        add(f'cap-id-{n}', workflow(dict(reload=p)))
    for n in [256,257]:
        p = base(); p['rules'][0]['path'] = '/'+'x'*(n-1)
        add(f'cap-path-{n}', workflow(dict(reload=p)))
    for n in [131072,131073]:
        raw = compact(workflow())
        add(f'cap-input-{n}', raw + ' '*(n-len(raw)))
    # Exactly hit the complete-output byte bound using a legal query ID.
    p = base(); p['rules'] = [rule(f'{i:03d}'+'x'*29) for i in range(64)]
    qs = [query(f'q{i}') for i in range(32)]
    w = workflow(dict(reload=p), dict(batch=qs))
    out = compact(dict(results=[dict(reload=True,revision=1),dict(revision=1,decisions=[decision(p,q) for q in qs])]))+'\n'
    # Start above the target, remove rule ID bytes evenly, then adjust query ID.
    while len(out.encode()) > 65536:
        r = max(p['rules'], key=lambda r:len(r['id']))
        r['id'] = r['id'][:-1]
        out,_,_ = interpret(compact(w))
        # interpret returns blank on cap; use the unconstrained decision output here.
        out = compact(dict(results=[dict(reload=True,revision=1),dict(revision=1,decisions=[decision(p,q) for q in qs])]))+'\n'
    delta = 65536-len(out.encode())
    # One ID can contribute <=31 extra bytes; the decrement above was 32.
    assert delta <= 31
    qs[0]['id'] += 'x'*delta
    assert len(interpret(compact(w))[0].encode()) == 65536
    add('cap-output-65536', w)
    qs[1]['id'] += 'x'
    add('cap-output-65537', w)
    add('domain-json', '{')
    add('version', dict(version=2, operations=[]))
    add('duplicate-query', workflow(dict(batch=[query(),query()])))
    add('pending-cancellation', workflow(), pending=True, stdout_override='', stderr_override='{"error":"canceled"}\n', exit_override=2)
    for r in rows:
        for key in ['stdout','stderr','exit']:
            if key+'_override' in r: r[key] = r.pop(key+'_override')
    add('cap-jobs-4', workflow(dict(reload=base()),dict(batch=[query()])), jobs=4)
    add('cap-jobs-5', workflow(), jobs=5)
    rows[-1].update(stdout='',stderr='{"error":"usage"}\n',exit=2)
    return dict(schema=1,cases=rows)


if __name__ == '__main__':
    target = Path(__file__).with_name('cases.json')
    if '--check' in sys.argv:
        frozen = json.loads(target.read_text())
        assert frozen == vectors()
        print(f"{len(frozen['cases'])} frozen independent vectors verified")
    else:
        target.write_text(json.dumps(vectors(),ensure_ascii=False,indent=2)+'\n')
