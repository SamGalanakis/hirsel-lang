"""Independent exhaustive-cut and feasible-flow oracle; no augmenting paths."""
import json,pathlib,sys

def capacities(m,s):
 overrides={o['arc']:o['capacity'] for o in s['overrides']}
 return [overrides.get(a['name'],a['capacity']) for a in m['arcs']]

def minimum(m,c):
 v=m['vertices']; source=v.index(m['source']);sink=v.index(m['sink'])
 return min(sum(c[i] for i,a in enumerate(m['arcs']) if mask>>v.index(a['source'])&1 and not mask>>v.index(a['target'])&1) for mask in range(1<<len(v)) if mask>>source&1 and not mask>>sink&1)

def validate(m,s,w):
 c=capacities(m,s);assert w['capacities']==c and w['name']==s['name']
 assert len(w['flows'])==len(c);v=m['vertices'];balance=[0]*len(v)
 for i,a in enumerate(m['arcs']):
  f=w['flows'][i];assert type(f) is int and 0<=f<=c[i]
  balance[v.index(a['source'])]-=f;balance[v.index(a['target'])]+=f
 value=w['value'];assert type(value) is int and value>=0
 assert balance==[-value if x==m['source'] else value if x==m['sink'] else 0 for x in v]
 reached={m['source']}
 while True:
  before=set(reached)
  for i,a in enumerate(m['arcs']):
   if a['source'] in reached and w['flows'][i]<c[i]:reached.add(a['target'])
   if a['target'] in reached and w['flows'][i]>0:reached.add(a['source'])
  if before==reached:break
 assert m['sink'] not in reached and w['reachable']==[x in reached for x in v]
 cut=[i for i,a in enumerate(m['arcs']) if a['source'] in reached and a['target'] not in reached]
 assert w['cut_arcs']==cut and w['cut_capacity']==sum(c[i] for i in cut)==value==minimum(m,c)

if __name__=='__main__':
 root=pathlib.Path(__file__).parent;count=0
 for file in ['cases.json','verify-cases.json','logical-jobs-cases.json']:
  for row in json.loads((root/file).read_text()):
   if 'error' in row:continue
   m=json.loads(row['input'])
   assert row['optima']==[minimum(m,capacities(m,s)) for s in m['scenarios']]
   if row.get('command')=='verify':
    for s,w in zip(m['scenarios'],json.loads(row['report'])['results']):validate(m,s,w)
   count+=1
 print(json.dumps({'checked':count,'algorithm':'exhaustive cuts; no augmenting paths'}))
