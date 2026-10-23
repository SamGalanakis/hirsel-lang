"""Go RPC 7,8 vectors and terminal cleanup with independent Unix peers.

No engine adapter. Peers block on explicit observer handshakes; production
Ordered placement is observed after the gateway's listening record.
"""
import argparse,json,os,resource,select,socket,struct,subprocess,tempfile,threading,shutil,signal,hashlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--hirsel',required=True);p.add_argument('--manifest',type=Path,required=True);a=p.parse_args();root=a.manifest.parent
# Keep Unix paths within the application's declared 100-byte cap.
work=Path(tempfile.mkdtemp(prefix='rg30-'));tapes=[];receipts=[]
env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')};env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off')
def compact(x):return json.dumps(x,separators=(',',':'))
def frame(x):
 b=compact(x).encode();return struct.pack('!I',len(b))+b
def exact(s,n):
 b=b''
 while len(b)<n:
  part=s.recv(n-len(b));assert part,('short',b,n);b+=part
 return b
def receive(s):
 n=struct.unpack('!I',exact(s,4))[0];assert 0<n<=2048;return json.loads(exact(s,n))
def limits():
 resource.setrlimit(resource.RLIMIT_CPU,(60,61));resource.setrlimit(resource.RLIMIT_DATA,(2*1024**3,2*1024**3))
def run(argv,d,name,setup=False):
 (d/(name+'.argv.json')).write_text(compact(list(map(str,argv))))
 o=subprocess.run(argv,capture_output=True,env=env,timeout=300 if setup else 60,preexec_fn=None if setup else limits)
 (d/(name+'.stdout')).write_bytes(o.stdout);(d/(name+'.stderr')).write_bytes(o.stderr);return o
class Peer:
 def __init__(self,path,mode):
  self.path=path;self.mode=mode;self.seen=[];self.errors=[];self.ready=threading.Event();self.release=threading.Event();self.closed=threading.Event()
  self.listener=socket.socket(socket.AF_UNIX);self.listener.settimeout(12);self.listener.bind(str(path));self.listener.listen(4)
  self.thread=threading.Thread(target=self.serve,daemon=True);self.thread.start()
 def serve(self):
  try:
   with self.listener.accept()[0] as stream:
    stream.settimeout(12);item=receive(stream);self.seen.append(item);self.ready.set()
    if self.mode=='cancel':assert stream.recv(1)==b'';self.closed.set();return
    assert self.release.wait(10)
    if self.mode=='unsafe':self.closed.set();return
    reply=dict(id=item['id'],**(dict(error=dict(code='divide_by_zero')) if self.mode=='error' else dict(result=15)))
    stream.sendall(frame(reply));assert stream.recv(1)==b'';self.closed.set()
  except BaseException as e:self.errors.append(repr(e));self.ready.set()
  finally:self.listener.close()
 def finish(self):
  self.release.set();self.thread.join(12);assert not self.thread.is_alive();assert not self.errors,self.errors;self.path.unlink(missing_ok=True)
binaries={}
for backend in ['llvm','cranelift']:
 binary=root/backend
 if os.environ.get('RPC_GATEWAY_30_BINARIES'):
  shutil.copyfile(Path(os.environ['RPC_GATEWAY_30_BINARIES'])/backend,binary);binary.chmod(0o755)
 else:
  o=run([a.hirsel,'build',str(a.manifest),'--backend',backend,'-o',str(binary)],root,'live-build-'+backend,True);assert o.returncode==0,o.stderr
 binaries[backend]=binary
for backend,binary in binaries.items():
 for width in [1,2,4]:
  for mode in os.environ.get('RPC_GATEWAY_30_LIVE_MODES','success,error,cancel').split(','):
   label=f'{backend}-{width}-{mode}';d=root/label;d.mkdir();sockets=work/label;sockets.mkdir();config=d/'config';config.mkdir()
   listener=sockets/'in';up=sockets/'up';control=sockets/'ctl';peer=Peer(up,mode)
   manifest=dict(jobs=4,clients=1,request_ms=3000,session_ms=15000,routes=[dict(method='Arith.Add',socket=str(up),idempotent=mode!='unsafe')])
   (config/'routes.json').write_text(compact(manifest))
   args=['serve','routes.json',str(listener)]
   grants=dict(args=args,io=dict(stdout='stdout.bytes@1',tasks='tasks.cooperative@1',random='random.splitmix64@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='config',host=str(config),access='read')],net=dict(provider='net.stream@1',rules=[dict(address=str(listener),service='0',operation='unix_listen'),dict(address=str(up),service='0',operation='unix_connect')])),placement=dict(cores=width,control=str(control),commit='shared',reorder='off'))
   grant=d/'grants.json';grant.write_text(compact(grants));tape=d/'run.tape';argv=[str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),*args];(d/'record.argv.json').write_text(compact(argv))
   wrapped=['python3',str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',*argv]
   (d/'wrapper.argv.json').write_text(compact(wrapped))
   child=subprocess.Popen(wrapped,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,start_new_session=True)
   try:
    assert select.select([child.stdout],[],[],12)[0],'no ready';ready=child.stdout.readline();assert ready==b'{"listening":true}\n',ready
    # Handshake keeps the app alive before any request is allowed to complete.
    with socket.socket(socket.AF_UNIX) as ctl:
     ctl.settimeout(5);ctl.connect(str(control));ctl.sendall(b'{"metrics":true}\n');receipt=b''
     while b'\n' not in receipt:receipt+=ctl.recv(65536)
    snapshot=json.loads(receipt);(d/'control.json').write_bytes(receipt)
    assert snapshot['worker_phase']=='running' and snapshot['width']['effective']==width and snapshot['width']['pending']==0,snapshot
    with socket.socket(socket.AF_UNIX) as down:
     down.settimeout(12);down.connect(str(listener));down.sendall(frame(dict(id='collision',method='Arith.Add',args=dict(a=7,b=8))))
     assert peer.ready.wait(10);assert len(peer.seen)==1 and peer.seen[0]['id']=='c0r0a1',peer.seen
     if mode=='cancel':down.close()
     else:
      peer.release.set();reply=receive(down);assert reply==dict(id='collision',**(dict(error=dict(code='unknown_outcome' if mode=='unsafe' else 'divide_by_zero')) if mode in ['error','unsafe'] else dict(result=15))),reply
    assert peer.closed.wait(10),'upstream not closed on disconnect'
    stdout,stderr=child.communicate(timeout=15);actual=dict(exit=child.returncode,stdout=(ready+stdout).decode())
    status='canceled' if mode=='cancel' else 'unknown_outcome' if mode=='unsafe' else 'application_error' if mode=='error' else 'ok'
    wanted=dict(exit=0,stdout='{"listening":true}\n'+compact(dict(client=0,request=0,attempt=1,status=status))+'\n{"clients":1,"expired":false}\n')
    (d/'wrapper.stderr').write_bytes(stderr)
    app_stderr=b'\n'.join(line for line in stderr.splitlines() if not line.startswith(b'capped:'))
    assert actual==wanted and app_stderr==b'',(actual,wanted,stderr)
    receipts.append(dict(id=label,passed=True,effective=width,phase=snapshot['worker_phase']));(root/'live-receipts.json').write_text(json.dumps(receipts,indent=2));(d/'actual.json').write_text(compact(actual))
   finally:
    if child.poll() is None:os.killpg(child.pid,signal.SIGKILL);child.communicate()
    peer.finish()
   assert not listener.exists(),'listener leaked';(config/'routes.json').unlink()
   for target,executable in binaries.items():
    replay=run([str(executable),'--hirsel-replay',str(tape)],d,'replay-'+target);assert (replay.returncode,replay.stdout,replay.stderr)==(0,wanted['stdout'].encode(),b''),(label,target,replay)
   tapes.append(dict(path=str(tape),expected=wanted))
print(json.dumps(dict(completed=len(receipts),tapes=tapes,receipts=receipts)))
