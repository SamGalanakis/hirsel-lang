import json,os,selectors,socket,subprocess,sys,tempfile,traceback
from pathlib import Path
from host import send,recv
cli, manifest, destination = sys.argv[1:]
base=Path(manifest).parent
root=Path(destination);root.mkdir()
products=os.environ['HIRSEL_PRODUCTS']
env={**os.environ,'HIRSEL_DAEMON':'off','HIRSEL_RESULTS':'none@1','HIRSEL_PRODUCTS':products,'HIRSEL_JOBS':'4','HIRSEL_MEMORY_LIMIT':'2G','HIRSEL_TIMING':'off'}
(base/'capped').chmod(0o755)
rows=[]
for mode in ['success','domain','cancel']:
 path=root/mode;path.mkdir();sock=Path(tempfile.mkdtemp(prefix='ps30-interp-'))/'relay.sock';tape=root/(mode+'.tape')
 grant=path/'grants.json'
 grant.write_text(json.dumps({'io':{'dirs':[],'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','monotonic_clock':'clock.monotonic.ns@1','net':{'provider':'net.stream@1','rules':[{'address':str(sock),'service':'0','operation':'unix_listen'}]}}}))
 argv=[str(base/'capped'),'--timeout','15','prlimit','--cpu=60:61',cli,'run',manifest,'--grants',str(grant),'--record',str(tape),'--max-steps','10000000','--','serve',str(sock),'64','5000' if mode!='cancel' else '500','4']
 (path/'argv.json').write_text(json.dumps(argv))
 child=subprocess.Popen(argv,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 peers=[]
 try:
  sel=selectors.DefaultSelector();sel.register(child.stdout,selectors.EVENT_READ);assert sel.select(10)
  first=child.stdout.readline();(path/'first').write_bytes(first);assert json.loads(first)=={'listening':True},first
  def connect():
   s=socket.socket(socket.AF_UNIX);s.settimeout(3);s.connect(str(sock));peers.append(s);return s
  if mode=='success':
   a,b,p=connect(),connect(),connect()
   for s in [a,b]:send(s,{'op':'subscribe','topic':'a'});assert recv(s)=={'op':'subscribed','topic':'a'}
   for seq in [1,2]:
    send(p,{'op':'publish','topic':'a','data':'x'});assert recv(p)=={'op':'queued','seq':seq,'subscribers':2}
    for s in [a,b]:assert recv(s)=={'op':'delivery','seq':seq,'topic':'a','data':'x'}
   send(b,{'op':'unsubscribe','topic':'a'});assert recv(b)['op']=='unsubscribed'
   send(p,{'op':'publish','topic':'a','data':'y'});assert recv(p)=={'op':'queued','seq':3,'subscribers':1};assert recv(a)['seq']==3
   send(p,{'op':'stop'});assert recv(p)=={'op':'stopping'}
  elif mode=='domain':
   bad=connect();bad.sendall(b'\0\0\0\1{');assert bad.recv(1)==b''
   p=connect();send(p,{'op':'stop'});assert recv(p)=={'op':'stopping'}
  else:p=connect();p.sendall(b'\0\0')
  out,err=child.communicate(timeout=12);out=first+out
  (path/'stdout').write_bytes(out);(path/'stderr').write_bytes(err);(path/'status').write_text(str(child.returncode))
  assert child.returncode==0,(child.returncode,out,err)
  final=json.loads(out.splitlines()[-1]);assert final['joined'] and final['credits_held']==0 and final['reserved']==final['returned']
  assert not sock.exists();grant.unlink()
  rows.append({'id':mode,'passed':True,'tape':str(tape),'stdout':out.decode(),'status':0})
 except Exception as error:
  rows.append({'id':mode,'passed':False,'error':repr(error),'traceback':traceback.format_exc()})
 finally:
  for s in peers:s.close()
  if child.poll() is None:child.kill()
  tail,diagnostic=child.communicate()
  if not (path/'stderr').exists():
   (path/'stdout-tail').write_bytes(tail);(path/'stderr').write_bytes(diagnostic);(path/'status').write_text(str(child.returncode))
  (root/'receipts.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows,indent=2))
sys.exit(0 if all(r['passed'] for r in rows) else 1)
