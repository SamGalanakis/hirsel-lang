"""One seeded, interleaved ten-round session for tasks/list and print."""
import hashlib,json,os,sys,time
from pathlib import Path
from types import SimpleNamespace
root=Path(__file__).resolve().parents[4]
sys.path[:0]=[str(root/'bench/apps/tasks'),str(root/'bench/micro')]
from measure import timing_lock,schedule,sample as micro_sample,bootstrap_ratio,ratio
namespace={'__name__':'text_build_tasks','__file__':str(root/'bench/apps/tasks/bench')}
exec(compile(Path(namespace['__file__']).read_text(),namespace['__file__'],'exec'),namespace)
old=Path(sys.argv[1]).resolve();new=Path(sys.argv[2]).resolve()
out=Path(sys.argv[3]).resolve();out.mkdir(exist_ok=True)
seed=20261021
samples=[]
participants={
 'print/rust':{'name':'rust','command':[str(new/'rust'),'print','1000000']},
 'print/before':{'name':'before','command':[str(old/'print'),'--hirsel-grants',str(old/'grants.json'),'1000000']},
 'print/after':{'name':'after','command':[str(new/'print'),'--hirsel-grants',str(new/'grants.json'),'1000000']},
}
for p in participants.values():p['binary_sha256']=hashlib.sha256(Path(p['command'][0]).read_bytes()).hexdigest()
document={'session':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'seed':seed,'rounds':10,'samples':samples,'participants':participants,'status':'running'}
document['task_binaries']={name:hashlib.sha256(path.read_bytes()).hexdigest() for name,path in [('before',old/'tasks'),('after',new/'tasks'),('rust',namespace['RUST'])]}
os.sched_setaffinity(0,{7,31})
with timing_lock(31):
 for r,key in schedule(['list/rust','list/before','list/after',*participants],10,seed):
  print(r+1,key,flush=True)
  workload,name=key.split('/')
  if workload=='print':row=micro_sample(participants[key],31,300)
  else:
   binary=old/'tasks' if name=='before' else new/'tasks'
   row=namespace['sample']('rust-one-worker' if name=='rust' else 'hirsel','list',16,SimpleNamespace(binary=binary,seconds=1),out/f'{r}-{name}',r)
   row['counts']={k:v/row['completed'] for k,v in row['perf'].items()}
  row.update(workload=workload,name=name,round=r)
  samples.append(row)
  (out/'session.json').write_text(json.dumps(document,indent=2)+'\n')
 document['comparisons']=[]
 for workload in ['list','print']:
  rows={name:sorted([s for s in samples if s['workload']==workload and s['name']==name],key=lambda s:s['round']) for name in ['rust','before','after']}
  if workload=='print':assert len({s['stdout_sha256'] for v in rows.values() for s in v})==1
  for name in ['before','after']:
   result={'workload':workload,'name':name}
   for event in ['instructions','instructions:u','cycles','cycles:u','task-clock']:
    left=[s['counts'][event] for s in rows[name]];right=[s['counts'][event] for s in rows['rust']]
    result[event]={'ratio':ratio(left,right),'ci95':bootstrap_ratio(left,right,seed)}
   document['comparisons'].append(result)
 document['status']='ok'
 (out/'session.json').write_text(json.dumps(document,indent=2)+'\n')
 print(json.dumps(document['comparisons'],indent=2))
