"""Deferred Ordered capacity qualification on the real geoaudit entry.

Adapted from pnm_tiles_30/qualify.py's inotify + control-socket discipline.
Run only after exact-cpu-request-31 is deployed; no worker/topology overrides.
No waiting timer counts as cancellation. A missing control reply is INCOMPLETE.
"""
import argparse,ctypes,hashlib,json,os,pathlib,select,signal,socket,struct,subprocess,time
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--hirsel',type=pathlib.Path,required=True);p.add_argument('--manifest',type=pathlib.Path,required=True);p.add_argument('--backend',choices=['llvm','cranelift'],required=True);p.add_argument('--root',type=pathlib.Path,required=True)
p.add_argument('--width',type=int,choices=[1,2,4],required=True);p.add_argument('--kind',choices=['success','domain','cancel'],required=True)
p.add_argument('--fixture-root',type=pathlib.Path,required=True)
a=p.parse_args();r=a.root.resolve();r.mkdir(parents=True);data=r/'data';data.mkdir()
rows=json.loads((a.fixture_root/'cases.json').read_text());source=next(x for x in rows if x['id']=='input-32768')
b=json.loads(source['input'])
if a.kind=='domain':b['version']=2
text=json.dumps(b,separators=(',',':'));text+=' '*(32768-len(text))
(data/'tracks.json').write_text(text);(data/'audit.json').write_bytes(b'preserve')
binary=r/f'ordered-{a.backend}'
# Setup/build is not measured; the executable itself is launched under the
# caller's exact-CPU admission and the capped outer test harness.
built=subprocess.run([str(a.hirsel),'build','--backend',a.backend,str(a.manifest),'-o',str(binary)],capture_output=True)
(r/'build-stdout').write_bytes(built.stdout);(r/'build-stderr').write_bytes(built.stderr)
assert built.returncode==0,built.stderr
control=r/'ctl';tape=r/'recording';trace=r/'trace.jsonl'
words=['data','tracks.json','audit.json','4']+(['--interruptible'] if a.kind=='cancel' else [])
grant=dict(args=words,io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='data',host=str(data),access='read_write')]),placement=dict(cores=a.width,commit='shared',reorder='off',control=str(control)))
if a.kind=='cancel':grant['io']['signals']=dict(provider='signals.host@1',kinds=['interrupt'])
(r/'grants.json').write_text(json.dumps(grant))
lib=ctypes.CDLL(None,use_errno=True);notify=lib.inotify_init1(os.O_CLOEXEC|os.O_NONBLOCK);assert notify>=0
assert lib.inotify_add_watch(notify,os.fsencode(r),0x102)>=0
assert lib.inotify_add_watch(notify,os.fsencode(data),0x20)>=0
cmd=[str(binary.resolve()),'--hirsel-grants',str(r/'grants.json'),'--hirsel-record',str(tape),'--hirsel-trace-events',str(trace),*words]
(r/'consumer-before.json').write_text(json.dumps(dict(argv=cmd,environment={k:v for k,v in os.environ.items() if k.startswith('HIRSEL_')},files={str(x):hashlib.sha256(x.read_bytes()).hexdigest() for x in [binary,pathlib.Path(__file__),r/'grants.json',data/'tracks.json']}),indent=2))
child=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE);pidfd=os.pidfd_open(child.pid)
samples=[];opened=False;ready=False;sent=False;deadline=time.monotonic()+45
while child.poll() is None and time.monotonic()<deadline:
 if control.exists() and opened:
  try:
   with socket.socket(socket.AF_UNIX) as conn:
    conn.settimeout(1);conn.connect(str(control));conn.sendall(b'{"metrics":true}\n');value=json.loads(conn.makefile('rb').readline());samples.append(value)
    ready=value.get('worker_phase')=='running' and value['width']['requested']==a.width and value['width']['effective']==a.width and value['width']['pending']==0
  except (OSError,ValueError):pass
 if ready and a.kind=='cancel' and not sent:
  try:
   mask=int(next(line.split()[1] for line in pathlib.Path(f'/proc/{child.pid}/status').read_text().splitlines() if line.startswith('SigCgt:')),16)
   if mask&(1<<(signal.SIGINT-1)):child.send_signal(signal.SIGINT);sent=True
  except (FileNotFoundError,StopIteration):pass
 if ready and (a.kind!='cancel' or sent):break
 rd,_,_=select.select([notify,pidfd],[],[],max(0,deadline-time.monotonic()))
 if notify in rd:
  raw=os.read(notify,65536);i=0
  while i<len(raw):
   wd,mask,cookie,size=struct.unpack_from('iIII',raw,i);name=raw[i+16:i+16+size].split(b'\0',1)[0];i+=16+size
   if mask&0x20 and name==b'tracks.json':opened=True
out,err=child.communicate(timeout=max(.1,deadline-time.monotonic()));os.close(pidfd);os.close(notify)
(r/'stdout').write_bytes(out);(r/'stderr').write_bytes(err)
receipt=dict(backend=a.backend,selected=True,started=True,completed=True,passed=False,kind=a.kind,width=a.width,controls=samples,opened=opened,signal_delivered=sent,exit=child.returncode)
(r/'receipt.json').write_text(json.dumps(receipt,indent=2))
expected=0 if a.kind=='success' else 2
assert child.returncode==expected,(child.returncode,err)
assert out==(b'geoaudit: 1 tracks\n' if a.kind=='success' else b'')
assert err==(b'' if a.kind=='success' else b'geoaudit: version\n' if a.kind=='domain' else b'geoaudit: canceled\n')
if a.kind!='success':assert (data/'audit.json').read_bytes()==b'preserve'
assert sorted(x.name for x in data.iterdir())==['audit.json','tracks.json']
if a.kind=='success':assert (data/'audit.json').read_text()==source['expected']
assert not control.exists(),'control socket leaked'
assert ready, 'INCOMPLETE: no post-open running/effective=requested/pending0 control reply'
assert a.kind!='cancel' or sent,'INCOMPLETE: no delivered cancellation'
# Independent real-host recordings replay only against their own immutable tape.
frozen=tape.read_bytes();retained=r/'unavailable';data.rename(retained)
prior={x.name:x.read_bytes() for x in retained.iterdir()}
replays=[]
for player in sorted({binary.resolve(),binary.resolve().parent/'ordered-llvm',binary.resolve().parent/'ordered-cranelift'}):
 if not player.is_file():continue
 replay=subprocess.run([str(player),'--hirsel-replay',str(tape)],capture_output=True,timeout=45)
 assert (replay.returncode,replay.stdout,replay.stderr)==(child.returncode,out,err),replay
 assert {x.name:x.read_bytes() for x in retained.iterdir()}==prior and not data.exists(),'offline host mutation'
 assert tape.read_bytes()==frozen
 replays.append(str(player))
receipt['replays']=replays
receipt['passed']=True;(r/'receipt.json').write_text(json.dumps(receipt,indent=2))
print(json.dumps(receipt))
