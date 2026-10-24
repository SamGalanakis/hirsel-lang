"""Full-entry production Ordered/real-file qualification; no test scheduler."""
import pathlib,subprocess,json,os,hashlib,socket,time,signal,sys,struct
root=pathlib.Path(sys.argv[1]).resolve();hirsel=pathlib.Path(sys.argv[2]).resolve();capped=pathlib.Path(sys.argv[3]).resolve()
env=dict(os.environ,HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def invoke(argv,name,execution=False):
 (root/(name+'.argv.json')).write_text(json.dumps([str(a) for a in argv]))
 if execution:argv=[capped,'--timeout','60','prlimit','--cpu=60:61',*argv]
 out=subprocess.run([str(a) for a in argv],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 (root/(name+'.stdout')).write_bytes(out.stdout);(root/(name+'.stderr')).write_bytes(out.stderr)
 (root/(name+'.exit')).write_text(str(out.returncode))
 return out
def signal_projected(tape):
 # Read-only observer of the existing v8 envelope, docs/spec/determinism.md.
 # Trace events publish at completion; the tape flushes each completed event.
 if not tape.exists():return None
 data=tape.read_bytes()
 if data[:17]!=b'hirsel-io-tape 9\0':return None
 at=17;expected=0
 while at+17<=len(data):
  length,checksum,seq,kind=struct.unpack_from('<IIQB',data,at)
  if seq!=expected or at+17+length>len(data):return None
  payload=data[at+17:at+17+length]
  if kind==3 and payload[:4]==struct.pack('<I',22):
   # CRC32C of length, sequence, kind and payload (checksum field excluded).
   crc=0xffffffff
   for b in data[at:at+4]+data[at+8:at+17]+payload:
    crc^=b
    for _ in range(8):crc=(crc>>1)^(0x82f63b78 if crc&1 else 0)
   if (crc^0xffffffff)!=checksum:raise RuntimeError('signal frame CRC')
   return dict(frame=seq,sha256=hashlib.sha256(payload).hexdigest())
  at+=17+length;expected+=1
 return None
receipts=[]
for backend in ['llvm','cranelift']:
 exe=root/('app-'+backend)
 if os.environ.get('CHUNK_BACKUP_REUSE'):
  assert exe.is_file();continue
 built=invoke([capped,'--timeout','300',hirsel,'build',root/'hirsel.json','--backend',backend,'-o',exe],'build-'+backend)
 if built.returncode:raise RuntimeError(('build',backend,built.stderr.decode()))
 (root/(backend+'-binary.sha256')).write_text(sha(exe))
if os.environ.get('CHUNK_BACKUP_BUILD_ONLY'):sys.exit(0)
for backend in ['llvm','cranelift']:
 exe=root/('app-'+backend)
 for mode in os.environ.get('CHUNK_BACKUP_MODES','success,domain,cancel').split(','):
  for width in map(int,os.environ.get('CHUNK_BACKUP_WIDTHS','1,2,4').split(',')):
   name=f'{backend}-{mode}-{width}';d=root/name;d.mkdir()
   for mount in ['source','chunks','snapshots','target']:(d/mount).mkdir()
   payload=bytes([1])*32768
   rows=[]
   for i in range(4):
    data=payload;identity=hashlib.sha256(data).hexdigest()
    if mode=='domain' and i==0:identity='0'*64
    (d/'source'/str(i)).write_bytes(data);rows.append(dict(path=str(i),size=len(data),id=identity))
   if mode=='pubfault':(d/'chunks').chmod(0o555)
   (d/'source/input.json').write_text(json.dumps(rows,separators=(',',':')))
   # Short control pathname under /tmp avoids sockaddr_un's 108-byte limit.
   import tempfile
   control_parent=pathlib.Path(tempfile.mkdtemp(prefix='cb31-'));control=control_parent/'ctl'
   grants=dict(args=['backup','input.json','daily','4'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',signals=dict(provider='signals.host@1',kinds=['interrupt']),dirs=[dict(path=m,host=str(d/m),access='read' if m=='source' else 'read_write') for m in ['source','chunks','snapshots','target']]),placement=dict(cores=width,commit='shared',reorder='off',control=str(control)))
   g=d/'grants.json';g.write_text(json.dumps(grants));tape=root/(name+'.tape');trace=root/(name+'.trace')
   argv=[str(capped),'--timeout','60','prlimit','--cpu=60:61',str(exe),'--hirsel-trace-events',str(trace),'--hirsel-record',str(tape),'--hirsel-grants',str(g),*grants['args']]
   (root/(name+'.argv.json')).write_text(json.dumps(argv));(root/(name+'.inputs.sha256')).write_text(json.dumps({str(p):sha(p) for p in d.rglob('*') if p.is_file()}))
   with open(root/(name+'.stdout'),'wb') as stdout,open(root/(name+'.stderr'),'wb') as stderr:
    child=subprocess.Popen(argv,env=env,stdout=stdout,stderr=stderr)
    settled=None;samples=[];deadline=time.monotonic()+60;sent=False
    while child.poll() is None and time.monotonic()<deadline:
     try:
      with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
       client.settimeout(1);client.connect(str(control));client.sendall(b'{"metrics":true}\n');reply=b''
       while b'\n' not in reply:reply+=client.recv(65536)
       peer_pid,peer_uid,peer_gid=struct.unpack('3i',client.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12));assert peer_uid==os.getuid();receipt=json.loads(reply);samples.append(receipt)
       if receipt.get('worker_phase')=='running' and receipt.get('width',{}).get('effective')==width and receipt['width'].get('pending')==0:
        settled=receipt
        if mode=='cancel' and not sent:
         # A signal is safe only after the app has projected its signal grant.
         # Trace handshake records that operation, never a sleep-based guess.
         handshake=signal_projected(tape)
         if handshake:
          (root/(name+'.signal-handshake.json')).write_text(json.dumps(dict(peer_pid=peer_pid,control=receipt,**handshake)))
          pid=peer_pid
          if pid:os.kill(pid,signal.SIGINT);sent=True
     except (OSError,ValueError,KeyError):pass
    status=child.wait(timeout=10)
   (root/(name+'.control.json')).write_text(json.dumps(samples,indent=2));(root/(name+'.exit')).write_text(str(status))
   result=dict(id=name,status=status,settled=settled is not None,cancel_sent=sent,tape=str(tape))
   if mode!='cancel':
    expected=0 if mode=='success' else 2
    if status!=expected:raise RuntimeError(result)
   else:
    result['incomplete']=not(sent and status==2)
   if mode=='success':
    snap=json.loads((d/'snapshots/daily').read_text());assert len(snap['files'])==4
   if mode in ['domain','pubfault'] or mode=='cancel' and sent:assert not(d/'snapshots/daily').exists()
   for p in (d/'chunks').iterdir():assert sha(p)==p.name
   # Original providers unavailable during every replay.
   import shutil
   shutil.copytree(d,root/('archive-'+name));shutil.rmtree(d);control_parent.rmdir()
   if tape.exists() and status in [0,2]:
    result['replay']=[]
    for other in ['llvm','cranelift']:
     candidate=root/('app-'+other)
     if candidate.exists():
      replay=invoke([candidate,'--hirsel-replay',tape],name+'-replay-'+other,True)
      assert replay.returncode==status,(result,other,replay.stderr)
      assert replay.stdout==(root/(name+'.stdout')).read_bytes()
      result['replay'].append(other)
   receipts.append(result);(root/'production-receipts.json').write_text(json.dumps(receipts,indent=2))
print(json.dumps(receipts))
