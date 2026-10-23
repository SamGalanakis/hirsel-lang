"""Ordinary Ordered placement and signal-cleanup acceptance, without worker hooks."""
import argparse,ctypes,hashlib,json,os,select,signal,socket,struct,subprocess,sys,time
from pathlib import Path

def cell(a):
    r=a.root;case=json.loads((r/'case.json').read_text()); mount=r/'data'
    lib=ctypes.CDLL(None,use_errno=True)
    notify=lib.inotify_init1(os.O_CLOEXEC|os.O_NONBLOCK);assert notify>=0
    assert lib.inotify_add_watch(notify,os.fsencode(r),0x100)>=0
    assert lib.inotify_add_watch(notify,os.fsencode(mount),0x20)>=0
    control=r/'ctl'; tape=r/'record.tape'; trace=r/'trace.jsonl'
    command=[str(a.binary),'--hirsel-trace-events',str(trace),'--hirsel-record',str(tape),'--hirsel-grants',str(r/'grants.json'),*case['args']]
    (r/'argv.json').write_text(json.dumps(command)+'\n')
    p=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    pidfd=os.pidfd_open(p.pid);observations=[]; opened=False; sent=False; ready=False; saw_control=False
    deadline=time.monotonic()+40
    while p.poll() is None and time.monotonic()<deadline:
        if saw_control:
            try:
                with socket.socket(socket.AF_UNIX) as s:
                    s.settimeout(2);s.connect(str(control));s.sendall(b'{"metrics":true}\n');payload=b''
                    while not payload.endswith(b'\n'):
                        chunk=s.recv(65536)
                        if not chunk:break
                        payload+=chunk
                    value=json.loads(payload);observations.append(value)
                    ready=value.get('worker_phase')=='running' and value['width']['effective']==a.width and value['width']['pending']==0
            except (OSError,ValueError):pass
            if ready and (a.kind!='cancel' or sent): break
        rd,_,_=select.select([notify,pidfd],[],[],0 if saw_control else max(0,deadline-time.monotonic()))
        if notify in rd:
            b=os.read(notify,65536);i=0
            while i<len(b):
                wd,mask,cookie,size=struct.unpack_from('iIII',b,i);name=b[i+16:i+16+size].split(b'\0',1)[0];i+=16+size
                if name==b'ctl':saw_control=True
                if mask&0x20 and name==b'input':opened=True
        if ready and opened and a.kind=='cancel' and not sent and p.poll() is None and (int(next(line.split()[1] for line in Path(f'/proc/{p.pid}/status').read_text().splitlines() if line.startswith('SigCgt:')),16) & (1 << (signal.SIGINT-1))):
            p.send_signal(signal.SIGINT);sent=True
    try:stdout,stderr=p.communicate(timeout=max(.1,deadline-time.monotonic()))
    except subprocess.TimeoutExpired:
        p.kill();stdout,stderr=p.communicate();raise
    os.close(pidfd);os.close(notify)
    result=dict(status=p.returncode,stdout=list(stdout),stderr=list(stderr),ready=ready,sent=sent,opened=opened,samples=observations)
    (r/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    assert p.returncode==case['exit'],result
    assert list(stdout)==case['stdout'] and list(stderr)==case['stderr'],result
    if case['expected'] is not None:assert (mount/'output').read_bytes()==bytes(case['expected'])
    else:assert (mount/'output').read_bytes()==b'preserve'
    assert sorted(p.name for p in mount.iterdir())==['input','output']
    assert not control.exists()
    # Preserve semantic success independently of incomplete width evidence.
    result['semantic_pass']=True
    (r/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(dict(id=r.name,semantic='PASS',width='PASS' if ready else 'INCOMPLETE',effective=observations[-1].get('width') if observations else None)))

def campaign(a):
    data=json.loads((a.app_root/'cases.json').read_text());out=a.root;out.mkdir()
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_TIMING='off',HIRSEL_DAEMON='off')
    records=[]
    for backend in ['llvm','cranelift']:
        binary=a.app_root/'native'/backend
        for kind in (a.only.split(',') if a.only else ['success','domain','cancel']):
            for width in [1,2,4]:
                r=out/f'{backend}-{kind}-{width}';r.mkdir();(r/'data').mkdir()
                if kind=='success':
                    src=data['files']['7x33-c3-blur'];row=next(c for c in data['cases'] if c['id']=='7x33-c3-blur').copy()
                elif kind=='domain':
                    src=data['files']['header-over'];row=next(c for c in data['cases'] if c['id']=='header-over').copy();row.pop('reference_disposition',None)
                else:
                    src=data['files']['cap-info'];row=dict(args=['filter','data','input','output','blur','4','--interruptible'],exit=2,stdout=[],stderr=list(b'pnmtiles: canceled\n'),expected=None,bound=16000000)
                row['args'][2]='input'
                if kind=='domain':row['args']=['filter','data','input','output','blur','4'];row['reference_disposition']='Steps'
                (r/'data'/'input').write_bytes(bytes(src) if isinstance(src,list) else bytes(src['prefix'])+bytes([src['value']])*src['repeat']);(r/'data'/'output').write_bytes(b'preserve')
                grant=dict(args=row['args'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='data',host=str(r/'data'),access='read_write')]),placement=dict(cores=width,commit='shared',reorder='auto',control=str(r/'ctl')))
                if kind=='cancel':grant['io']['signals']=dict(provider='signals.host@1',kinds=['interrupt'])
                (r/'grants.json').write_text(json.dumps(grant)+'\n');(r/'case.json').write_text(json.dumps(row)+'\n')
                cmd=[sys.executable,str(a.app_root/'capped'),'--timeout','60','prlimit','--cpu=60:61',sys.executable,str(Path(__file__).resolve()),'--cell','--root',str(r),'--binary',str(binary),'--width',str(width),'--kind',kind]
                (r/'consumer-before.json').write_text(json.dumps(dict(argv=cmd,files={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [binary,Path(__file__),r/'grants.json',r/'case.json',r/'data'/'input']}),indent=2)+'\n')
                done=subprocess.run(cmd,env=env,capture_output=True,timeout=90)
                (r/'stdout').write_bytes(done.stdout);(r/'stderr').write_bytes(done.stderr);(r/'exit').write_text(str(done.returncode)+'\n')
                print(r.name,done.returncode,done.stdout.decode(),flush=True)
                if done.returncode==0:
                    (r/'data').rename(r/'unavailable');(r/'grants.json').unlink()
                    records.append(dict(row,id=r.name,tape=str(r/'record.tape'),result=str(r/'result.json')))
                (out/'records.json').write_text(json.dumps(records)+'\n')
    assert len(records)==(len(a.only.split(',')) if a.only else 3)*6,f'{len(records)} semantic completions'

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--only');p.add_argument('--cell',action='store_true');p.add_argument('--root',type=Path,required=True);p.add_argument('--binary',type=Path);p.add_argument('--app-root',type=Path);p.add_argument('--width',type=int);p.add_argument('--kind');a=p.parse_args()
    cell(a) if a.cell else campaign(a)
