"""Real Unix/files, production Ordered grants, immutable own-tape replays."""
import argparse, json, os, selectors, socket, struct, subprocess, tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--hirsel',required=True);p.add_argument('--root',type=Path,required=True);p.add_argument('--capped',required=True);a=p.parse_args()
env=dict(os.environ,HIRSEL_MEMORY_LIMIT='2G',HIRSEL_JOBS='4')
def read_exact(s,n):
 b=b''
 while len(b)<n:
  part=s.recv(n-len(b));assert part,'unexpected EOF';b+=part
 return b
def call(s,c):
 data=json.dumps(c,separators=(',',':')).encode();s.sendall(struct.pack('>I',len(data))+data)
 return json.loads(read_exact(s,struct.unpack('>I',read_exact(s,4))[0]))
def line(child):
 ready=selectors.DefaultSelector();ready.register(child.stdout,selectors.EVENT_READ)
 assert ready.select(10),'no application-ready handshake'
 return child.stdout.readline()
with tempfile.TemporaryDirectory(prefix='wq31-') as transport:
 transport=Path(transport);data=a.root/'host-data';data.mkdir();socketpath=transport/'q.sock';control=transport/'control'
 for backend in ['llvm','cranelift']:
  binary=a.root/backend
  for width in [1,2,4]:
   for workflow in ['success','domain','cancel']:
    cell=f'{backend}-w{width}-{workflow}';receipt=a.root/cell;receipt.mkdir()
    for f in data.iterdir():f.unlink()
    grants={'io':{'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','monotonic_clock':'clock.monotonic.ns@1','dirs':[{'path':'data','host':str(data),'access':'read_write'}],'net':{'provider':'net.stream@1','rules':[{'address':str(socketpath),'service':'0','operation':'unix_listen'}]}},'placement':{'cores':width,'control':str(control),'commit':'shared','reorder':'auto'}}
    grant=receipt/'grants.json';grant.write_text(json.dumps(grants));tape=receipt/'original.tape'
    argv=['python3',a.capped,'--timeout','60','prlimit','--cpu=60:61',str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),'serve',str(socketpath),'1000','100']
    (receipt/'argv.json').write_text(json.dumps(argv))
    child=subprocess.Popen(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
    try:
     prefix=line(child);assert prefix==b'{"ready":true}\n',prefix
     with socket.socket(socket.AF_UNIX) as ctl:
      ctl.settimeout(5);ctl.connect(str(control));ctl.sendall(b'{"metrics":true}\n');reply=b''
      while not reply.endswith(b'\n'):reply+=ctl.recv(8192)
     (receipt/'control.json').write_bytes(reply);metrics=json.loads(reply)
     assert metrics['worker_phase']=='running' and metrics['width']['effective']==width and metrics['width']['pending']==0,metrics
     with socket.socket(socket.AF_UNIX) as client:
      client.settimeout(5);client.connect(str(socketpath))
      if workflow=='cancel':client.sendall(b'\x00')
      else:
       assert call(client,{'op':'group','group':0})=={'group':0}
       assert call(client,{'op':'enqueue','data':'a'})=={'enqueued':1}
       assert call(client,{'op':'claim','group':0,'consumer':0})=={'id':1,'group':0,'consumer':0,'token':1,'data':'a'}
       if workflow=='domain':assert call(client,{'op':'ack','group':0,'id':1,'token':2})=={'error':'stale_token'}
       else:
        assert call(client,{'op':'ack','group':0,'id':1,'token':1})=={'acked':1}
        assert call(client,{'op':'checkpoint'})=={'checkpointed':True}
       assert call(client,{'op':'stop'})=={'stopped':True}
      suffix,err=child.communicate(timeout=10)
    finally:
     if child.poll() is None:child.kill();child.wait()
    output=prefix+suffix;(receipt/'stdout').write_bytes(output);(receipt/'stderr').write_bytes(err)
    assert child.returncode==0 and output==b'{"ready":true}\n{"joined":true}\n',(cell,child.returncode,output,err)
    assert not socketpath.exists(),'listener leak'
    original=tape.read_bytes()
    for f in data.iterdir():f.unlink()
    data.rmdir();grant.unlink()
    for target in ['llvm','cranelift','reference']:
     command=[str(a.root/target),'--hirsel-replay',str(tape)] if target!='reference' else [a.hirsel,'run',str(a.root/'hirsel.json'),'--replay',str(tape),'--max-steps','10000000']
     replay=subprocess.run(['python3',a.capped,'--timeout','60','prlimit','--cpu=60:61',*command],capture_output=True,env=env)
     (receipt/f'replay-{target}.stdout').write_bytes(replay.stdout);(receipt/f'replay-{target}.stderr').write_bytes(replay.stderr)
     assert replay.returncode==0 and replay.stdout==output,(cell,target,replay.returncode,replay.stderr)
    assert not data.exists() and tape.read_bytes()==original
    data.mkdir();print('PASS',cell,flush=True)
