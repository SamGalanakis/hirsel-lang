"""Frozen bounded matching fixtures. Exhaustive oracle shares no layering code.
NetworkX 9d866a5f bipartite/tests/test_matching.py:17-35, BSD-3-Clause.
"""
import copy, itertools, json, pathlib, random
ROOT=pathlib.Path(__file__).parent

def manifest(r,j,edges,disabled=()):
 return dict(resources=[dict(id=f'r{i}') for i in range(r)],jobs=[dict(id=f'j{i}') for i in range(j)],allowed=[dict(resource=a,job=b) for a,b in edges],disabled=list(disabled))
def exhaustive(m):
 n=len(m['jobs']); edges=m['allowed']; disabled=m['disabled']
 def visit(j,used):
  if j==n:return 0
  return max([visit(j+1,used)]+[1+visit(j+1,used|{e['resource']}) for e in edges if e['job']==j and e['resource'] not in used and e['resource'] not in disabled])
 return visit(0,set())
def batch(ms):return dict(version=1,revision=7,manifests=ms)
rows=[]
def add(id,ms,error=None,**extra):
 b=ms if isinstance(ms,dict) else batch(ms)
 row=dict(id=id,input=json.dumps(b,separators=(',',':')),**extra)
 if error:row['error']=error
 elif all(len(m['resources'])<=7 and len(m['jobs'])<=7 for m in b['manifests']):row['optima']=[exhaustive(m) for m in b['manifests']]
 rows.append(row)
empty=manifest(0,0,[])
chain=manifest(2,2,[(0,0),(1,0),(0,1)])
nx=manifest(6,6,[(b-6,a) for a,b in [(0,7),(0,8),(2,6),(2,9),(3,8),(4,8),(4,9),(5,11)]])
add('networkx', [nx]);add('networkx-simple',[manifest(3,2,list(itertools.product(range(3),range(2))))])
add('chain',[chain]);add('ties',[manifest(2,2,[(0,0),(1,0),(0,1),(1,1)])],golden=[[0,1]])
add('empty',[empty]);add('batch-empty',[])
add('disconnected',[manifest(5,7,[(0,0),(1,1),(2,3),(3,3)])])
add('unbalanced',[manifest(1,3,[(0,0),(0,1),(0,2)])])
add('disabled',[manifest(2,2,[(0,0),(1,0),(0,1)],[0])])
add('batch',[chain,nx,empty,manifest(2,3,[(1,0)])])
rng=random.Random(34034)
for i in range(12):
 r=i%7+1;j=(i*3)%7+1; edges=[(a,b) for b in range(j) for a in range(r) if rng.randrange(4)==0]
 add(f'oracle-{i}',[manifest(r,j,edges)])
for key,limit,err in [('resources',32,'resources'),('jobs',32,'job_count'),('allowed',128,'pairs'),('disabled',32,'disabled')]:
 m=manifest(32,32,[(a,b) for b in range(4) for a in range(32)],range(32))
 if key=='resources':m=manifest(32,0,[])
 if key=='jobs':m=manifest(0,32,[])
 if key=='disabled':m=manifest(32,0,[],range(32))
 add(key+'-cap',[m],cardinality=[0])
 over=copy.deepcopy(m)
 over[key].append(dict(id='over') if key in ['resources','jobs'] else dict(resource=0,job=4) if key=='allowed' else 0)
 add(key+'-over',[over],err)
add('manifests-cap',[empty]*4);add('manifests-over',[empty]*5,'manifests')
m=manifest(1,1,[(0,0)]);m['resources'][0]['id']='é'*32;add('identifier-cap',[m])
m=copy.deepcopy(m);m['resources'][0]['id']+='x';add('identifier-over',[m],'identifier')
for id,key in [('duplicate-resource','resources'),('duplicate-job','jobs')]:
 m=copy.deepcopy(chain);m[key][1]['id']=m[key][0]['id'];add(id,[m],'duplicate')
m=copy.deepcopy(chain);m['allowed'].append(m['allowed'][0]);add('duplicate-edge',[m],'duplicate')
m=copy.deepcopy(chain);m['disabled']=[0,0];add('duplicate-disabled',[m],'duplicate')
m=copy.deepcopy(chain);m['allowed'][0]['resource']=2;add('endpoint',[m],'endpoint')
m=copy.deepcopy(chain);m['disabled']=[2];add('disabled-endpoint',[m],'endpoint')
m=copy.deepcopy(chain);m['jobs'][0]['id']='';add('empty-identifier',[m],'identifier')
b=batch([chain]);b['version']=2;add('version',b,'version')
b=batch([chain]);b['manifests'].append(m);add('invalid-reload',b,'identifier')
for id,text,error in [('json','{','json'),('shape','{}','shape')]:rows.append(dict(id=id,input=text,error=error))
text=json.dumps(batch([empty]),separators=(',',':'))
# Keep the original 32 KiB admission boundary, but require decoded JSON work:
# bulk whitespace scanning made the former all-space padding only 22,632 TIR
# steps. 8,192 escaped newlines keep more than twice the 1M-step allowance in
# both reference modes; the unknown field changes no assignment observation.
cap_text=text[:-1]+',"padding":'+json.dumps('\n'*8192)+'}'
assert len(cap_text)<32768
rows.append(dict(id='input-cap',input=cap_text+' '*(32768-len(cap_text)),optima=[0]))
rows.append(dict(id='input-over',input=text+' '*(32769-len(text)),error='input_limit'))
add('denial',[chain],'denied',access='read');add('pending-cancel',[chain],'canceled',pending=True)
add('jobs-zero',[chain],'jobs',cli_jobs=0);add('cli-jobs-over',[chain],'jobs',cli_jobs=5)
# Additional public-provider fault workflows; original manifest bytes unchanged.
for name,op,error,requested,partial in [
 ('open','file.open','file_open',None,0),
 ('create','file.create_atomic','file_create',None,0),
 ('write','file.write_all','file_write',257,13),
 ('commit','file.commit','file_commit',None,0)]:
 add('fault-'+name,[chain],error,file_faults={op:[dict(requested=requested,error=dict(Fault=77),partial=partial)]},fault_operation=op,expected_write=[123, 34, 118, 101, 114, 115, 105, 111, 110, 34, 58, 49, 44, 34, 114, 101, 118, 105, 115, 105, 111, 110, 34, 58, 55, 44, 34, 114, 101, 115, 117, 108, 116, 115, 34, 58, 91, 123, 34, 99, 97, 114, 100, 105, 110, 97, 108, 105, 116, 121, 34, 58, 50, 44, 34, 112, 97, 105, 114, 115, 34, 58, 91, 123, 34, 114, 101, 115, 111, 117, 114, 99, 101, 34, 58, 49, 44, 34, 106, 111, 98, 34, 58, 48, 125, 44, 123, 34, 114, 101, 115, 111, 117, 114, 99, 101, 34, 58, 48, 44, 34, 106, 111, 98, 34, 58, 49, 125, 93, 44, 34, 106, 111, 98, 95, 116, 111, 95, 114, 101, 115, 111, 117, 114, 99, 101, 34, 58, 91, 49, 44, 48, 93, 44, 34, 114, 101, 115, 111, 117, 114, 99, 101, 95, 116, 111, 95, 106, 111, 98, 34, 58, 91, 49, 44, 48, 93, 44, 34, 117, 110, 109, 97, 116, 99, 104, 101, 100, 34, 58, 91, 93, 44, 34, 114, 101, 97, 99, 104, 101, 100, 95, 106, 111, 98, 115, 34, 58, 91, 93, 44, 34, 114, 101, 97, 99, 104, 101, 100, 95, 114, 101, 115, 111, 117, 114, 99, 101, 115, 34, 58, 91, 93, 44, 34, 99, 111, 118, 101, 114, 95, 106, 111, 98, 115, 34, 58, 91, 48, 44, 49, 93, 44, 34, 99, 111, 118, 101, 114, 95, 114, 101, 115, 111, 117, 114, 99, 101, 115, 34, 58, 91, 93, 125, 93, 125, 10])
for width in [1,2]:add(f'jobs-{width}',[chain,nx,empty,manifest(2,3,[(1,0)])],cli_jobs=width)
(ROOT/'cases.json').write_text(json.dumps(rows,ensure_ascii=False,indent=2)+'\n')
print(len(rows),'frozen workflows')
