"""Independent exhaustive CART oracle; Python Fraction, no production imports.
scikit-learn 59106029, _criterion.pyx:605-688, _splitter.pyx:427-485;
test_tree.py:158-161,249-259 (BSD-3-Clause). Integer inclusive thresholds,
ascending feature/threshold ties, label IDs 0/1 replace upstream -1/+1.
"""
import copy
import json
from fractions import Fraction
from pathlib import Path


def train(job):
    rows, k = job['rows'], job['labels']
    nodes, audit = [None], []
    def visit(indices, depth, slot):
        counts = [sum(rows[i]['label'] == c for i in indices) for c in range(k)]
        leaf = counts.index(max(counts))
        nodes[slot] = dict(counts=counts, leaf=leaf)
        parent = 1 - sum(Fraction(c, len(indices)) ** 2 for c in counts)
        best = None
        if depth < job['max_depth']:
            for f in range(job['features']):
                for t in sorted({rows[i]['x'][f] for i in indices})[:-1]:
                    left = [i for i in indices if rows[i]['x'][f] <= t]
                    right = [i for i in indices if rows[i]['x'][f] > t]
                    if min(len(left), len(right)) < job['min_leaf']:
                        continue
                    impurity = sum(Fraction(len(part), len(indices)) *
                        (1 - sum(Fraction(sum(rows[i]['label'] == c for i in part), len(part)) ** 2
                                 for c in range(k))) for part in [left, right])
                    gain = parent - impurity
                    if gain > 0 and (best is None or gain > best[0]):
                        best = gain, f, t, left, right
        audit.append(dict(node=slot, indices=indices, counts=counts,
                          gain=str(best[0]) if best else '0'))
        if best:
            _, f, t, left, right = best
            a = len(nodes)
            nodes.extend([None, None])
            nodes[slot] = dict(counts=counts, feature=f, threshold=t, left=a, right=a+1)
            visit(left, depth+1, a)
            visit(right, depth+1, a+1)
    visit(list(range(len(rows))), 0, 0)
    model = {key: job[key] for key in ['features', 'labels', 'max_depth', 'min_leaf']}
    model.update(version=1, nodes=nodes)
    return model, audit


def predict(model, queries):
    out = []
    for x in queries:
        i, path = 0, []
        while 'leaf' not in model['nodes'][i]:
            n = model['nodes'][i]
            left = x[n['feature']] <= n['threshold']
            path.append(dict(node=i, feature=n['feature'], value=x[n['feature']],
                             threshold=n['threshold'], left=left))
            i = n['left'] if left else n['right']
        n = model['nodes'][i]
        out.append(dict(label=n['leaf'], counts=n['counts'], path=path, leaf=i))
    return out


def freeze():
    def job(xs, ys, queries=None, **kw):
        return dict(features=len(xs[0]), labels=max(ys)+1, max_depth=4, min_leaf=1,
                    rows=[dict(x=x, label=y) for x,y in zip(xs,ys)], queries=queries or xs, **kw)
    toy = job([[-2,-1],[-1,-1],[-1,-2],[1,1],[1,2],[2,1]], [0,0,0,1,1,1],
              [[-1,-1],[2,2],[3,2],[0,0]])
    seeds = [('toy', toy), ('constant', job([[1],[1],[1],[1]],[0,1,1,0])),
             ('duplicates', job([[0],[0],[1],[1]],[0,1,1,1])),
             ('tie-features', job([[0,0],[1,1],[2,2],[3,3]],[0,1,1,0])),
             ('four-labels', job([[i] for i in range(8)],[0,0,1,1,2,2,3,3])),
             ('feature-bounds', job([[-10000],[10000]],[0,1],[[-10000],[0],[10000]])),
             ('depth-zero', dict(toy,max_depth=0)), ('min-leaf-block',dict(toy,min_leaf=4)),
             ('min-leaf-eight',dict(toy,min_leaf=8)),
             ('rows-64', job([[i%8,i%4,i%2,i] for i in range(64)], [i%4 for i in range(64)], [[0,0,0,0]])),
             ('queries-16', dict(toy,queries=[[i,i] for i in range(16)]))]
    cases=[]
    for name,j in seeds:
        model,audit=train(j)
        cases.append(dict(id=name,command='train',input=json.dumps(dict(version=1,jobs=[j]),separators=(',',':')),
                          report=dict(version=1,jobs=[dict(model=model,predictions=predict(model,j['queries']))]),
                          audit=audit,exit=0,stderr=''))
    model,_=train(toy)
    cases.append(dict(id='predict-loaded',command='predict',input=json.dumps(dict(version=1,jobs=[dict(model=model,queries=toy['queries'])])),report=dict(version=1,jobs=[dict(model=model,predictions=predict(model,toy['queries']))]),exit=0,stderr=''))
    cases.append(dict(id='batch-four',command='train',input=json.dumps(dict(version=1,jobs=[toy]*4)),report=dict(version=1,jobs=[cases[0]['report']['jobs'][0]]*4),exit=0,stderr=''))
    def bad(name,doc,code,command='train'):
        cases.append(dict(id=name,command=command,input=json.dumps(doc,separators=(',',':')),exit=2,stderr='treelearn: '+code+'\n'))
    for name,key,value,code in [('rows-65','rows',toy['rows']*10+toy['rows'][:5],'rows'),
            ('features-5','features',5,'features'),('features-0','features',0,'features'),
            ('labels-5','labels',5,'labels'),('labels-0','labels',0,'labels'),
            ('depth-5','max_depth',5,'depth'),('min-leaf-0','min_leaf',0,'min_leaf'),
            ('min-leaf-9','min_leaf',9,'min_leaf'),('queries-17','queries',[[0,0]]*17,'queries'),
            ('empty-rows','rows',[],'rows')]:
        bad(name,dict(version=1,jobs=[dict(toy,**{key:value})]),code)
    for val in [-10001,10001,1.5,True]:
        j=copy.deepcopy(toy);j['rows'][0]['x'][0]=val
        bad('numeric-'+str(val),dict(version=1,jobs=[j]),'numeric')
    j=copy.deepcopy(toy);j['rows'][0]['label']=2
    bad('label-out-of-range',dict(version=1,jobs=[j]),'labels')
    bad('jobs-five',dict(version=1,jobs=[toy]*5),'jobs')
    bad('version',dict(version=2,jobs=[toy]),'version')
    for name,change in [('cycle',lambda m:m['nodes'][0].update(left=0)),
                         ('bad-child',lambda m:m['nodes'][0].update(right=99)),
                         ('bad-feature',lambda m:m['nodes'][0].update(feature=2)),
                         ('bad-count',lambda m:m['nodes'][1]['counts'].__setitem__(0,2)),
                         ('bad-leaf-tie',lambda m:m['nodes'][1].update(leaf=1)),
                         ('nodes-32',lambda m:m['nodes'].extend([m['nodes'][1]]*29))]:
        m=copy.deepcopy(model);change(m)
        bad(name,dict(version=1,jobs=[dict(model=m,queries=[[0,0]])]),'model' if name!='nodes-32' else 'nodes','predict')
    # Input byte cap identities are frozen raw bytes including legal JSON whitespace.
    base=cases[0]['input']
    for size in [65536,65537]:
        cases.append(dict(cases[0],id='bytes-'+str(size),input=base+' '*(size-len(base)),
                          **({} if size==65536 else dict(exit=2,stderr='treelearn: input_limit\n',report=None))))
    cases.append(dict(id='pending-cancel',command='train',input=cases[0]['input'],pending=True,exit=2,stderr='treelearn: canceled\n'))
    cases.append(dict(id='denial',command='train',input=cases[0]['input'],denial=True,exit=2,stderr='treelearn: grant\n'))
    for c in cases:
        c['stdout']='published\n' if c['exit']==0 else ''
        c['output']=json.dumps(c['report'],separators=(',',':'))+'\n' if c.get('report') else 'old\n'
    root=Path(__file__).parent
    (root/'cases.json').write_text(json.dumps(dict(cases=cases),indent=2)+'\n')
    Path('examples/treelearn/fixtures/toy.json').write_text(cases[0]['input']+'\n')

if __name__=='__main__':
    import sys
    if '--check' in sys.argv:
        cases=json.loads((Path(__file__).parent/'cases.json').read_text())['cases']
        for c in cases:
            if c.get('report') and c['exit']==0:
                inp=json.loads(c['input'])
                for j,result in zip(inp['jobs'],c['report']['jobs']):
                    model,audit=train(j) if c['command']=='train' else (j['model'],None)
                    assert model==result['model'],c['id']
                    assert predict(model,j['queries'])==result['predictions'],c['id']
                    if audit and 'audit' in c: assert audit==c['audit'],c['id']
        print(f'{len(cases)} frozen independent vectors')
    else:
        freeze()
