"""One production cell; launched under tools/capped/prlimit by the lane."""
import argparse, ctypes, hashlib, json, os, selectors, shutil, signal, socket, struct, subprocess
from pathlib import Path

p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,required=True);p.add_argument('--root',type=Path,required=True);p.add_argument('--width',type=int,required=True);p.add_argument('--mode',choices=['success','domain','cancel'],required=True);a=p.parse_args()
root=a.root;root.mkdir();world=Path(os.environ.get('OBJECT_STORE_30_QUAL_WORLD',str(root/'world')))
for n in ['source','objects','roots']:(world/n).mkdir(parents=True)
def frame(kind,b):return kind.encode()+b' '+str(len(b)).encode()+b'\0'+b
def oid(kind,b):return hashlib.sha256(frame(kind,b)).hexdigest()
rows=[];data=bytes(range(128))
for i in range(32):
 name=f'f{i:02}';(world/'source'/name).write_bytes(data)
 rows.append(dict(path=name,size=len(data),id=oid('blob',data)))
if a.mode=='domain':rows[-1]['id']=oid('blob',b'y'*128)
(world/'source/manifest').write_text(json.dumps(rows,separators=(',',':')))
body=json.dumps([dict(type='blob',name=row['path'],id=oid('blob',data)) for row in rows],separators=(',',':')).encode();identity=oid('tree',body)
expected_out=(identity+'\n').encode() if a.mode=='success' else b''
expected_error=b'' if a.mode=='success' else b'{"kind":"input_changed","path":"f31"}\n' if a.mode=='domain' else b'{"kind":"canceled","path":""}\n'
ctl=root/'ctl';grant=root/'grants.json';tape=root/'run.tape'
grant.write_text(json.dumps(dict(args=['snapshot','manifest','head','4'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',signals=dict(provider='signals.host@1',kinds=['interrupt']),dirs=[dict(path=n,host=str(world/n),access='read' if n=='source' else 'read_write') for n in ['objects','roots','source']]),placement=dict(cores=a.width,control=str(ctl),commit='shared',reorder='off'))))
argv=[str(a.binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),'snapshot','manifest','head','4']
seal={'argv':argv,'binary_sha256':hashlib.sha256(a.binary.read_bytes()).hexdigest(),'grants_sha256':hashlib.sha256(grant.read_bytes()).hexdigest(),'files':{str(p.relative_to(world)):hashlib.sha256(p.read_bytes()).hexdigest() for p in world.rglob('*') if p.is_file()}}
(root/'seal-before.json').write_text(json.dumps(seal,indent=2)+'\n')
# The inotify queue is installed before the producer starts, avoiding a lost
# startup event. Blocking select is bounded by the enclosing CPU/wall cap.
libc=ctypes.CDLL(None,use_errno=True);fd=libc.inotify_init1(os.O_NONBLOCK|os.O_CLOEXEC);assert fd>=0
assert libc.inotify_add_watch(fd,os.fsencode(root),0x100|0x2|0x8)>=0
sel=selectors.DefaultSelector();sel.register(fd,selectors.EVENT_READ)
environment={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
environment.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G')
proc=subprocess.Popen(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment)
pidfd=os.pidfd_open(proc.pid);sel.register(pidfd,selectors.EVENT_READ)
snapshots=[];settled=False;sent=False
# Tape envelope is the shipped format, inspected only for confirmed Signals
# projection tag22. No event is rewritten and no timing sleep is involved.
def signals_ready():
 if not tape.exists():return False
 b=tape.read_bytes();at=len(b'hirsel-io-tape 8\0')
 while at+17<=len(b):
  length=struct.unpack_from('<I',b,at)[0];end=at+17+length
  if end>len(b):return False
  if b[at+16]==3 and length>=4 and struct.unpack_from('<I',b,at+17)[0]==22:return True
  at=end
 return False
try:
 while proc.poll() is None:
  if ctl.exists() and not settled:
   try:
    with socket.socket(socket.AF_UNIX) as sock:
     sock.settimeout(2);sock.connect(str(ctl));sock.sendall(b'{"metrics":true}\n');f=sock.makefile('rb');reply=json.loads(f.readline())
    snapshots.append(reply)
    settled=reply.get('worker_phase')=='running' and reply['width']['effective']==a.width and reply['width']['pending']==0
   except (FileNotFoundError,ConnectionRefusedError,ConnectionResetError):pass
  if a.mode=='cancel' and settled and not sent and signals_ready():
   signal.pidfd_send_signal(pidfd,signal.SIGINT);sent=True
  if settled and (a.mode!='cancel' or sent):break
  for key,_ in sel.select(5):
   if key.fd==fd:
    try:os.read(fd,65536)
    except BlockingIOError:pass
  # A control reply in starting state can advance to running without another
  # FS notification. Continue control transactions immediately at that point.
  if ctl.exists() and not settled:continue
 out,err=proc.communicate(timeout=30)
finally:
 os.close(fd);os.close(pidfd);sel.close()
(root/'stdout').write_bytes(out);(root/'stderr').write_bytes(err);(root/'metrics.json').write_text(json.dumps(snapshots,indent=2)+'\n')
receipt=dict(mode=a.mode,width=a.width,exit=proc.returncode,stdout=out.hex(),stderr=err.hex(),settled=settled,signal_sent=sent,tape='run.tape',steps=40000000)
(root/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
assert (proc.returncode,out,err)==(0 if a.mode=='success' else 2,expected_out,expected_error),receipt
if a.mode=='success':assert (world/'roots/head').read_bytes()==expected_out
else:assert not (world/'roots/head').exists()
# Every newly published object is independently framed and hashed.
for file in (world/'objects').iterdir():assert hashlib.sha256(file.read_bytes()).hexdigest()==file.name
shutil.rmtree(world);grant.unlink();receipt['tape_sha256']=hashlib.sha256(tape.read_bytes()).hexdigest();(root/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt))
