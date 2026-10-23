"""Independent closure oracle and pinned Go MVS build-list vectors (BSD-3-Clause).
License: examples/depsolve/LICENSE-Go (Copyright 2018 The Go Authors).
Go mvs_test.go:16-79, ebe2feefc400f813aefdc8a036e66d2619da6649.
Expected selection uses a set fixed point, independently of the app's BFS.
"""
import copy
import hashlib
import json


def encoded(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False)


def req(name, version=1):
    return dict(name=name, version=f'{version}.0.0' if isinstance(version, int) else version)


def pkg(name, version=1, deps=()):
    return dict(**req(name, version), dependencies=list(deps))


def oracle(root, catalogs):
    nodes = {(p['name'], p['version']): p for c in catalogs for p in c['packages']}
    reached = {(r['name'], r['version']) for r in root['dependencies']}
    while True:
        expanded = reached | {(d['name'], d['version']) for key in reached for d in nodes[key]['dependencies']}
        if expanded == reached:
            break
        reached = expanded
    selected = {}
    for name, version in reached:
        selected[name] = max(selected.get(name, (0, 0, 0)), tuple(map(int, version.split('.'))))
    return [f'{name}@' + '.'.join(map(str, v)) for name, v in sorted(selected.items())]


def lock(root_bytes, files):
    root = json.loads(root_bytes)
    catalogs = [json.loads(data) for _, data in sorted(files.items())]
    expected = oracle(root, catalogs)
    nodes = {(p['name'], p['version']): p for c in catalogs for p in c['packages']}
    # Independent shortest-path relaxation; ties use lexicographic full paths.
    paths = {(r['name'], r['version']): ['$root@0.0.0', r['name']+'@'+r['version']] for r in root['dependencies']}
    while True:
        changed = False
        for key, path in list(sorted(paths.items(), key=lambda x: (len(x[1]), x[1]))):
            for d in sorted(nodes[key]['dependencies'], key=lambda d: (d['name'], d['version'])):
                child = (d['name'], d['version'])
                candidate = path + [d['name']+'@'+d['version']]
                if child not in paths or (len(candidate), candidate) < (len(paths[child]), paths[child]):
                    paths[child] = candidate
                    changed = True
        if not changed:
            break
    source = lambda name, data: dict(file=name, sha256=hashlib.sha256(data.encode()).hexdigest())
    rows = []
    for item in expected:
        name, version = item.split('@')
        rows.append(dict(name=name, version=version, path=paths[(name, version)]))
    return dict(schema=1, root=source('root.json', root_bytes), catalogs=[source(name, data) for name, data in sorted(files.items())], selected=rows, visited=sorted(n+'@'+v for n, v in paths))


def cases():
    out = []
    def add(identity, roots, packages, literal=None):
        root = dict(dependencies=roots)
        files = {'catalog.json': encoded(dict(packages=packages))} if packages else {}
        text = encoded(root)
        if literal is not None:
            assert oracle(root, [json.loads(x) for x in files.values()]) == literal
        report = lock(text, files)
        out.append(dict(id=identity, args=['resolve', 'root.json', '4']+list(files), files=dict(files, **{'root.json': text}), exit=0, output=report))
    add('empty', [], [], [])
    add('blog', [req('B'), req('C', 2)], [pkg('B', deps=[req('D', 3)]), pkg('C', 1, [req('D', 2)]), pkg('C', 2, [req('D', 4)]), pkg('C', 3, [req('D', 5)]), pkg('C', 4, [req('G')]), pkg('D', 2, [req('E')]), pkg('D', 3, [req('E', 2)]), pkg('D', 4, [req('E', 2), req('F')]), pkg('D', 5, [req('E', 2)]), pkg('E'), pkg('E', 2), pkg('F'), pkg('G', deps=[req('C', 4)])], ['B@1.0.0','C@2.0.0','D@4.0.0','E@2.0.0','F@1.0.0'])
    add('trim', [req('B'), req('C', 2)], [pkg('B', deps=[req('D', 3)]), pkg('B', 2), pkg('C', 2, [req('B', 2)]), pkg('D', 3)], ['B@2.0.0','C@2.0.0','D@3.0.0'])
    add('cross', [req('B'), req('C')], [pkg('B', deps=[req('D')]), pkg('C', deps=[req('D', 2)]), pkg('D', 1, [req('E', 2)]), pkg('D', 2, [req('E')]), pkg('E'), pkg('E', 2)], ['B@1.0.0','C@1.0.0','D@2.0.0','E@2.0.0'])
    add('cycle', [req('A')], [pkg('A', deps=[req('B')]), pkg('B', deps=[req('A')])], ['A@1.0.0','B@1.0.0'])
    add('numeric', [req('A',2),req('A',10)], [pkg('A',2),pkg('A',10)], ['A@10.0.0'])
    add('u64-max', [req('A','18446744073709551615.0.0')], [pkg('A','18446744073709551615.0.0')])
    def error(identity, base='empty', code='shape', detail='', path=None):
        case = copy.deepcopy(next(c for c in out if c['id']==base))
        case.update(id=identity, exit=2, output=dict(error=code, detail=detail, path=path or ['root.json']))
        out.append(case)
        return case
    c=error('duplicate', 'cross', 'duplicate', 'B@1.0.0', ['catalog.json']);data=json.loads(c['files']['catalog.json']);data['packages'].append(data['packages'][0]);c['files']['catalog.json']=encoded(data)
    c=error('missing','empty','missing','absent@1.0.0',['$root@0.0.0','absent@1.0.0']);c['files']['root.json']=encoded(dict(dependencies=[req('absent')]))
    for identity,version,code in [('range','^1.0.0','version'),('prerelease','1.0.0-rc.1','unstable'),('metadata','1.0.0+abc','unstable'),('overflow','18446744073709551616.0.0','version'),('invalid','01.0.0','version')]:
        c=error(identity,code=code,detail=version);c['files']['root.json']=encoded(dict(dependencies=[req('A',version)]))
    c=error('read-failure',code='open',path=['absent.json']);c['args'].append('absent.json')
    c=error('json',code='json');c['files']['root.json']='{'
    c=error('shape');c['files']['root.json']='{}'
    c=error('cleanup','cross','json',path=['a.json']);c['files']['a.json']='{';c['args']+=['a.json']
    for jobs in ['0','5']:
        c=error('jobs-'+jobs,code='limit',detail='jobs',path=['$argv']);c['args'][2]=jobs
    c=error('duplicate-file','cross','duplicate','catalog.json',['catalog.json']);c['args'].append('catalog.json')
    for identity in ['verify','stale-lock','changed-digest']:
        c=copy.deepcopy(next(c for c in out if c['id']=='cross'));c['id']=identity;c['files']['lock.json']=encoded(c['output']);c['args']=['verify-lock','lock.json']+c['args'][1:]
        if identity!='verify':
            if identity=='changed-digest': c['files']['catalog.json']+=' '
            else:
                value=json.loads(c['files']['lock.json']);value['selected'][0]['version']='9.0.0';c['files']['lock.json']=encoded(value)
            c.update(exit=1,output=dict(error='stale_lock',detail='',path=['lock.json']))
        out.append(c)
    for jobs in ['1','2','3']:
        c=copy.deepcopy(next(c for c in out if c['id']=='cross'));c['id']='jobs-'+jobs;c['args'][2]=jobs;out.append(c)
    c=copy.deepcopy(next(c for c in out if c['id']=='cross'));c['id']='split'
    packages=json.loads(c['files'].pop('catalog.json'))['packages']
    c['files']['a.json']=encoded(dict(packages=packages[:3]));c['files']['b.json']=encoded(dict(packages=packages[3:]))
    c['args']=['resolve','root.json','4','b.json','a.json']
    c['output']=lock(c['files']['root.json'],{k:v for k,v in c['files'].items() if k!='root.json'});out.append(c)
    c=copy.deepcopy(next(c for c in out if c['id']=='empty'));c['id']='fanout-small'
    c['files']['a.json']=encoded(dict(packages=[pkg('A')]))
    c['files']['b.json']=encoded(dict(packages=[pkg('B')]))
    c['files']['root.json']=encoded(dict(dependencies=[req('A'),req('B')]))
    c['args']=['resolve','root.json','4','b.json','a.json']
    c['output']=lock(c['files']['root.json'],{k:v for k,v in c['files'].items() if k!='root.json'});out.append(c)
    # Cap/one-over pairs retain their original sizes, never shrink on failure.
    for count in [64,65]:
        if count==64: add('names64',[],[pkg('p'+str(i)) for i in range(count)])
        else:
            c=error('names65',code='limit',detail='names',path=['catalog.json']);c['files']['catalog.json']=encoded(dict(packages=[pkg('p'+str(i)) for i in range(count)]));c['args'].append('catalog.json')
    for count in [128,129]:
        packages=[pkg('A',i) for i in range(1,count+1)]
        if count==128:add('versions128',[],packages)
        else:
            c=error('versions129',code='limit',detail='versions',path=['catalog.json']);c['files']['catalog.json']=encoded(dict(packages=packages));c['args'].append('catalog.json')
    for count in [256,257]:
        roots=[req('A')]*count
        if count==256:add('edges256',roots,[pkg('A')])
        else:
            c=error('edges257',code='limit',detail='edges');c['files']['root.json']=encoded(dict(dependencies=roots))
    for count in [32,33]:
        c=copy.deepcopy(out[0]);c['id']='catalogs'+str(count);c['files'].update({f'c{i:02}.json':'{"packages":[]}' for i in range(count)});c['args']+=list(sorted(k for k in c['files'] if k!='root.json'))
        if count==32:c['output']=lock(c['files']['root.json'],{k:v for k,v in c['files'].items() if k!='root.json'})
        else:c.update(exit=2,output=dict(error='limit',detail='catalogs',path=['$argv']))
        out.append(c)
    for count in [65536,65537]:
        c=copy.deepcopy(out[0]);c['id']='bytes'+str(count);c['files']['catalog.json']='{"packages":[]}'+' '*(count-15);assert len(c['files']['catalog.json'])==count;c['args'].append('catalog.json')
        if count==65536:c['output']=lock(c['files']['root.json'],{'catalog.json':c['files']['catalog.json']})
        else:c.update(exit=2,output=dict(error='limit',detail='file_bytes',path=['catalog.json']))
        out.append(c)
    for count in [524288,524289]:
        c=copy.deepcopy(out[0]);c['id']='total'+str(count)
        for i in range(8):c['files'][f'c{i}.json']='{"packages":[]}'+' '*(65536-15)
        if count==524289:c['files']['z.json']=' '
        cats={k:v for k,v in c['files'].items() if k!='root.json'};c['args']+=list(cats)
        if count==524288:c['output']=lock(c['files']['root.json'],cats)
        else:c.update(exit=2,output=dict(error='limit',detail='catalog_bytes',path=['z.json']))
        out.append(c)
    return out

if __name__=='__main__':
    print(json.dumps(cases(),indent=2))
