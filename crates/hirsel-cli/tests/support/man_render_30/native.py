"""Ordinary production Ordered launches. Width evidence is explicit, never inferred."""
import argparse, json, os, pathlib, socket, subprocess, threading
from vectors import cases
p=argparse.ArgumentParser();p.add_argument('--hirsel');p.add_argument('--backend');p.add_argument('--root');a=p.parse_args()
root=pathlib.Path(a.root); app=root/a.backend
repo=pathlib.Path(__file__).resolve() # capped is supplied from the repository below
capped=root/'capped'
serial=0
def run(args):
 global serial
 serial+=1
 out=root/f'command-{serial}.stdout';err=root/f'command-{serial}.stderr'
 wrapper="import subprocess,sys; r=subprocess.run(sys.argv[3:],stdout=open(sys.argv[1],'wb'),stderr=open(sys.argv[2],'wb'));sys.exit(r.returncode)"
 r=subprocess.run(['python3',str(capped),'--timeout','60','prlimit','--cpu=60:61','python3','-c',wrapper,str(out),str(err),*map(str,args)],env={**os.environ,'HIRSEL_JOBS':'4','HIRSEL_MEMORY_LIMIT':'2G'},capture_output=True)
 (root/f'command-{serial}.cap').write_bytes(r.stderr)
 return subprocess.CompletedProcess(args,r.returncode,out.read_bytes() if out.exists() else b'',err.read_bytes() if err.exists() else r.stderr)
build=subprocess.run([a.hirsel,'build',str(root/'hirsel.json'),'--backend',a.backend,'-o',str(app)],capture_output=True)
(root/'build.log').write_bytes(build.stdout+build.stderr)
assert build.returncode==0,build.stderr.decode()
selected=[r for r in cases if 'pending' not in r and not r.get('commit_fault')]
for r in cases:
 if 'pending' in r or r.get('commit_fault'):print(json.dumps({'id':r['id'],'engine':a.backend,'disposition':'INCOMPLETE','reason':'production root-Io file script/fault provider unavailable'}),flush=True)
if os.getenv('MAN_RENDER_30_CASE'):selected=[r for r in selected if r['id'] in os.environ['MAN_RENDER_30_CASE'].split(',')]
assert selected
counts={'selected':sum(3 if r['id'] in ['cmark-nowrap','html-block','batch-no-publish','write-refused'] else 1 for r in selected),'completed':0,'passed':0,'width_qualified':0}
for row in selected:
 for width in ([1,2,4] if row['id'] in ['cmark-nowrap','html-block','batch-no-publish','write-refused'] else [1]):
  id=f"{row['id']}-{width}";data=root/('data-'+id);data.mkdir()
  for name,value in row['files'].items():
   path=data/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(value.encode() if isinstance(value,str) else bytes(value))
  ctl=root/('ctl-'+id);tape=root/(id+'.tape');grant=root/(id+'.json')
  grant.write_text(json.dumps({'args':row['args'],**({'inputs':{'hash_seed':0,'random_seed':0,'test_seed':0,'scheduler':{'algorithm':'random','version':1,'seed':row['scheduler_seed']}}} if 'scheduler_seed' in row else {}),'io':{'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','dirs':[{'path':'data','host':str(data),'access':'read' if row.get('read_only') else 'read_write'}]},'placement':{'cores':width,'commit':'shared','reorder':'off','control':str(ctl)}}))
  samples=[];stop=threading.Event()
  def observe():
   while not stop.is_set():
    try:
     with socket.socket(socket.AF_UNIX) as s:
      s.settimeout(.2);s.connect(str(ctl));s.sendall(b'{"metrics":true}\n');reply=b''
      while not reply.endswith(b'\n'):
       part=s.recv(65536)
       if not part:break
       reply+=part
      if reply:samples.append(json.loads(reply))
    except (OSError,ValueError):pass
  thread=threading.Thread(target=observe);thread.start()
  r=run([app,'--hirsel-record',tape,'--hirsel-grants',grant,*row['args']]);stop.set();thread.join()
  (root/(id+'.control.json')).write_text(json.dumps(samples))
  (root/(id+'.run.json')).write_text(json.dumps({'args':row['args'],'exit':r.returncode,'stdout':r.stdout.decode(errors='replace'),'stderr':r.stderr.decode(errors='replace')}))
  assert r.returncode==row['exit'],(id,r.returncode,r.stdout,r.stderr)
  assert r.stdout==row['stdout'].encode(),(id,r.stdout,row['stdout'])
  assert not r.stderr,(id,r.stderr)
  for name,value in row['outputs'].items():assert (data/name).read_bytes()==value.encode(),(id,name)
  expected=set(row['files'])|set(row['outputs']);assert {str(p.relative_to(data)) for p in data.rglob('*') if p.is_file()}==expected,id
  original=tape.read_bytes()
  # Remove all original provider state; replay must neither need nor recreate it.
  import shutil
  shutil.rmtree(data);grant.unlink()
  rr=run([app,'--hirsel-replay',tape]);assert (rr.returncode,rr.stdout,rr.stderr)==(r.returncode,r.stdout,r.stderr),(id,rr)
  assert not data.exists() and tape.read_bytes()==original
  qualified=any(s.get('worker_phase')=='running' and s.get('width',{}).get('effective')==width and s.get('width',{}).get('pending')==0 for s in samples)
  counts['completed']+=1;counts['passed']+=1;counts['width_qualified']+=qualified
  print(json.dumps({'id':id,'engine':a.backend,'pass':True,'width':width,'qualified':qualified}),flush=True)
print(json.dumps(counts),flush=True)
